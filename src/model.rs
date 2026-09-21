//! 规则与查询会话的数据模型。
//!
//! 所有“瞬间”统一用 UTC epoch 秒表示；只有界面展示时才转换回本地墙钟时间。

use serde::{Deserialize, Serialize};

/// 不存在的本地墙钟时间（夏令时跳空 gap）的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapPolicy {
    /// 跳过整个候选。
    Skip,
    /// 平移到跳空结束后的下一合法瞬间（本地侧）。
    ShiftForward,
}

/// 重复出现的本地墙钟时间（回拨 fold）的选择策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FoldPolicy {
    /// 只取偏移较大的早侧（DST 进行中的那一次）。
    Early,
    /// 只取偏移较小的晚侧（回到标准时后的那一次）。
    Late,
    /// 两次都保留为独立瞬间。
    Both,
}

/// 月内日期：可为正数（从 1 开始）或负数（-1 表示最后一天）。
pub type DayOfMonth = i32;

/// ISO 周内日期，1=周一 … 7=周日。
pub type DayOfWeekIso = u32;

/// 月内日期包含规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonthlyRule {
    pub id: String,
    pub days_of_month: Vec<DayOfMonth>,
    /// 本地墙钟时间：[时, 分, 秒]。
    pub hms: [u32; 3],
}

/// 周内日期包含规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeeklyRule {
    pub id: String,
    pub days_of_week: Vec<DayOfWeekIso>,
    pub hms: [u32; 3],
}

/// 节假日包含规则；日期引用节假日集合中的 key。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolidayRule {
    pub id: String,
    pub holidays: Vec<String>,
    pub hms: [u32; 3],
}

/// 三类包含规则之一。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InclusionRule {
    Monthly(MonthlyRule),
    Weekly(WeeklyRule),
    Holiday(HolidayRule),
}

impl InclusionRule {
    pub fn id(&self) -> &str {
        match self {
            InclusionRule::Monthly(r) => &r.id,
            InclusionRule::Weekly(r) => &r.id,
            InclusionRule::Holiday(r) => &r.id,
        }
    }
    pub fn hms(&self) -> [u32; 3] {
        match self {
            InclusionRule::Monthly(r) => r.hms,
            InclusionRule::Weekly(r) => r.hms,
            InclusionRule::Holiday(r) => r.hms,
        }
    }
}

/// 排除窗：闭区间 [start_epoch, end_epoch]，端点按 UTC 瞬间比较。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExclusionWindow {
    pub id: String,
    /// 包含的 UTC epoch 秒。
    pub start_epoch: i64,
    /// 包含的 UTC epoch 秒。
    pub end_epoch: i64,
    #[serde(default)]
    pub note: String,
}

/// 可版本化的周期规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleVersion {
    pub rule_id: String,
    /// 人类可读的版本号；规则指纹由全部字段决定，版本号仅用于展示。
    pub version: u32,
    /// IANA 时区名，如 `America/New_York`、`Asia/Tokyo`。
    pub timezone: String,
    /// 闭区间起点（UTC epoch 秒，包含）。
    pub start_epoch: i64,
    /// 闭区间终点（UTC epoch 秒，包含）。
    pub end_epoch: i64,
    /// 节假日集合：key -> 本地日期数组（"YYYY-MM-DD"，按规则时区解释）。
    #[serde(default)]
    pub holidays: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub inclusions: Vec<InclusionRule>,
    #[serde(default)]
    pub exclusions: Vec<ExclusionWindow>,
    /// 被接受触发点之间的最小间隔秒数（>0 时生效）。
    #[serde(default)]
    pub min_gap_seconds: i64,
    pub gap_policy: GapPolicy,
    pub fold_policy: FoldPolicy,
}

/// 查询方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Forward,
    Backward,
}
