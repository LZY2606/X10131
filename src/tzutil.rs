//! 时区工具与夏令时（跳空/回拨）解析。
//!
//! 所有判定都在 UTC 瞬间层面完成；本地墙钟仅用于生成候选与展示。

use chrono::{DateTime, LocalResult, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;

use crate::model::{FoldPolicy, GapPolicy};

/// 解析 IANA 时区名。
pub fn parse_tz(name: &str) -> Result<Tz, String> {
    name.parse::<Tz>()
        .map_err(|_| format!("未知时区: {name}"))
}

/// 内置时区数据库版本（chrono-tz 0.9 自带 2024a）。
pub fn tzdb_version() -> &'static str {
    "2024a (chrono-tz 0.9.0 bundled)"
}

/// 回拨时的具体一侧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldSide {
    /// 唯一合法（或跳空平移）的时刻。
    Single,
    /// 早侧：较大 UTC 偏移、DST 仍生效的第一次出现。
    Early,
    /// 晚侧：较小 UTC 偏移、回拨后的第二次出现。
    Late,
}

/// 一个本地墙钟解析出的候选瞬间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedInstant {
    pub epoch: i64,
    pub side: FoldSide,
    /// 跳空时若采用 ShiftForward，记录向前平移的秒数。
    pub shifted_by_seconds: i64,
}

fn wall_at(tz: Tz, epoch: i64) -> NaiveDateTime {
    Utc.timestamp_opt(epoch, 0)
        .single()
        .expect("规则边界 epoch 秒在 chrono 支持范围内")
        .with_timezone(&tz)
        .naive_local()
}

/// 找到跳空结束后的第一个合法 UTC 瞬间。
///
/// 思路：本地墙钟函数 wall(t)=t+offset(t) 在跳空处向前跳跃。
/// 以肯定落在跳空之前的瞬间为锚，指数+二分搜索第一个满足 wall(t)>=目标墙钟的 t。
fn gap_end_instant(tz: Tz, target: NaiveDateTime) -> i64 {
    let as_utc = target.and_utc().timestamp();
    // 任意时区偏移不超过 ±14 小时，24 小时前的瞬间其墙钟必然早于目标。
    let mut lo = as_utc - 86_400;
    let mut hi = lo;
    let mut step: i64 = 60;
    let cap = as_utc + 86_400;
    while wall_at(tz, hi) < target && hi < cap {
        hi = (hi + step).min(cap);
        step = step.saturating_mul(2).min(86_400);
    }
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if wall_at(tz, mid) >= target {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// 按策略把本地墙钟时间解析为 0、1 或 2 个 UTC 瞬间。
pub fn resolve_local(
    tz: Tz,
    local: NaiveDateTime,
    gap: GapPolicy,
    fold: FoldPolicy,
) -> Vec<ResolvedInstant> {
    match tz.from_local_datetime(&local) {
        LocalResult::Single(dt) => vec![ResolvedInstant {
            epoch: dt.timestamp(),
            side: FoldSide::Single,
            shifted_by_seconds: 0,
        }],
        LocalResult::None => match gap {
            GapPolicy::Skip => vec![],
            GapPolicy::ShiftForward => {
                let end = gap_end_instant(tz, local);
                // 平移量按本地墙钟差度量（如 02:30 → 03:00 = 1800 秒）。
                let shifted = (wall_at(tz, end) - local).num_seconds();
                vec![ResolvedInstant {
                    epoch: end,
                    side: FoldSide::Single,
                    shifted_by_seconds: shifted,
                }]
            }
        },
        LocalResult::Ambiguous(early, late) => {
            let make = |dt: DateTime<Tz>, side: FoldSide| ResolvedInstant {
                epoch: dt.timestamp(),
                side,
                shifted_by_seconds: 0,
            };
            match fold {
                FoldPolicy::Early => vec![make(early, FoldSide::Early)],
                FoldPolicy::Late => vec![make(late, FoldSide::Late)],
                FoldPolicy::Both => vec![
                    make(early, FoldSide::Early),
                    make(late, FoldSide::Late),
                ],
            }
        }
    }
}

/// UTC 瞬间在指定时区的本地展示信息。
#[derive(Debug, Clone)]
pub struct LocalView {
    pub wall: String,
    pub offset_seconds: i32,
    pub offset_name: String,
}

pub fn view_instant(tz: Tz, epoch: i64) -> LocalView {
    let dt = Utc
        .timestamp_opt(epoch, 0)
        .single()
        .expect("epoch 秒在 chrono 支持范围内")
        .with_timezone(&tz);
    LocalView {
        wall: dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        offset_seconds: dt.offset().fix().local_minus_utc(),
        offset_name: dt.format("%Z").to_string(),
    }
}
