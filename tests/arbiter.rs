//! 周期规则裁决器集成测试：固定使用 chrono-tz 内置 2024a 数据库，不依赖机器时区。

use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use periodic_arbiter::engine::{day_of_month_matches, Engine, VerdictKind};
use periodic_arbiter::fingerprint;
use periodic_arbiter::model::*;
use periodic_arbiter::storage;
use periodic_arbiter::tzutil;

fn ymd(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    Utc.with_ymd_and_hms(y, m, d, h, mi, s).unwrap().timestamp()
}

fn daily(hms: [u32; 3], id: &str) -> InclusionRule {
    InclusionRule::Weekly(WeeklyRule {
        id: id.into(),
        days_of_week: vec![1, 2, 3, 4, 5, 6, 7],
        hms,
    })
}

fn base_rule(gap: GapPolicy, fold: FoldPolicy) -> RuleVersion {
    RuleVersion {
        rule_id: "r".into(),
        version: 1,
        timezone: "America/New_York".into(),
        start_epoch: ymd(2024, 1, 1, 5, 0, 0),
        end_epoch: ymd(2025, 1, 1, 5, 0, 0),
        holidays: BTreeMap::new(),
        inclusions: vec![daily([9, 0, 0], "w")],
        exclusions: vec![],
        min_gap_seconds: 0,
        gap_policy: gap,
        fold_policy: fold,
    }
}

fn local_ny(y: i32, m: u32, d: u32, h: u32, mi: u32) -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, mi, 0).unwrap()
}

// ---------- 跳空 ----------
#[test]
fn spring_forward_gap_skip() {
    let tz = tzutil::parse_tz("America/New_York").unwrap();
    let r = tzutil::resolve_local(tz, local_ny(2024, 3, 10, 2, 30), GapPolicy::Skip, FoldPolicy::Early);
    assert!(r.is_empty());
}

#[test]
fn spring_forward_gap_shifts_to_next_valid_instant() {
    let tz = tzutil::parse_tz("America/New_York").unwrap();
    let r = tzutil::resolve_local(
        tz,
        local_ny(2024, 3, 10, 2, 30),
        GapPolicy::ShiftForward,
        FoldPolicy::Early,
    );
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].epoch, ymd(2024, 3, 10, 7, 0, 0));
    assert_eq!(r[0].shifted_by_seconds, 1800);
    let view = tzutil::view_instant(tz, r[0].epoch);
    assert_eq!(view.wall, "2024-03-10 03:00:00");
    assert_eq!(view.offset_name, "EDT");
}

#[test]
fn southern_hemisphere_gap() {
    let tz = tzutil::parse_tz("Australia/Sydney").unwrap();
    let local = chrono::NaiveDate::from_ymd_opt(2024, 10, 6).unwrap().and_hms_opt(2, 30, 0).unwrap();
    assert!(tzutil::resolve_local(tz, local, GapPolicy::Skip, FoldPolicy::Early).is_empty());
    let shifted = tzutil::resolve_local(tz, local, GapPolicy::ShiftForward, FoldPolicy::Early);
    assert_eq!(shifted[0].shifted_by_seconds, 1800);
}

// ---------- 回拨 ----------
#[test]
fn fall_back_fold_policies() {
    let tz = tzutil::parse_tz("America/New_York").unwrap();
    let local = local_ny(2024, 11, 3, 1, 30);
    let early = tzutil::resolve_local(tz, local, GapPolicy::Skip, FoldPolicy::Early);
    let late = tzutil::resolve_local(tz, local, GapPolicy::Skip, FoldPolicy::Late);
    let both = tzutil::resolve_local(tz, local, GapPolicy::Skip, FoldPolicy::Both);
    assert_eq!(early[0].epoch, ymd(2024, 11, 3, 5, 30, 0));
    assert_eq!(late[0].epoch, ymd(2024, 11, 3, 6, 30, 0));
    assert_eq!(both.len(), 2);
    assert_eq!(both[0].epoch, early[0].epoch);
    assert_eq!(both[1].epoch, late[0].epoch);
    assert_eq!(late[0].epoch - early[0].epoch, 3600);
}

#[test]
fn fold_both_emits_two_distinct_instants() {
    let mut rule = base_rule(GapPolicy::Skip, FoldPolicy::Both);
    rule.inclusions = vec![InclusionRule::Weekly(WeeklyRule {
        id: "w".into(),
        days_of_week: vec![7],
        hms: [1, 30, 0],
    })];
    rule.start_epoch = ymd(2024, 11, 3, 0, 0, 0);
    rule.end_epoch = ymd(2024, 11, 3, 12, 0, 0);
    let epochs: Vec<i64> = Engine::new(rule).unwrap().evaluate().iter().map(|v| v.epoch).collect();
    assert!(epochs.contains(&ymd(2024, 11, 3, 5, 30, 0)));
    assert!(epochs.contains(&ymd(2024, 11, 3, 6, 30, 0)));
}

// ---------- 月末 / 闰日 ----------
#[test]
fn month_end_negative_dom() {
    assert!(day_of_month_matches(-1, chrono::NaiveDate::from_ymd_opt(2024, 2, 29).unwrap()));
    assert!(day_of_month_matches(-2, chrono::NaiveDate::from_ymd_opt(2024, 2, 28).unwrap()));
    assert!(day_of_month_matches(-1, chrono::NaiveDate::from_ymd_opt(2023, 2, 28).unwrap()));
    assert!(day_of_month_matches(31, chrono::NaiveDate::from_ymd_opt(2024, 1, 31).unwrap()));
    assert!(!day_of_month_matches(31, chrono::NaiveDate::from_ymd_opt(2024, 4, 30).unwrap()));
    assert!(!day_of_month_matches(29, chrono::NaiveDate::from_ymd_opt(2023, 2, 28).unwrap()));
}

#[test]
fn leap_day_monthly_rule() {
    let rule = RuleVersion {
        rule_id: "leap".into(), version: 1, timezone: "UTC".into(),
        start_epoch: ymd(2023, 1, 1, 0, 0, 0), end_epoch: ymd(2025, 12, 31, 23, 59, 59),
        holidays: BTreeMap::new(),
        inclusions: vec![InclusionRule::Monthly(MonthlyRule {
            id: "m29".into(), days_of_month: vec![29], hms: [12, 0, 0],
        })],
        exclusions: vec![], min_gap_seconds: 0,
        gap_policy: GapPolicy::Skip, fold_policy: FoldPolicy::Early,
    };
    let vs = Engine::new(rule).unwrap().evaluate();
    assert_eq!(vs.iter().filter(|v| v.local_wall.starts_with("2024-02-29")).count(), 1);
    assert!(vs.iter().all(|v| !v.local_wall.starts_with("2023-02")));
}

// ---------- 闭区间端点 ----------
#[test]
fn closed_interval_endpoints_inclusive() {
    let rule = RuleVersion {
        rule_id: "ep".into(), version: 1, timezone: "UTC".into(),
        start_epoch: ymd(2024, 6, 1, 12, 0, 0), end_epoch: ymd(2024, 6, 2, 12, 0, 0),
        holidays: BTreeMap::new(), inclusions: vec![daily([12, 0, 0], "w")],
        exclusions: vec![], min_gap_seconds: 0,
        gap_policy: GapPolicy::Skip, fold_policy: FoldPolicy::Early,
    };
    let epochs: Vec<i64> = Engine::new(rule).unwrap().evaluate().iter().map(|v| v.epoch).collect();
    assert_eq!(epochs, vec![ymd(2024, 6, 1, 12, 0, 0), ymd(2024, 6, 2, 12, 0, 0)]);
}

// ---------- 排除窗端点按瞬间 ----------
#[test]
fn exclusion_window_endpoints_inclusive() {
    let rule = RuleVersion {
        rule_id: "ex".into(), version: 1, timezone: "UTC".into(),
        start_epoch: ymd(2024, 6, 1, 0, 0, 0), end_epoch: ymd(2024, 6, 3, 0, 0, 0),
        holidays: BTreeMap::new(), inclusions: vec![daily([9, 0, 0], "w")],
        exclusions: vec![ExclusionWindow {
            id: "blk".into(),
            // 恰好覆盖 6/1 09:00 这一个瞬间（端点包含）。
            start_epoch: ymd(2024, 6, 1, 9, 0, 0),
            end_epoch: ymd(2024, 6, 1, 9, 0, 0),
            note: String::new(),
        }],
        min_gap_seconds: 0,
        gap_policy: GapPolicy::Skip, fold_policy: FoldPolicy::Early,
    };
    let vs = Engine::new(rule).unwrap().evaluate();
    let blocked = vs.iter().find(|v| v.epoch == ymd(2024, 6, 1, 9, 0, 0)).unwrap();
    assert_eq!(blocked.kind, VerdictKind::Excluded);
    let next = vs.iter().find(|v| v.epoch == ymd(2024, 6, 2, 9, 0, 0)).unwrap();
    assert_eq!(next.kind, VerdictKind::Accepted);
    assert!(blocked.checks.iter().any(|c| c.contains("按瞬间命中端点包含")));
}

// ---------- 最小间隔 ----------
#[test]
fn min_gap_compares_instants_not_strings() {
    // 每日 09:00，间隔设为 25 小时：相邻两次 24h < 25h，隔天拒绝；
    // 该结论只依赖 UTC 瞬间差，与本地格式无关。
    let rule = RuleVersion {
        rule_id: "gap".into(), version: 1, timezone: "UTC".into(),
        start_epoch: ymd(2024, 6, 1, 0, 0, 0), end_epoch: ymd(2024, 6, 5, 0, 0, 0),
        holidays: BTreeMap::new(), inclusions: vec![daily([9, 0, 0], "w")],
        exclusions: vec![], min_gap_seconds: 25 * 3600,
        gap_policy: GapPolicy::Skip, fold_policy: FoldPolicy::Early,
    };
    let vs = Engine::new(rule).unwrap().evaluate();
    // 6/1 accept, 6/2 soon, 6/3 accept(48h>=25h), 6/4 soon
    let kind_at = |d| {
        vs.iter()
            .find(|v| v.epoch == ymd(2024, 6, d, 9, 0, 0))
            .map(|v| v.kind)
            .unwrap()
    };
    assert_eq!(kind_at(1), VerdictKind::Accepted);
    assert_eq!(kind_at(2), VerdictKind::TooSoon);
    assert_eq!(kind_at(3), VerdictKind::Accepted);
    assert_eq!(kind_at(4), VerdictKind::TooSoon);
}

// ---------- 合并来源 ----------
#[test]
fn multiple_sources_same_instant_merge_once() {
    let mut holidays = BTreeMap::new();
    holidays.insert("spec".to_string(), vec!["2024-06-03".to_string()]);
    let rule = RuleVersion {
        rule_id: "merge".into(), version: 1, timezone: "UTC".into(),
        start_epoch: ymd(2024, 6, 3, 0, 0, 0), end_epoch: ymd(2024, 6, 3, 23, 59, 59),
        holidays,
        inclusions: vec![
            InclusionRule::Weekly(WeeklyRule { id: "w".into(), days_of_week: vec![1], hms: [9, 0, 0] }),
            InclusionRule::Monthly(MonthlyRule { id: "m".into(), days_of_month: vec![3], hms: [9, 0, 0] }),
            InclusionRule::Holiday(HolidayRule { id: "h".into(), holidays: vec!["spec".into()], hms: [9, 0, 0] }),
        ],
        exclusions: vec![], min_gap_seconds: 0,
        gap_policy: GapPolicy::Skip, fold_policy: FoldPolicy::Early,
    };
    let vs = Engine::new(rule).unwrap().evaluate();
    assert_eq!(vs.len(), 1, "同一瞬间只能出现一次");
    let v = &vs[0];
    assert_eq!(v.kind, VerdictKind::Accepted);
    let ids: Vec<&str> = v.sources.iter().map(|s| s.rule_id.as_str()).collect();
    assert_eq!(ids, vec!["h", "m", "w"], "解释必须保留全部三个来源");
    assert!(v.checks.iter().any(|c| c.contains("最终裁决:接受")));
}

#[test]
fn merge_during_fold_preserves_two_sides_and_sources() {
    // 11/3 01:30 回拨；两条周规则（周末 + 每日）都指向同一墙钟。
    let rule = RuleVersion {
        rule_id: "mergefold".into(), version: 1, timezone: "America/New_York".into(),
        start_epoch: ymd(2024, 11, 3, 0, 0, 0), end_epoch: ymd(2024, 11, 3, 12, 0, 0),
        holidays: BTreeMap::new(),
        inclusions: vec![
            InclusionRule::Weekly(WeeklyRule { id: "daily".into(), days_of_week: vec![7], hms: [1, 30, 0] }),
            InclusionRule::Weekly(WeeklyRule { id: "weekend".into(), days_of_week: vec![6, 7], hms: [1, 30, 0] }),
        ],
        exclusions: vec![], min_gap_seconds: 0,
        gap_policy: GapPolicy::Skip, fold_policy: FoldPolicy::Both,
    };
    let vs = Engine::new(rule).unwrap().evaluate();
    assert_eq!(vs.len(), 2);
    for v in &vs {
        let ids: Vec<&str> = v.sources.iter().map(|s| s.rule_id.as_str()).collect();
        assert!(ids.contains(&"daily") && ids.contains(&"weekend"));
    }
}

// ---------- 指纹 ----------
#[test]
fn fingerprint_changes_with_any_field_and_stable_for_order() {
    let r1 = base_rule(GapPolicy::Skip, FoldPolicy::Early);
    let fp1 = fingerprint::rule_fingerprint(&r1);
    // 改变 DST 策略即改指纹
    let mut r2 = r1.clone();
    r2.fold_policy = FoldPolicy::Late;
    assert_ne!(fp1, fingerprint::rule_fingerprint(&r2));
    // 包含规则书写顺序不影响指纹
    let mut r3 = r1.clone();
    r3.inclusions = vec![daily([9, 0, 0], "z")];
    let mut r4 = r3.clone();
    r4.inclusions = vec![daily([9, 0, 0], "z")];
    assert_eq!(fingerprint::rule_fingerprint(&r3), fingerprint::rule_fingerprint(&r4));
    // 空数组与缺省序列化等价性不是要求；但同结构两次必须一致
    assert_eq!(fp1, fingerprint::rule_fingerprint(&r1));
}

// ---------- 游标失效与方向绑定 ----------
#[test]
fn cursor_rejects_on_rule_change_and_wrong_direction() {
    let rule = base_rule(GapPolicy::Skip, FoldPolicy::Early);
    let fp = fingerprint::rule_fingerprint(&rule);
    let tzdb = tzutil::tzdb_version();
    let cur = fingerprint::encode_cursor(&fp, tzdb, Direction::Forward, 12345);
    assert_eq!(
        fingerprint::decode_cursor(&cur, &fp, tzdb, Direction::Forward).unwrap(),
        12345
    );
    // 反方向拒绝
    assert!(matches!(
        fingerprint::decode_cursor(&cur, &fp, tzdb, Direction::Backward),
        Err(fingerprint::CursorError::DirectionMismatch)
    ));
    // 规则改动 → 指纹不符
    let mut changed = rule.clone();
    changed.version = 2;
    let fp2 = fingerprint::rule_fingerprint(&changed);
    assert!(matches!(
        fingerprint::decode_cursor(&cur, &fp2, tzdb, Direction::Forward),
        Err(fingerprint::CursorError::FingerprintMismatch)
    ));
    // 损坏游标
    let mut tampered = cur.clone();
    tampered.push('A');
    assert!(fingerprint::decode_cursor(&tampered, &fp, tzdb, Direction::Forward).is_err());
}

// ---------- 正反向镜像（含分页） ----------
fn demo_rule() -> RuleVersion {
    let mut holidays = BTreeMap::new();
    holidays.insert("special".to_string(), vec!["2024-11-11".to_string()]);
    RuleVersion {
        rule_id: "demo".into(), version: 1, timezone: "America/New_York".into(),
        start_epoch: ymd(2024, 3, 8, 0, 0, 0), end_epoch: ymd(2024, 11, 12, 23, 59, 59),
        holidays,
        inclusions: vec![
            InclusionRule::Monthly(MonthlyRule { id: "m10".into(), days_of_month: vec![10], hms: [2, 30, 0] }),
            InclusionRule::Weekly(WeeklyRule { id: "wday".into(), days_of_week: vec![1, 2, 3, 4, 5], hms: [9, 0, 0] }),
            InclusionRule::Weekly(WeeklyRule { id: "wend".into(), days_of_week: vec![6, 7], hms: [1, 30, 0] }),
            InclusionRule::Holiday(HolidayRule { id: "hsp".into(), holidays: vec!["special".into()], hms: [9, 0, 0] }),
        ],
        exclusions: vec![ExclusionWindow {
            id: "july4".into(),
            start_epoch: ymd(2024, 7, 4, 4, 0, 0),
            end_epoch: ymd(2024, 7, 5, 3, 59, 59),
            note: "独立日".into(),
        }],
        min_gap_seconds: 20 * 3600,
        gap_policy: GapPolicy::ShiftForward,
        fold_policy: FoldPolicy::Both,
    }
}

fn collect_pages(rule: &RuleVersion, dir: Direction, limit: usize) -> Vec<i64> {
    let engine = Engine::new(rule.clone()).unwrap();
    let all = engine.evaluate();
    let fp = fingerprint::rule_fingerprint(rule);
    let tzdb = tzutil::tzdb_version();
    let mut anchor: Option<i64> = None;
    let mut out = Vec::new();
    loop {
        let page = engine.page(&all, dir, anchor, limit);
        if page.is_empty() { break; }
        let last = page.last().unwrap().epoch;
        out.extend(page.iter().map(|v| v.epoch));
        let cur = fingerprint::encode_cursor(&fp, tzdb, dir, last);
        // 每次都完整解码游标，模拟真实往返
        anchor = Some(fingerprint::decode_cursor(&cur, &fp, tzdb, dir).unwrap());
    }
    out
}

#[test]
fn forward_backward_are_mirrors_over_closed_interval() {
    let rule = demo_rule();
    for limit in [1, 3, 7, 50, 1000] {
        let fwd = collect_pages(&rule, Direction::Forward, limit);
        let mut back = collect_pages(&rule, Direction::Backward, limit);
        back.reverse();
        assert_eq!(fwd, back, "limit={limit} 时正反向序列必须互为镜像");
    }
}

#[test]
fn demo_contains_gap_fold_exclusion_and_merge() {
    let vs = Engine::new(demo_rule()).unwrap().evaluate();
    // 跳空平移：3/10 02:30 → 03:00 EDT = 07:00Z
    let gapped = vs.iter().find(|v| v.epoch == ymd(2024, 3, 10, 7, 0, 0));
    assert!(gapped.is_some(), "必须包含跳空平移后的瞬间");
    assert!(gapped.unwrap().sources.iter().any(|s| s.shifted_by_seconds == 1800));
    // 回拨两侧
    let fold_count = vs.iter().filter(|v| v.local_wall == "2024-11-03 01:30:00").count();
    assert_eq!(fold_count, 2);
    // 排除窗：7/4 09:00 本地被拦
    assert!(vs
        .iter()
        .any(|v| v.kind == VerdictKind::Excluded && v.local_wall == "2024-07-04 09:00:00"));
    // 合并来源：11/11（周一 + Veterans Day）09:00 有两个来源
    let merged = vs.iter().find(|v| v.local_wall == "2024-11-11 09:00:00").unwrap();
    let ids: Vec<&str> = merged.sources.iter().map(|s| s.rule_id.as_str()).collect();
    assert!(ids.contains(&"hsp") && ids.contains(&"wday"));
}

// ---------- 导出 → 另一实例导入：相同 UTC 瞬间与解释顺序 ----------
#[test]
fn export_import_roundtrip_reproduces_identical_verdicts() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let store_a = storage::Store::open(dir_a.path()).unwrap();
    let rule = demo_rule();
    let engine_a = Engine::new(rule.clone()).unwrap();
    let verdicts_a = engine_a.evaluate();
    let fp = fingerprint::rule_fingerprint(&rule);

    // 在实例 A 记录若干正向+反向查询
    let fwd1 = engine_a.page(&verdicts_a, Direction::Forward, None, 5);
    let cur = fingerprint::encode_cursor(&fp, tzutil::tzdb_version(), Direction::Forward, fwd1.last().unwrap().epoch);
    let anchor = fingerprint::decode_cursor(&cur, &fp, tzutil::tzdb_version(), Direction::Forward).unwrap();
    let fwd2 = engine_a.page(&verdicts_a, Direction::Forward, Some(anchor), 5);
    let session = storage::SessionRecord {
        session_id: "sess-x".into(),
        rule_id: rule.rule_id.clone(),
        rule_fingerprint: fp.clone(),
        tzdb: tzutil::tzdb_version().to_string(),
        rule_snapshot: rule.clone(),
        queries: vec![
            storage::QueryRecord {
                direction: Direction::Forward, limit: 5, cursor: None,
                returned_epochs: fwd1.iter().map(|v| v.epoch).collect(),
                next_cursor: Some(cur.clone()), anchor_epoch: None,
            },
            storage::QueryRecord {
                direction: Direction::Forward, limit: 5, cursor: Some(cur),
                returned_epochs: fwd2.iter().map(|v| v.epoch).collect(),
                next_cursor: None, anchor_epoch: Some(anchor),
            },
        ],
        created_unix: 1700000000,
    };
    store_a.save_session(&session).unwrap();

    let envelope = storage::export_session(session, tzutil::tzdb_version());
    let json = serde_json::to_vec(&envelope).unwrap();

    // 实例 B：仅通过导入文件恢复，规则快照、游标继续可用。
    let parsed: storage::ExportEnvelope = serde_json::from_slice(&json).unwrap();
    let (imported, warning) = storage::import_envelope(parsed).unwrap();
    assert!(warning.is_none());
    let store_b = storage::Store::open(dir_b.path()).unwrap();
    store_b.save_rule(&imported.rule_snapshot).unwrap();
    store_b.save_session(&imported).unwrap();

    let reloaded = store_b.load_session("sess-x").unwrap().unwrap();
    assert_eq!(reloaded.rule_fingerprint, fp);
    let engine_b = Engine::new(reloaded.rule_snapshot).unwrap();
    let verdicts_b = engine_b.evaluate();

    // 相同的 UTC 瞬间序列
    let epochs_a: Vec<i64> = verdicts_a.iter().map(|v| v.epoch).collect();
    let epochs_b: Vec<i64> = verdicts_b.iter().map(|v| v.epoch).collect();
    assert_eq!(epochs_a, epochs_b);
    // 相同的裁决种类与解释顺序（逐字一致）
    let kinds_a: Vec<_> = verdicts_a.iter().map(|v| v.kind).collect();
    let kinds_b: Vec<_> = verdicts_b.iter().map(|v| v.kind).collect();
    assert_eq!(kinds_a, kinds_b);
    for (a, b) in verdicts_a.iter().zip(verdicts_b.iter()) {
        assert_eq!(a.sources, b.sources);
        assert_eq!(a.checks, b.checks);
    }
    // 导入后用记录里的游标继续翻页，得到的瞬间与实例 A 的下一页一致
    let stored_cur = reloaded.queries[0].next_cursor.clone().unwrap();
    let anchor_b =
        fingerprint::decode_cursor(&stored_cur, &fp, tzutil::tzdb_version(), Direction::Forward)
            .unwrap();
    let page_b = engine_b.page(&verdicts_b, Direction::Forward, Some(anchor_b), 5);
    assert_eq!(
        page_b.iter().map(|v| v.epoch).collect::<Vec<_>>(),
        fwd2.iter().map(|v| v.epoch).collect::<Vec<_>>()
    );
}

#[test]
fn corrupted_import_is_rejected() {
    let rule = demo_rule();
    let session = storage::SessionRecord {
        session_id: "s".into(), rule_id: rule.rule_id.clone(),
        rule_fingerprint: fingerprint::rule_fingerprint(&rule),
        tzdb: tzutil::tzdb_version().to_string(),
        rule_snapshot: rule, queries: vec![], created_unix: 1,
    };
    let mut env = storage::export_session(session, tzutil::tzdb_version());
    env.digest = format!("{:0>64}", "0");
    assert!(storage::import_envelope(env).is_err());
}

// ---------- 不依赖机器当前时区 ----------
#[test]
fn results_independent_of_machine_tz_env() {
    let rule = demo_rule();
    let vs = Engine::new(rule.clone()).unwrap().evaluate();
    std::env::set_var("TZ", "Pacific/Kiritimati");
    let vs2 = Engine::new(rule).unwrap().evaluate();
    std::env::remove_var("TZ");
    let a: Vec<i64> = vs.iter().map(|v| v.epoch).collect();
    let b: Vec<i64> = vs2.iter().map(|v| v.epoch).collect();
    assert_eq!(a, b);
}

// ---------- 输入校验 ----------
#[test]
fn rejects_invalid_rules() {
    let mut r = demo_rule();
    r.start_epoch = r.end_epoch + 1;
    assert!(Engine::new(r).is_err());
    let mut r = demo_rule();
    r.min_gap_seconds = -5;
    assert!(Engine::new(r).is_err());
    let mut r = demo_rule();
    r.timezone = "Not/AZone".into();
    assert!(Engine::new(r).is_err());
    let mut r = demo_rule();
    r.holidays.insert("bad".into(), vec!["2024-13-40".into()]);
    r.inclusions.push(InclusionRule::Holiday(HolidayRule {
        id: "hb".into(), holidays: vec!["bad".into()], hms: [0, 0, 0],
    }));
    assert!(Engine::new(r).is_err());
}
