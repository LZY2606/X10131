//! API 编排：把引擎、指纹游标和存储串联起来。

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::engine::{Engine, Provenance, Verdict, VerdictKind};
use crate::fingerprint;
use crate::model::{Direction, RuleVersion};
use crate::storage::{QueryRecord, SessionRecord, Store};
use crate::tzutil;

#[derive(Debug, Clone, Serialize)]
pub struct SourceDto {
    pub rule_id: String,
    pub rule_kind: String,
    pub local_wall: String,
    pub side: String,
    pub shifted_by_seconds: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerdictDto {
    pub epoch: i64,
    pub local_wall: String,
    pub offset_name: String,
    pub offset_seconds: i32,
    pub kind: &'static str,
    pub sources: Vec<SourceDto>,
    pub checks: Vec<String>,
}

impl From<&Provenance> for SourceDto {
    fn from(p: &Provenance) -> Self {
        SourceDto {
            rule_id: p.rule_id.clone(),
            rule_kind: p.rule_kind.clone(),
            local_wall: p.local_wall.clone(),
            side: p.side.to_string(),
            shifted_by_seconds: p.shifted_by_seconds,
        }
    }
}

impl From<&Verdict> for VerdictDto {
    fn from(v: &Verdict) -> Self {
        VerdictDto {
            epoch: v.epoch,
            local_wall: v.local_wall.clone(),
            offset_name: v.offset_name.clone(),
            offset_seconds: v.offset_seconds,
            kind: match v.kind {
                VerdictKind::Accepted => "accepted",
                VerdictKind::Excluded => "excluded",
                VerdictKind::TooSoon => "too_soon",
            },
            sources: v.sources.iter().map(SourceDto::from).collect(),
            checks: v.checks.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub rule: RuleVersion,
    pub direction: Direction,
    pub limit: usize,
    pub cursor: Option<String>,
    /// 首页之后每次查询都应提供 session_id；缺省时新建会话。
    pub session_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub rule_fingerprint: String,
    pub tzdb: String,
    pub session_id: String,
    pub direction: Direction,
    pub anchor_epoch: Option<i64>,
    pub items: Vec<VerdictDto>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub error: Option<String>,
}

pub struct AppState {
    pub store: Store,
}

pub type SharedState = Arc<Mutex<AppState>>;

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

fn api_err(status: u16, message: impl Into<String>) -> ApiError {
    ApiError { status, message: message.into() }
}

pub fn list_timezones() -> Vec<&'static str> {
    crate::engine::known_tz_list()
}

/// 校验并保存规则（不产生查询）。
pub fn save_rule(state: &mut AppState, rule: RuleVersion) -> Result<String, ApiError> {
    let engine = Engine::new(rule.clone()).map_err(|e| api_err(400, e.to_string()))?;
    // 构造一次以确保规则可裁决（校验逻辑在 Engine::new 内）。
    let _ = engine.evaluate();
    let fp = fingerprint::rule_fingerprint(&rule);
    state
        .store
        .save_rule(&rule)
        .map_err(|e| api_err(500, e.to_string()))?;
    Ok(fp)
}

#[derive(Debug, Serialize)]
pub struct EvaluateResponse {
    pub rule_fingerprint: String,
    pub tzdb: String,
    pub items: Vec<VerdictDto>,
}

/// 只计算（不落盘），用于预览。
pub fn evaluate_rule(rule: RuleVersion) -> Result<EvaluateResponse, ApiError> {
    let engine = Engine::new(rule.clone()).map_err(|e| api_err(400, e.to_string()))?;
    Ok(EvaluateResponse {
        rule_fingerprint: fingerprint::rule_fingerprint(&rule),
        tzdb: tzutil::tzdb_version().to_string(),
        items: engine.evaluate().iter().map(VerdictDto::from).collect(),
    })
}

pub fn handle_query(state: &mut AppState, req: QueryRequest) -> Result<QueryResponse, ApiError> {
    let limit = req.limit.clamp(1, 500);
    let engine = Engine::new(req.rule.clone()).map_err(|e| api_err(400, e.to_string()))?;
    let fp = fingerprint::rule_fingerprint(&req.rule);
    let tzdb = tzutil::tzdb_version();

    // 游标校验：指纹/方向/tzdb 任一不匹配都拒绝旧游标。
    let anchor: Option<i64> = match &req.cursor {
        None => None,
        Some(c) => Some(
            fingerprint::decode_cursor(c, &fp, tzdb, req.direction).map_err(
                |e| api_err(409, e.to_string()),
            )?,
        ),
    };

    let all = engine.evaluate();
    let page = engine.page(&all, req.direction, anchor, limit);

    // has_more：本页之后是否还有结果。
    let has_more = match (req.direction, page.last()) {
        (Direction::Forward, Some(last)) => all.iter().any(|v| v.epoch > last.epoch),
        (Direction::Backward, Some(last)) => all.iter().any(|v| v.epoch < last.epoch),
        (_, None) => false,
    };
    let next_cursor = page.last().map(|last| {
        fingerprint::encode_cursor(&fp, tzdb, req.direction, last.epoch)
    });

    // 会话：新建或沿用并追加查询记录。
    let session_id = req
        .session_id
        .clone()
        .unwrap_or_else(|| format!("sess-{}", &fp[..12]));
    let mut session = state
        .store
        .load_session(&session_id)
        .map_err(|e| api_err(500, e.to_string()))?
        .unwrap_or_else(|| SessionRecord {
            session_id: session_id.clone(),
            rule_id: req.rule.rule_id.clone(),
            rule_fingerprint: fp.clone(),
            tzdb: tzdb.to_string(),
            rule_snapshot: req.rule.clone(),
            queries: Vec::new(),
            created_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        });
    // 会话内规则快照必须与当前指纹一致，否则拒绝在旧会话上继续。
    if session.rule_fingerprint != fp {
        return Err(api_err(
            409,
            "会话绑定的规则版本与当前规则不一致，旧会话/游标被拒绝；请以新规则开始查询",
        ));
    }
    let record = QueryRecord {
        direction: req.direction,
        limit,
        cursor: req.cursor.clone(),
        returned_epochs: page.iter().map(|v| v.epoch).collect(),
        next_cursor: next_cursor.clone(),
        anchor_epoch: anchor,
    };
    session.queries.push(record);
    state
        .store
        .save_session(&session)
        .map_err(|e| api_err(500, e.to_string()))?;
    state
        .store
        .save_rule(&req.rule)
        .map_err(|e| api_err(500, e.to_string()))?;

    Ok(QueryResponse {
        rule_fingerprint: fp,
        tzdb: tzdb.to_string(),
        session_id,
        direction: req.direction,
        anchor_epoch: anchor,
        items: page.iter().map(VerdictDto::from).collect(),
        next_cursor,
        has_more,
        error: None,
    })
}


#[derive(Debug, Deserialize)]
pub struct ResolveRequest {
    pub timezone: String,
    /// "YYYY-MM-DDTHH:MM:SS"
    pub local: String,
    pub gap_policy: crate::model::GapPolicy,
    pub fold_policy: crate::model::FoldPolicy,
}

#[derive(Debug, Serialize)]
pub struct ResolvedDto {
    pub epoch: i64,
    pub side: &'static str,
    pub shifted_by_seconds: i64,
    pub local_view: String,
    pub offset_name: String,
}

pub fn resolve_local_endpoint(req: ResolveRequest) -> Result<Vec<ResolvedDto>, ApiError> {
    let tz = tzutil::parse_tz(&req.timezone).map_err(|m| api_err(400, m))?;
    let local = chrono::NaiveDateTime::parse_from_str(&req.local, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&req.local, "%Y-%m-%dT%H:%M"))
        .map_err(|e| api_err(400, format!("本地时间格式非法: {e}")))?;
    Ok(tzutil::resolve_local(tz, local, req.gap_policy, req.fold_policy)
        .into_iter()
        .map(|r| {
            let view = tzutil::view_instant(tz, r.epoch);
            ResolvedDto {
                epoch: r.epoch,
                side: match r.side {
                    tzutil::FoldSide::Single => "single",
                    tzutil::FoldSide::Early => "early",
                    tzutil::FoldSide::Late => "late",
                },
                shifted_by_seconds: r.shifted_by_seconds,
                local_view: view.wall,
                offset_name: view.offset_name,
            }
        })
        .collect())
}
