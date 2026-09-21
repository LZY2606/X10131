//! 规则指纹与分页游标。
//!
//! 指纹对规则的规范 JSON（键排序）做 SHA-256；游标把指纹、查询方向与定位瞬间
//! 绑定在一起。规则任意字段改变都会得到不同指纹，旧游标随即被拒绝。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::{Direction, RuleVersion};

/// 计算规则指纹（16 进制 SHA-256）。
pub fn rule_fingerprint(rule: &RuleVersion) -> String {
    let canonical =
        serde_json::to_string(&SortedRule(rule)).expect("规则可序列化为 JSON");
    let mut hasher = Sha256::new();
    hasher.update(b"periodic-arbiter-rule/v1\n");
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())
}

/// 包装器：借助 serde_json 的 BTreeMap 表示得到键排序的规范序列化，
/// 使指纹不受字段书写顺序影响。
struct SortedRule<'a>(&'a RuleVersion);

impl<'a> Serialize for SortedRule<'a> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = serde_json::to_value(self.0).expect("规则可转 JSON 值");
        let sorted = sort_json(value);
        sorted.serialize(serializer)
    }
}

fn sort_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, serde_json::Value> =
                std::collections::BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k, sort_json(v));
            }
            serde_json::Value::Object(
                sorted.into_iter().collect::<serde_json::Map<_, _>>(),
            )
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(sort_json).collect())
        }
        other => other,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CursorPayload {
    /// 游标格式版本。
    v: u32,
    /// 绑定的规则指纹。
    fp: String,
    /// 固定时区数据库标识。
    tzdb: String,
    /// 查询方向；游标只能沿签发方向继续。
    dir: Direction,
    /// 已返回的最后一个瞬间（正向=最大、反向=最小）。
    last_epoch: i64,
}

/// 游标错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorError {
    Decode(String),
    FingerprintMismatch,
    TzdbMismatch,
    DirectionMismatch,
}

impl std::fmt::Display for CursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CursorError::Decode(m) => write!(f, "游标无法解码: {m}"),
            CursorError::FingerprintMismatch => {
                f.write_str("游标绑定的规则指纹与当前规则不一致（规则已变更），旧游标被拒绝")
            }
            CursorError::TzdbMismatch => {
                f.write_str("游标绑定的时区数据库版本与当前实例不一致，旧游标被拒绝")
            }
            CursorError::DirectionMismatch => {
                f.write_str("游标签发方向与本次查询方向不一致，旧游标被拒绝")
            }
        }
    }
}
impl std::error::Error for CursorError {}

fn b64url_encode(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | bytes[i + 2] as u32;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(T[((n >> 6) & 63) as usize] as char);
        out.push(T[(n & 63) as usize] as char);
        i += 3;
    }
    match bytes.len() - i {
        1 => {
            let n = (bytes[i] as u32) << 16;
            out.push(T[((n >> 18) & 63) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
        }
        2 => {
            let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
            out.push(T[((n >> 18) & 63) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
            out.push(T[((n >> 6) & 63) as usize] as char);
        }
        _ => {}
    }
    out
}

fn b64url_decode(s: &str) -> Result<Vec<u8>, String> {
    let val = |c: u8| -> Option<i32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as i32),
            b'a'..=b'z' => Some((c - b'a' + 26) as i32),
            b'0'..=b'9' => Some((c - b'0' + 52) as i32),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    };
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let (groups, rem) = bytes.as_chunks::<4>();
    for chunk in groups {
        let mut n = 0u32;
        for &c in chunk {
            n = (n << 6) | val(c).ok_or("非法 base64url 字符")? as u32;
        }
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
    }
    match rem.len() {
        0 => {}
        2 => {
            let n = ((val(rem[0]).ok_or("非法字符")? as u32) << 18)
                | ((val(rem[1]).ok_or("非法字符")? as u32) << 12);
            out.push((n >> 16) as u8);
        }
        3 => {
            let n = ((val(rem[0]).ok_or("非法字符")? as u32) << 18)
                | ((val(rem[1]).ok_or("非法字符")? as u32) << 12)
                | ((val(rem[2]).ok_or("非法字符")? as u32) << 6);
            out.push((n >> 16) as u8);
            out.push((n >> 8) as u8);
        }
        _ => return Err("base64url 长度非法".to_string()),
    }
    Ok(out)
}

/// 签发下一页游标。
pub fn encode_cursor(
    fingerprint: &str,
    tzdb: &str,
    direction: Direction,
    last_epoch: i64,
) -> String {
    let payload = CursorPayload {
        v: 1,
        fp: fingerprint.to_string(),
        tzdb: tzdb.to_string(),
        dir: direction,
        last_epoch,
    };
    let json = serde_json::to_vec(&payload).expect("游标可序列化");
    b64url_encode(&json)
}

/// 校验并解码游标。
pub fn decode_cursor(
    cursor: &str,
    expected_fingerprint: &str,
    expected_tzdb: &str,
    direction: Direction,
) -> Result<i64, CursorError> {
    let raw = b64url_decode(cursor).map_err(CursorError::Decode)?;
    let payload: CursorPayload =
        serde_json::from_slice(&raw).map_err(|e| CursorError::Decode(e.to_string()))?;
    if payload.v != 1 {
        return Err(CursorError::Decode(format!("不支持的游标版本 {}", payload.v)));
    }
    if payload.fp != expected_fingerprint {
        return Err(CursorError::FingerprintMismatch);
    }
    if payload.tzdb != expected_tzdb {
        return Err(CursorError::TzdbMismatch);
    }
    if payload.dir != direction {
        return Err(CursorError::DirectionMismatch);
    }
    Ok(payload.last_epoch)
}
