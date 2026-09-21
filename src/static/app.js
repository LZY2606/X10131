// 周期规则裁决器 —— 前端逻辑（无框架，原生 JS）
"use strict";

const $ = (id) => document.getElementById(id);
const WEEK_LABELS = ["", "周一", "周二", "周三", "周四", "周五", "周六", "周日"];

const state = {
  inclusions: [],
  holidays: [], // {key, dates: ["YYYY-MM-DD", ...]}
  exclusions: [], // {id, startEpoch, endEpoch, note}
  cursors: { forward: null, backward: null },
  sessions: { forward: "", backward: "" },
  lastFilter: "all",
};

// ---------- 通用 ----------
async function api(path, body) {
  const opts = body
    ? { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }
    : {};
  const resp = await fetch(path, opts);
  let data = null;
  try { data = await resp.json(); } catch (_) {}
  if (!resp.ok) {
    throw new Error((data && data.error) || `HTTP ${resp.status}`);
  }
  return data;
}

function msg(text, cls) {
  const el = $("message");
  el.textContent = text;
  el.className = "message " + (cls || "");
}

// 把 datetime-local 的值（可能缺秒）规范化为 YYYY-MM-DDTHH:MM:SS
function normLocalInput(value) {
  if (!value) return null;
  let v = value;
  if (v.length === 16) v += ":00";
  return v;
}

// 按当前规则的 DST 策略把一个本地时间解析为 epoch（跳空/回拨）。
async function resolveEpoch(localInput) {
  const local = normLocalInput(localInput);
  if (!local) throw new Error("本地时间为空");
  const data = await api("/api/resolve-local", {
    timezone: $("timezone").value.trim(),
    local,
    gap_policy: $("gap_policy").value,
    fold_policy: $("fold_policy").value,
  });
  if (!data.items.length) {
    throw new Error(`本地时间 ${local} 是夏令时跳空，按 skip 策略不存在对应瞬间`);
  }
  // 回拨 both 时边界取早侧；late 时自然为晚侧。
  const pick = data.items.find((x) => x.side === "single") || data.items[0];
  return { epoch: pick.epoch, all: data.items };
}

async function maybeEpoch(input) {
  const local = normLocalInput(input);
  if (!local) return null;
  return await resolveEpoch(local);
}

// ---------- 规则组装 ----------
function readInclusions() {
  const out = [];
  for (const node of state.inclusions) {
    const hms = [
      Number(node.el.querySelector(".hh").value),
      Number(node.el.querySelector(".mm").value),
      Number(node.el.querySelector(".ss").value),
    ];
    const id = node.el.querySelector(".inc-id").value.trim();
    if (!id) throw new Error("包含规则缺少 id");
    if (node.kind === "monthly") {
      const days = parseIntList(node.el.querySelector(".days").value);
      out.push({ kind: "monthly", id, days_of_month: days, hms });
    } else if (node.kind === "weekly") {
      const days = [...node.el.querySelectorAll(".dow:checked")].map((c) => Number(c.value));
      out.push({ kind: "weekly", id, days_of_week: days, hms });
    } else {
      const sets = [...node.el.querySelectorAll(".holset:checked")].map((c) => c.value);
      out.push({ kind: "holiday", id, holidays: sets, hms });
    }
  }
  return out;
}

function parseIntList(text) {
  return text.split(/[,，\s]+/).filter(Boolean).map((t) => {
    const n = Number(t);
    if (!Number.isInteger(n)) throw new Error(`非法整数: ${t}`);
    return n;
  });
}

function readHolidaysMap() {
  const map = {};
  for (const item of state.holidays) {
    const key = item.el.querySelector(".hkey").value.trim();
    if (!key) throw new Error("节假日集合缺少名称");
    const dates = item.el
      .querySelector(".dates-input").value
      .split(/[,，\s]+/).filter(Boolean);
    for (const d of dates) {
      if (!/^\d{4}-\d{2}-\d{2}$/.test(d)) throw new Error(`节假日日期格式应为 YYYY-MM-DD: ${d}`);
    }
    map[key] = dates;
  }
  return map;
}

async function readExclusions() {
  const out = [];
  for (const item of state.exclusions) {
    const id = item.el.querySelector(".ex-id").value.trim();
    if (!id) throw new Error("排除窗缺少 id");
    const s = await maybeEpoch(item.el.querySelector(".ex-start").value);
    const e = await maybeEpoch(item.el.querySelector(".ex-end").value);
    if (!s || !e) throw new Error(`排除窗 ${id} 的起止时间不能为空`);
    out.push({
      id,
      start_epoch: s.epoch,
      end_epoch: e.epoch,
      note: item.el.querySelector(".ex-note").value || "",
    });
  }
  return out;
}

async function buildRule() {
  const start = await resolveEpoch($("start_local").value);
  const end = await resolveEpoch($("end_local").value);
  if (end.epoch < start.epoch) throw new Error("结束边界不能早于起始边界");
  const rule = {
    rule_id: $("rule_id").value.trim() || "rule",
    version: Number($("version").value) || 0,
    timezone: $("timezone").value.trim(),
    start_epoch: start.epoch,
    end_epoch: end.epoch,
    holidays: readHolidaysMap(),
    inclusions: readInclusions(),
    exclusions: await readExclusions(),
    min_gap_seconds: Number($("min_gap_seconds").value) || 0,
    gap_policy: $("gap_policy").value,
    fold_policy: $("fold_policy").value,
  };
  return rule;
}

// ---------- 包含规则 UI ----------
function addInclusion(kind) {
  const tpl = $("tpl-inc");
  const node = tpl.content.firstElementChild.cloneNode(true);
  const kindLabel = { monthly: "月内日期", weekly: "周内日期", holiday: "节假日" }[kind];
  node.querySelector(".kind").textContent = kindLabel;
  node.querySelector(".inc-id").value =
    kind + "-" + (state.inclusions.filter((x) => x.kind === kind).length + 1);
  const body = node.querySelector(".inc-body");
  if (kind === "monthly") {
    body.innerHTML = `日期（1..31，逗号分隔；-1=月末，-2=倒数第二天）
      <input class="days" value="1,15,-1" />`;
  } else if (kind === "weekly") {
    body.innerHTML = `周内日期 <span class="taglist" id></span>`;
    const taglist = body.querySelector(".taglist");
    for (let d = 1; d <= 7; d++) {
      const lab = document.createElement("label");
      lab.className = "tag";
      lab.innerHTML = `<input type="checkbox" class="dow" value="${d}" ${d <= 5 ? "checked" : ""}/>${WEEK_LABELS[d]}`;
      taglist.appendChild(lab);
    }
  } else {
    body.innerHTML = `命中的节假日集合 <span class="taglist"></span>`;
    const taglist = body.querySelector(".taglist");
    const wrap = { taglist };
    node.dataset.holWrap = "1";
    refreshHolidayCheckboxes(node);
  }
  node.querySelector(".del").addEventListener("click", () => {
    node.remove();
    state.inclusions = state.inclusions.filter((x) => x.el !== node);
  });
  $("inclusions").appendChild(node);
  state.inclusions.push({ kind, el: node });
}

function refreshHolidayCheckboxes(scope) {
  const blocks = scope
    ? [scope]
    : state.inclusions.filter((x) => x.kind === "holiday").map((x) => x.el);
  for (const node of blocks) {
    const taglist = node.querySelector(".taglist");
    if (!taglist) continue;
    const previous = new Set([...taglist.querySelectorAll(".holset:checked")].map((c) => c.value));
    taglist.innerHTML = "";
    for (const item of state.holidays) {
      const key = item.el.querySelector(".hkey").value.trim();
      if (!key) continue;
      const lab = document.createElement("label");
      lab.className = "tag";
      lab.innerHTML = `<input type="checkbox" class="holset" value="${key}" ${
        previous.has(key) ? "checked" : ""
      }/>${key}`;
      taglist.appendChild(lab);
    }
  }
}

// ---------- 节假日集合 UI ----------
function addHolidaySet(key, dates) {
  const row = document.createElement("div");
  row.className = "holiday-row";
  row.innerHTML = `
    <div class="subhead">集合
      <input class="hkey" placeholder="如 us-holiday" />
      <button class="del">删除</button>
    </div>
    <input class="dates-input" placeholder="YYYY-MM-DD 逗号分隔，如 2024-11-28, 2024-12-25" />`;
  row.querySelector(".hkey").value = key || "";
  row.querySelector(".dates-input").value = (dates || []).join(", ");
  row.querySelector(".del").addEventListener("click", () => {
    row.remove();
    state.holidays = state.holidays.filter((x) => x.el !== row);
    refreshHolidayCheckboxes();
  });
  row.querySelector(".hkey").addEventListener("change", () => refreshHolidayCheckboxes());
  $("holidays").appendChild(row);
  state.holidays.push({ el: row });
  refreshHolidayCheckboxes();
}

// ---------- 排除窗 UI ----------
function addExclusion(id, startLocal, endLocal, note) {
  const row = document.createElement("div");
  row.className = "excl-row";
  row.innerHTML = `
    <div class="excl-grid">
      <label>窗 ID <input class="ex-id" /></label>
      <label>备注 <input class="ex-note" /></label>
      <button class="del">删除</button>
    </div>
    <div class="grid2">
      <label>开始（本地，含端点）<input type="datetime-local" step="1" class="ex-start" /></label>
      <label>结束（本地，含端点）<input type="datetime-local" step="1" class="ex-end" /></label>
    </div>`;
  row.querySelector(".ex-id").value = id || "ex-" + (state.exclusions.length + 1);
  row.querySelector(".ex-note").value = note || "";
  row.querySelector(".ex-start").value = startLocal || "";
  row.querySelector(".ex-end").value = endLocal || "";
  row.querySelector(".del").addEventListener("click", () => {
    row.remove();
    state.exclusions = state.exclusions.filter((x) => x.el !== row);
  });
  $("exclusions").appendChild(row);
  state.exclusions.push({ el: row });
}

// ---------- 时间轴渲染 ----------
const KIND_LABEL = { accepted: "接受", excluded: "排除窗拒绝", too_soon: "最小间隔拒绝" };

function formatUtc(epoch) {
  return new Date(epoch * 1000).toISOString().replace("T", " ").replace(".000Z", " UTC");
}

function renderTimeline(items) {
  state.lastItems = items;
  const tl = $("timeline");
  tl.innerHTML = "";
  const filter = state.lastFilter;
  const shown = items.filter((v) => filter === "all" || v.kind === filter);
  if (!shown.length) {
    tl.innerHTML = `<p class="hint">没有匹配的候选。</p>`;
    return;
  }
  for (const v of shown) {
    const item = document.createElement("div");
    item.className = "tl-item " + v.kind;
    const src = v.sources
      .map((s) => `${s.rule_id}(${s.rule_kind})@${s.local_wall}[${sideLabel(s)}]`)
      .join(" + ");
    const checks = v.checks
      .map((c) => {
        const cls = /拒绝|不存在/.test(c) ? "fail" : /通过|接受/.test(c) ? "pass" : "";
        return `<li class="${cls}"></li>`; // 文本用 textContent 设置，避免注入
      })
      .join("");
    item.innerHTML = `
      <div class="tl-card">
        <div class="tl-head">
          <span><span class="when"></span> <span class="utc"></span></span>
          <span class="badge ${v.kind}">${KIND_LABEL[v.kind]}</span>
        </div>
        <div class="tl-body">
          <div class="sources"></div>
          <div class="utc2 hint"></div>
          <ul class="checks">${checks}</ul>
        </div>
      </div>`;
    item.querySelector(".when").textContent = v.local_wall + " (" + v.offset_name + ")";
    item.querySelector(".utc").textContent = " = " + formatUtc(v.epoch) + " · epoch " + v.epoch;
    item.querySelector(".sources").textContent = "来源（合并）: " + src;
    item.querySelector(".utc2").textContent =
      `UTC 偏移 ${v.offset_seconds >= 0 ? "+" : ""}${v.offset_seconds} 秒`;
    const lis = item.querySelectorAll(".checks li");
    v.checks.forEach((c, i) => (lis[i].textContent = c));
    item.querySelector(".tl-head").addEventListener("click", () => item.classList.toggle("open"));
    tl.appendChild(item);
  }
}

function sideLabel(s) {
  if (s.side === "early") return "回拨早侧";
  if (s.side === "late") return "回拨晚侧";
  if (s.shifted_by_seconds > 0) return `跳空平移+${s.shifted_by_seconds}s`;
  return "唯一";
}

function renderPaging(resp) {
  const dir = resp.direction;
  const p = $("paging");
  p.innerHTML = "";
  const line1 = document.createElement("div");
  line1.innerHTML = `会话 <code></code> · 方向 <b></b> · 本页 <b></b> 条 · 是否还有 <b></b>`;
  const code = line1.querySelector("code");
  code.textContent = resp.session_id;
  line1.querySelector("b").textContent = dir;
  line1.querySelectorAll("b")[1].textContent = String(resp.items.length);
  line1.querySelectorAll("b")[2].textContent = resp.has_more ? "是" : "否";
  p.appendChild(line1);
  if (resp.anchor_epoch !== null && resp.anchor_epoch !== undefined) {
    const line2 = document.createElement("div");
    line2.textContent = "游标定位瞬间: " + formatUtc(resp.anchor_epoch) + "（旧游标在规则变更或方向不符时会被拒绝）";
    p.appendChild(line2);
  }
}

// ---------- 操作 ----------
async function doPreview() {
  msg("正在计算全闭区间裁决序列…", "");
  const rule = await buildRule();
  const resp = await api("/api/evaluate", rule);
  $("fingerprint").value = resp.rule_fingerprint;
  renderTimeline(resp.items);
  const counts = resp.items.reduce((acc, v) => ((acc[v.kind] = (acc[v.kind] || 0) + 1), acc), {});
  msg(
    `全量序列 ${resp.items.length} 个候选：接受 ${counts.accepted || 0} / 排除 ${
      counts.excluded || 0
    } / 间隔拒绝 ${counts.too_soon || 0}。点击各行展开理由。指纹 ${resp.rule_fingerprint.slice(0, 16)}…`,
    "ok"
  );
}

async function doQuery() {
  const rule = await buildRule();
  const dir = $("direction").value;
  const body = {
    rule,
    direction: dir,
    limit: Math.max(1, Number($("limit").value) || 8),
    cursor: state.cursors[dir],
    session_id: state.sessions[dir] || $("session_id").value.trim() || null,
  };
  msg("查询中…", "");
  try {
    const resp = await api("/api/query", body);
    state.cursors[dir] = resp.next_cursor;
    state.sessions[dir] = resp.session_id;
    $("fingerprint").value = resp.rule_fingerprint;
    renderTimeline(resp.items);
    renderPaging(resp);
    msg(
      resp.items.length
        ? `已返回 ${resp.items.length} 个瞬间；${resp.has_more ? "继续同方向翻页" : "已到闭区间尽头"}。`
        : "闭区间内没有更多候选。",
      "ok"
    );
  } catch (e) {
    // 游标/会话失效时清空本地状态，用户可直接再次点击从首页开始。
    state.cursors[dir] = null;
    msg(e.message, "err");
  }
}

// ---------- 规则回填 ----------
function epochToLocalInput(epoch, tz) {
  const parts = new Intl.DateTimeFormat("en-CA", {
    timeZone: tz, year: "numeric", month: "2-digit", day: "2-digit",
    hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false,
  }).formatToParts(new Date(epoch * 1000));
  const get = (t) => parts.find((p) => p.type === t).value;
  let h = get("hour");
  if (h === "24") h = "00";
  return `${get("year")}-${get("month")}-${get("day")}T${h}:${get("minute")}:${get("second")}`;
}

function clearLists() {
  $("inclusions").innerHTML = "";
  $("holidays").innerHTML = "";
  $("exclusions").innerHTML = "";
  state.inclusions = [];
  state.holidays = [];
  state.exclusions = [];
}

function populateFromRule(rule) {
  $("rule_id").value = rule.rule_id;
  $("version").value = rule.version;
  $("timezone").value = rule.timezone;
  $("gap_policy").value = rule.gap_policy;
  $("fold_policy").value = rule.fold_policy;
  $("min_gap_seconds").value = rule.min_gap_seconds;
  $("start_local").value = epochToLocalInput(rule.start_epoch, rule.timezone);
  $("end_local").value = epochToLocalInput(rule.end_epoch, rule.timezone);
  clearLists();
  for (const [key, dates] of Object.entries(rule.holidays || {})) {
    addHolidaySet(key, dates);
  }
  for (const inc of rule.inclusions || []) {
    addInclusion(inc.kind);
    const node = state.inclusions[state.inclusions.length - 1].el;
    node.querySelector(".inc-id").value = inc.id;
    node.querySelector(".hh").value = inc.hms[0];
    node.querySelector(".mm").value = inc.hms[1];
    node.querySelector(".ss").value = inc.hms[2];
    if (inc.kind === "monthly") node.querySelector(".days").value = inc.days_of_month.join(",");
    if (inc.kind === "weekly") {
      node.querySelectorAll(".dow").forEach((c) => (c.checked = inc.days_of_week.includes(Number(c.value))));
    }
    if (inc.kind === "holiday") {
      node.querySelectorAll(".holset").forEach((c) => (c.checked = inc.holidays.includes(c.value)));
    }
  }
  for (const ex of rule.exclusions || []) {
    addExclusion(
      ex.id,
      epochToLocalInput(ex.start_epoch, rule.timezone),
      epochToLocalInput(ex.end_epoch, rule.timezone),
      ex.note || ""
    );
  }
  invalidateCursors();
}

function invalidateCursors() {
  state.cursors = { forward: null, backward: null };
  state.sessions = { forward: "", backward: "" };
  $("paging").innerHTML = "";
}

// ---------- 已存规则 ----------
async function refreshSavedRules() {
  const rules = await api("/api/rules");
  const sel = $("saved_rules");
  sel.innerHTML = "";
  for (const r of rules) {
    const opt = document.createElement("option");
    opt.value = r.rule_id;
    opt.textContent = `${r.rule_id} v${r.version}`;
    sel.appendChild(opt);
  }
}

// ---------- 导出 / 导入 ----------
async function doExport() {
  const dir = $("direction").value;
  const sessionId = state.sessions[dir] || $("session_id").value.trim();
  if (!sessionId) {
    msg("当前还没有可导出的会话，请先查询。", "warn");
    return;
  }
  const env = await api("/api/export", { session_id: sessionId });
  const blob = new Blob([JSON.stringify(env, null, 2)], { type: "application/json" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = `${sessionId}.session.json`;
  a.click();
  URL.revokeObjectURL(a.href);
  msg(`会话 ${sessionId} 已导出（含规则快照、全部查询记录与摘要）。`, "ok");
}

async function doImport(file) {
  const text = await file.text();
  let env;
  try {
    env = JSON.parse(text);
  } catch (e) {
    msg("导入文件不是合法 JSON: " + e.message, "err");
    return;
  }
  try {
    const resp = await api("/api/import", env);
    await refreshSavedRules();
    populateFromRule(resp_rule_fix(resp, env));
    msg(
      `导入成功：会话 ${resp.session_id}，${resp.queries} 条查询记录，指纹 ${resp.rule_fingerprint.slice(0, 12)}…` +
        (resp.warning ? "\n注意：" + resp.warning : ""),
      resp.warning ? "warn" : "ok"
    );
  } catch (e) {
    msg("导入被拒绝: " + e.message, "err");
  }
}

function resp_rule_fix(resp, env) {
  return env.session.rule_snapshot;
}

// ---------- 演示规则 ----------
function loadDemo() {
  populateFromRule({
    rule_id: "payroll-us",
    version: 1,
    timezone: "America/New_York",
    start_epoch: 0,
    end_epoch: 0,
    holidays: { special: ["2024-07-04", "2024-11-11"] },
    inclusions: [
      { kind: "monthly", id: "m-10th", days_of_month: [10], hms: [2, 30, 0] },
      { kind: "weekly", id: "w-weekday", days_of_week: [1, 2, 3, 4, 5], hms: [9, 0, 0] },
      { kind: "weekly", id: "w-weekend", days_of_week: [6, 7], hms: [1, 30, 0] },
      { kind: "holiday", id: "h-special", holidays: ["special"], hms: [9, 0, 0] },
    ],
    exclusions: [
      { id: "july4", start_epoch: 0, end_epoch: 0, note: "美国独立日停摆" },
    ],
    min_gap_seconds: 20 * 3600,
    gap_policy: "shift_forward",
    fold_policy: "both",
  });
  $("start_local").value = "2024-03-08T00:00:00";
  $("end_local").value = "2024-11-12T23:59:59";
  const exRow = state.exclusions[0].el;
  exRow.querySelector(".ex-start").value = "2024-07-04T00:00:00";
  exRow.querySelector(".ex-end").value = "2024-07-04T23:59:59";
  msg(
    "演示规则已载入：覆盖 2024 纽约春令跳空（3/10 02:30 平移）、秋令回拨（11/3 01:30 早/晚两侧）、11/11 节假日与周规则合并来源、7/4 独立日排除窗、20 小时最小间隔；月内规则固定在每月 10 日。可直接“全量预览”。",
    "ok"
  );
}

// ---------- 初始化与事件 ----------
function wire() {
  document.querySelectorAll("[data-add]").forEach((btn) =>
    btn.addEventListener("click", () => {
      addInclusion(btn.dataset.add);
      invalidateCursors();
    })
  );
  document.querySelector("[data-add-holiday]").addEventListener("click", () => {
    addHolidaySet();
    invalidateCursors();
  });
  document.querySelector("[data-add-exclusion]").addEventListener("click", () => {
    addExclusion();
    invalidateCursors();
  });

  $("btn-preview").addEventListener("click", () =>
    doPreview().catch((e) => msg(e.message, "err"))
  );
  $("btn-query").addEventListener("click", () => doQuery());
  $("btn-reset").addEventListener("click", () => {
    invalidateCursors();
    $("session_id").value = "";
    msg("已清空游标，下次查询从闭区间首页开始。", "ok");
  });
  $("btn-save").addEventListener("click", async () => {
    try {
      const rule = await buildRule();
      const resp = await api("/api/rules", rule);
      $("fingerprint").value = resp.rule_fingerprint;
      await refreshSavedRules();
      msg("规则已保存到本地数据目录，指纹 " + resp.rule_fingerprint.slice(0, 16) + "…", "ok");
    } catch (e) {
      msg(e.message, "err");
    }
  });
  $("btn-demo").addEventListener("click", loadDemo);
  $("btn-export").addEventListener("click", () =>
    doExport().catch((e) => msg(e.message, "err"))
  );
  $("import_file").addEventListener("change", (e) => {
    if (e.target.files[0]) doImport(e.target.files[0]);
    e.target.value = "";
  });
  $("btn-refresh-saved").addEventListener("click", () =>
    refreshSavedRules().catch((e) => msg(e.message, "err"))
  );
  $("btn-load-saved").addEventListener("click", async () => {
    const id = $("saved_rules").value;
    if (!id) return msg("没有可载入的规则。", "warn");
    const rules = await api("/api/rules");
    const rule = rules.find((r) => r.rule_id === id);
    if (!rule) return msg("找不到规则 " + id, "err");
    populateFromRule(rule);
    $("fingerprint").value = "";
    msg(`已载入规则 ${id}，游标已失效，需要从首页重新查询。`, "ok");
  });

  // 任何规则字段变化都使旧游标失效（与服务端指纹校验形成双保险）。
  document
    .querySelector(".editor")
    .addEventListener("change", (ev) => {
      if (ev.target.closest(".mainbtns") || ev.target.id === "saved_rules") return;
      invalidateCursors();
    });

  // 时间轴过滤条
  const bar = document.createElement("div");
  bar.className = "filterbar";
  bar.innerHTML = `显示
    <select>
      <option value="all">全部候选</option>
      <option value="accepted">仅接受</option>
      <option value="excluded">仅排除窗拒绝</option>
      <option value="too_soon">仅间隔拒绝</option>
    </select>`;
  const sel = bar.querySelector("select");
  sel.addEventListener("change", () => {
    state.lastFilter = sel.value;
    const items = state.lastItems || [];
    renderTimeline(items);
  });
  $("timeline").before(bar);
}

async function init() {
  wire();
  try {
    const health = await api("/api/health");
    $("tzdb").textContent = "固定时区数据库: " + health.tzdb;
    const tzs = await api("/api/timezones");
    const dl = $("tzlist");
    for (const t of tzs) {
      const opt = document.createElement("option");
      opt.value = t;
      dl.appendChild(opt);
    }
    await refreshSavedRules();
  } catch (e) {
    msg("初始化失败: " + e.message, "err");
  }
  loadDemo();
}

init();
