//! 周期规则裁决引擎。
//!
//! 在规则的完整闭区间 [start_epoch, end_epoch] 上一次性计算出按 UTC 瞬间排序的
//! 裁决序列。正向查询取其前缀切片、反向查询取后缀切片，因此二者在同一闭区间上
//! 天然互为镜像。

use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::{Datelike, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

use crate::model::{
    DayOfMonth, Direction, ExclusionWindow, InclusionRule, RuleVersion,
};
use crate::tzutil::{self, FoldSide, ResolvedInstant};

/// 产生某个候选瞬间的一条包含规则来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub rule_id: String,
    pub rule_kind: String,
    pub local_wall: String,
    pub side: &'static str,
    /// 跳空平移秒数（0 表示未平移）。
    pub shifted_by_seconds: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictKind {
    Accepted,
    Excluded,
    TooSoon,
}

/// 对一个 UTC 瞬间的裁决：接受、被排除窗拦截、或因最小间隔被拒绝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub epoch: i64,
    pub local_wall: String,
    pub offset_name: String,
    pub offset_seconds: i32,
    pub kind: VerdictKind,
    /// 产生该瞬间的所有包含规则来源（保留合并来源）。
    pub sources: Vec<Provenance>,
    /// 依次通过/触发的检查说明。
    pub checks: Vec<String>,
}

#[derive(Debug)]
pub struct Engine {
    rule: RuleVersion,
    tz: Tz,
}

/// 规则或输入校验错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for EngineError {}

fn err(msg: impl Into<String>) -> EngineError {
    EngineError(msg.into())
}

fn validate_hms(hms: [u32; 3]) -> bool {
    hms[0] < 24 && hms[1] < 60 && hms[2] < 60
}

impl Engine {
    pub fn new(rule: RuleVersion) -> Result<Self, EngineError> {
        if rule.start_epoch > rule.end_epoch {
            return Err(err("start_epoch 不能晚于 end_epoch（必须是闭区间）"));
        }
        if rule.min_gap_seconds < 0 {
            return Err(err("min_gap_seconds 不能为负"));
        }
        if rule.end_epoch - rule.start_epoch > 12_614_400_000 {
            return Err(err("闭区间跨度超过 400 年，请收窄时间窗"));
        }
        let tz = tzutil::parse_tz(&rule.timezone).map_err(err)?;
        let mut ids = HashSet::new();
        for inc in &rule.inclusions {
            if !ids.insert(inc.id()) {
                return Err(err(format!("包含规则 id 重复: {}", inc.id())));
            }
            if !validate_hms(inc.hms()) {
                return Err(err(format!("包含规则 {} 的 hms 非法", inc.id())));
            }
        }
        for (k, days) in &rule.holidays {
            for d in days {
                if NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err() {
                    return Err(err(format!("节假日集合 {k} 含非法日期 {d}")));
                }
            }
        }
        for ex in &rule.exclusions {
            if ex.start_epoch > ex.end_epoch {
                return Err(err(format!("排除窗 {} 起点晚于终点", ex.id)));
            }
        }
        for inc in &rule.inclusions {
            if let InclusionRule::Holiday(h) = inc {
                for name in &h.holidays {
                    if !rule.holidays.contains_key(name) {
                        return Err(err(format!("节假日规则 {} 引用了未定义集合 {name}", h.id)));
                    }
                }
            }
        }
        Ok(Engine { rule, tz })
    }

    pub fn rule(&self) -> &RuleVersion {
        &self.rule
    }

    fn date_of_epoch(&self, epoch: i64) -> NaiveDate {
        Utc.timestamp_opt(epoch, 0)
            .unwrap()
            .with_timezone(&self.tz)
            .date_naive()
    }

    /// 检查某本地日期是否命中某条包含规则。
    fn day_matches(&self, date: NaiveDate, inc: &InclusionRule) -> bool {
        match inc {
            InclusionRule::Monthly(m) => m
                .days_of_month
                .iter()
                .any(|d| day_of_month_matches(*d, date)),
            InclusionRule::Weekly(w) => {
                let iso = date.weekday().number_from_monday();
                w.days_of_week.contains(&iso)
            }
            InclusionRule::Holiday(h) => {
                let key = date.format("%Y-%m-%d").to_string();
                h.holidays
                    .iter()
                    .any(|set| self.rule.holidays.get(set).is_some_and(|v| v.contains(&key)))
            }
        }
    }

    fn emit(
        &self,
        groups: &mut BTreeMap<i64, Vec<Provenance>>,
        date: NaiveDate,
        inc: &InclusionRule,
        resolved: ResolvedInstant,
    ) {
        // 规则闭区间按瞬间裁剪。
        if resolved.epoch < self.rule.start_epoch || resolved.epoch > self.rule.end_epoch {
            return;
        }
        let side = match resolved.side {
            FoldSide::Single => "single",
            FoldSide::Early => "early",
            FoldSide::Late => "late",
        };
        groups.entry(resolved.epoch).or_default().push(Provenance {
            rule_id: inc.id().to_string(),
            rule_kind: match inc {
                InclusionRule::Monthly(_) => "monthly",
                InclusionRule::Weekly(_) => "weekly",
                InclusionRule::Holiday(_) => "holiday",
            }
            .to_string(),
            local_wall: format!("{} {}", date, fmt_hms(inc.hms())),
            side,
            shifted_by_seconds: resolved.shifted_by_seconds,
        });
    }

    /// 在完整闭区间上生成裁决序列（升序）。
    pub fn evaluate(&self) -> Vec<Verdict> {
        let first_date = self.date_of_epoch(self.rule.start_epoch);
        let last_date = self.date_of_epoch(self.rule.end_epoch);
        let mut groups: BTreeMap<i64, Vec<Provenance>> = BTreeMap::new();

        let mut date = first_date;
        // 安全上限：约 400 年，正常规则远小于此。
        let mut guard = 0i64;
        while date <= last_date && guard < 150_000 {
            guard += 1;
            for inc in &self.rule.inclusions {
                if !self.day_matches(date, inc) {
                    continue;
                }
                let hms = inc.hms();
                let local = date.and_time(
                    NaiveTime::from_hms_opt(hms[0], hms[1], hms[2]).unwrap(),
                );
                for resolved in tzutil::resolve_local(
                    self.tz,
                    local,
                    self.rule.gap_policy,
                    self.rule.fold_policy,
                ) {
                    self.emit(&mut groups, date, inc, resolved);
                }
            }
            date = date.succ_opt().unwrap();
        }

        let mut verdicts = Vec::with_capacity(groups.len());
        let mut last_accepted: Option<i64> = None;
        for (epoch, mut sources) in groups {
            // 来源稳定排序：按规则 id，再按侧。
            sources.sort_by(|a, b| {
                a.rule_id
                    .cmp(&b.rule_id)
                    .then_with(|| side_rank(a.side).cmp(&side_rank(b.side)))
            });
            sources.dedup_by(|a, b| {
                a.rule_id == b.rule_id
                    && a.side == b.side
                    && a.shifted_by_seconds == b.shifted_by_seconds
            });
            let view = tzutil::view_instant(self.tz, epoch);

            let mut checks: Vec<String> = Vec::new();
            for s in &sources {
                checks.push(format!("来源:包含规则 {}({}) 的本地时间 {}", s.rule_id, s.rule_kind, s.local_wall));
            }
            for s in &sources {
                if s.shifted_by_seconds > 0 {
                    checks.push(format!(
                        "DST 跳空:规则 {} 本地时间不存在，按 shift_forward 平移 {} 秒到下一合法瞬间",
                        s.rule_id, s.shifted_by_seconds
                    ));
                } else if s.side != "single" {
                    checks.push(format!(
                        "DST 回拨:规则 {} 选择{}侧（偏移 {}）",
                        s.rule_id,
                        if s.side == "early" { "早" } else { "晚" },
                        view.offset_name
                    ));
                }
            }

            let hit = self.find_exclusion(epoch);
            if let Some(ex) = hit {
                checks.push(format!(
                    "排除窗 {} [{}..{}](UTC epoch {}..{}) 按瞬间命中端点包含:拒绝",
                    ex.id,
                    fmt_epoch(self.tz, ex.start_epoch),
                    fmt_epoch(self.tz, ex.end_epoch),
                    ex.start_epoch,
                    ex.end_epoch
                ));
                verdicts.push(Verdict {
                    epoch,
                    local_wall: view.wall,
                    offset_name: view.offset_name,
                    offset_seconds: view.offset_seconds,
                    kind: VerdictKind::Excluded,
                    sources,
                    checks,
                });
                continue;
            }
            checks.push("排除窗检查:未命中任何闭区间排除窗:通过".to_string());

            if self.rule.min_gap_seconds > 0 {
                if let Some(prev) = last_accepted {
                    let delta = epoch - prev;
                    if delta < self.rule.min_gap_seconds {
                        checks.push(format!(
                            "最小间隔:距上一已接受瞬间 {delta} 秒 < {} 秒:拒绝",
                            self.rule.min_gap_seconds
                        ));
                        verdicts.push(Verdict {
                            epoch,
                            local_wall: view.wall,
                            offset_name: view.offset_name,
                            offset_seconds: view.offset_seconds,
                            kind: VerdictKind::TooSoon,
                            sources,
                            checks,
                        });
                        continue;
                    }
                    checks.push(format!(
                        "最小间隔:距上一已接受瞬间 {delta} 秒 ≥ {} 秒:通过",
                        self.rule.min_gap_seconds
                    ));
                } else {
                    checks.push("最小间隔:区间内尚无已接受瞬间:通过".to_string());
                }
            }

            checks.push("最终裁决:接受".to_string());
            last_accepted = Some(epoch);
            verdicts.push(Verdict {
                epoch,
                local_wall: view.wall,
                offset_name: view.offset_name,
                offset_seconds: view.offset_seconds,
                kind: VerdictKind::Accepted,
                sources,
                checks,
            });
        }
        verdicts
    }

    /// 按查询方向取页。返回 (本页, 下一页游标起点 epoch)。
    pub fn page(
        &self,
        verdicts: &[Verdict],
        direction: Direction,
        after_epoch: Option<i64>,
        limit: usize,
    ) -> Vec<Verdict> {
        match direction {
            Direction::Forward => {
                let mut iter = verdicts.iter().filter(|v| match after_epoch {
                    Some(c) => v.epoch > c,
                    None => true,
                });
                let mut out = Vec::new();
                for _ in 0..limit {
                    match iter.next() {
                        Some(v) => out.push(v.clone()),
                        None => break,
                    }
                }
                out
            }
            Direction::Backward => {
                let mut filtered: Vec<&Verdict> = verdicts
                    .iter()
                    .filter(|v| match after_epoch {
                        Some(c) => v.epoch < c,
                        None => true,
                    })
                    .collect();
                filtered.reverse();
                filtered.into_iter().take(limit).cloned().collect()
            }
        }
    }

    fn find_exclusion(&self, epoch: i64) -> Option<&ExclusionWindow> {
        self.rule
            .exclusions
            .iter()
            .find(|e| epoch >= e.start_epoch && epoch <= e.end_epoch)
    }
}

fn side_rank(side: &str) -> u8 {
    match side {
        "early" => 0,
        "single" => 1,
        "late" => 2,
        _ => 3,
    }
}

fn fmt_hms(hms: [u32; 3]) -> String {
    format!("{:02}:{:02}:{:02}", hms[0], hms[1], hms[2])
}

fn fmt_epoch(tz: Tz, epoch: i64) -> String {
    tzutil::view_instant(tz, epoch).wall
}

/// 月内日期匹配；`d` 为负数时表示从月末倒数（-1=最后一天）。
pub fn day_of_month_matches(d: DayOfMonth, date: NaiveDate) -> bool {
    let n = date.day() as i32;
    if d > 0 {
        n == d
    } else {
        let len = days_in_month(date) as i32;
        n == len + d + 1
    }
}

pub fn days_in_month(date: NaiveDate) -> u32 {
    let (y, m) = (date.year(), date.month());
    let next = if m == 12 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(y, m + 1, 1).unwrap()
    };
    next.pred_opt().unwrap().day()
}

/// 供外部查询/测试使用的辅助：解析所有相关时区名集合。
pub fn known_tz_list() -> Vec<&'static str> {
    let all: BTreeSet<&'static str> = chrono_tz::TZ_VARIANTS
        .iter()
        .map(|t| t.name())
        .collect();
    all.into_iter().collect()
}
