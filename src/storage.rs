//! 本地落盘存储、会话导出/导入。
//!
//! 数据目录默认 `./.periodic-arbiter-data`，可用环境变量
//! `PERIODIC_ARBITER_DATA_DIR` 覆盖。文件布局：
//! - `rules/<rule_id>.json`：规则版本
//! - `sessions/<session_id>.json`：查询会话（含规则快照与页记录）

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::{Direction, RuleVersion};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryRecord {
    pub direction: Direction,
    pub limit: usize,
    pub cursor: Option<String>,
    /// 本页返回的 UTC epoch 秒（升序存储）。
    pub returned_epochs: Vec<i64>,
    pub next_cursor: Option<String>,
    /// 校验游标时使用的定位瞬间（None 表示首页）。
    pub anchor_epoch: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub rule_id: String,
    pub rule_fingerprint: String,
    pub tzdb: String,
    pub rule_snapshot: RuleVersion,
    pub queries: Vec<QueryRecord>,
    pub created_unix: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportEnvelope {
    pub format: String,
    pub format_version: u32,
    pub tzdb: String,
    pub session: SessionRecord,
    /// 对 `canonical_json(session)` 的 sha256，导入时校验。
    pub digest: String,
}

pub struct Store {
    root: PathBuf,
}

#[derive(Debug)]
pub struct StoreError(pub String);
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for StoreError {}

fn mkerr(msg: impl Into<String>) -> StoreError {
    StoreError(msg.into())
}

impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("rules")).map_err(|io| mkerr(io.to_string()))?;
        fs::create_dir_all(root.join("sessions")).map_err(|io| mkerr(io.to_string()))?;
        Ok(Store { root })
    }

    pub fn from_env(default: &str) -> Result<Self, StoreError> {
        let dir = std::env::var("PERIODIC_ARBITER_DATA_DIR").unwrap_or_else(|_| default.to_string());
        Store::open(dir)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn save_rule(&self, rule: &RuleVersion) -> Result<(), StoreError> {
        let path = self.root.join("rules").join(format!("{}.json", safe(&rule.rule_id)));
        write_atomic(&path, rule)
    }

    pub fn load_rule(&self, rule_id: &str) -> Result<Option<RuleVersion>, StoreError> {
        let path = self.root.join("rules").join(format!("{}.json", safe(rule_id)));
        if !path.exists() {
            return Ok(None);
        }
        read_json(&path)
    }

    pub fn list_rules(&self) -> Result<Vec<RuleVersion>, StoreError> {
        let dir = self.root.join("rules");
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|io| mkerr(io.to_string()))? {
            let path = entry.map_err(|io| mkerr(io.to_string()))?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                out.push(read_json::<RuleVersion>(&path)?);
            }
        }
        out.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
        Ok(out)
    }

    pub fn save_session(&self, session: &SessionRecord) -> Result<(), StoreError> {
        let path = self
            .root
            .join("sessions")
            .join(format!("{}.json", safe(&session.session_id)));
        write_atomic(&path, session)
    }

    pub fn load_session(&self, session_id: &str) -> Result<Option<SessionRecord>, StoreError> {
        let path = self
            .root
            .join("sessions")
            .join(format!("{}.json", safe(session_id)));
        if !path.exists() {
            return Ok(None);
        }
        read_json(&path)
    }

    pub fn list_sessions(&self) -> Result<Vec<String>, StoreError> {
        let dir = self.root.join("sessions");
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|io| mkerr(io.to_string()))? {
            let path = entry.map_err(|io| mkerr(io.to_string()))?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    out.push(stem.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

fn safe(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

fn mkerr_str<E: std::fmt::Display>(e: E) -> StoreError {
    mkerr(e.to_string())
}

fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let json = serde_json::to_string_pretty(value).map_err(|io| mkerr(io.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|io| mkerr(io.to_string()))?;
    fs::rename(&tmp, path).map_err(mkerr_str)?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, StoreError> {
    let bytes = fs::read(path).map_err(|io| mkerr(io.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|j| mkerr(format!("{}: {j}", path.display())))
}

/// 导出会话为自包含信封（含校验摘要）。
pub fn export_session(session: SessionRecord, tzdb: &str) -> ExportEnvelope {
    let canonical = canonical_session_json(&session);
    let digest = hex::encode(Sha256::digest(canonical.as_bytes()));
    ExportEnvelope {
        format: "periodic-arbiter-session".to_string(),
        format_version: 1,
        tzdb: tzdb.to_string(),
        session,
        digest,
    }
}

/// 导入并校验导出信封；返回会话记录与警告（如 tzdb 版本不同）。
pub fn import_envelope(envelope: ExportEnvelope) -> Result<(SessionRecord, Option<String>), StoreError> {
    if envelope.format != "periodic-arbiter-session" || envelope.format_version != 1 {
        return Err(mkerr("不支持的导出格式或版本"));
    }
    let canonical = canonical_session_json(&envelope.session);
    let actual = hex::encode(Sha256::digest(canonical.as_bytes()));
    if actual != envelope.digest {
        return Err(mkerr("会话摘要不一致：文件已损坏或被修改"));
    }
    let warning = if envelope.tzdb != crate::tzutil::tzdb_version() {
        Some(format!(
            "时区数据库版本不同：导出端 {} / 导入端 {}（固定库标识已记入游标与文件）",
            envelope.tzdb,
            crate::tzutil::tzdb_version()
        ))
    } else {
        None
    };
    Ok((envelope.session, warning))
}

/// 键排序的规范 JSON，保证不同实例序列化逐字节一致。
pub fn canonical_session_json(session: &SessionRecord) -> String {
    let value = serde_json::to_value(session).expect("会话可序列化");
    let sorted = sort_json(value);
    serde_json::to_string(&sorted).expect("规范 JSON 可序列化")
}

fn sort_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted: BTreeMap<String, serde_json::Value> = BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k, sort_json(v));
            }
            serde_json::Value::Object(sorted.into_iter().collect::<serde_json::Map<_, _>>())
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(sort_json).collect())
        }
        other => other,
    }
}
