import { STATES, dayKey, dateFromKey, shiftDay, dayRange, periodRange, duration, usageMetrics,
  aggregateApps, dayParts, ledgerGroups, distribution, deviceCoverage, clipRows,
  coalesceTimelineItems, activeSpans, quietSpans, buildTimelineScale } from "./report.mjs";
import { connectAccount } from "./account.mjs";

const invoke = (...args) => window.__TAURI__.core.invoke(...args);
const get = id => document.getElementById(id);
const views = [
  ["dashboard", "多设备仪表盘", "dashboard"],
  ["today", "今日复盘", "today"], ["ledger", "记录明细", "list"],
  ["summary", "趋势", "chart"], ["settings", "设置", "settings"]
];
const TIMELINE_SMOOTH_GAP_MS = 2000;
const TIMELINE_SCALE = { minQuietMs: 20 * 60e3, maxQuietSpans: 4, quietShare: .05, minQuietWeight: 5 * 60e3 };
const shapes = {
  dashboard:'<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 10h18m-7 0v10M7 14h3m-3 3h3"/>',
  today:'<rect x="3" y="5" width="18" height="16" rx="2"/><path d="M7 3v4m10-4v4M3 10h18m-13 5h3m3 0h2"/>',
  list:'<path d="M9 6h12M9 12h12M9 18h12M3 6h1m-1 6h1m-1 6h1"/>',
  chart:'<path d="M3 20h18M6 16v-5m6 5V5m6 11V8"/>',
  settings:'<path d="m9.6 3-.8 2.3-2 .9-2.2-.9-2 3.4 1.5 1.8v2.9l-1.5 1.8 2 3.4 2.2-.8 2 .9.8 2.3h4.8l.8-2.3 2-.9 2.2.8 2-3.4-1.5-1.8v-2.9l1.5-1.8-2-3.4-2.2.9-2-.9-.8-2.3z"/><circle cx="12" cy="12" r="3"/>',
  person:'<circle cx="12" cy="8" r="3.2"/><path d="M5 20v-2a5 5 0 0 1 5-4h4a5 5 0 0 1 5 4v2"/>',
  enter:'<path d="M12 4H4v16h8m-3-8h12m-4-4 4 4-4 4"/>',
  exit:'<path d="M11 4H4v16h7m-2-8h12m-4-4 4 4-4 4"/>',
  close:'<path d="m6 6 12 12M18 6 6 18"/>',
  prev:'<path d="m14 6-6 6 6 6"/>', next:'<path d="m10 6 6 6-6 6"/>',
  computer:'<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8m-4-4v4"/>',
  phone:'<rect x="7" y="2" width="10" height="20" rx="2"/><path d="M11 18h2"/>',
  tablet:'<rect x="4" y="2" width="16" height="20" rx="2"/><path d="M11 18h2"/>',
  lock:'<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/>',
  doc:'<path d="M6 2h8l4 4v16H6zm8 0v5h4M9 12h6m-6 4h6"/>',
  site:'<circle cx="12" cy="12" r="9"/><path d="M3 12h18m-9-9a16 16 0 0 1 0 18m0-18a16 16 0 0 0 0 18"/>',
  unknown:'<circle cx="12" cy="12" r="8" stroke-dasharray="2 4"/>',
  clock:'<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>',
  layers:'<path d="m12 3 8.5 4.5L12 12 3.5 7.5z"/><path d="M3.5 12 12 16.5 20.5 12M3.5 16.5 12 21l8.5-4.5"/>',
  devices:'<rect x="2" y="4" width="13" height="10" rx="2"/><path d="M6 18h5"/><rect x="17" y="9" width="5" height="10" rx="1.5"/>',
  target:'<circle cx="12" cy="12" r="8.5"/><circle cx="12" cy="12" r="3.5"/>',
  code:'<path d="m8 6-6 6 6 6m8-12 6 6-6 6m-2-15-4 18"/>',
  terminal:'<rect x="3" y="5" width="18" height="15" rx="2"/><path d="m7 9 3 3-3 3m6 0h4"/>',
  chat:'<path d="M20 15a3 3 0 0 1-3 3H9l-5 3v-6a3 3 0 0 1-1-2V7a3 3 0 0 1 3-3h11a3 3 0 0 1 3 3z"/><path d="M8 10h7m-7 4h4"/>'
};

const textTime = new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23" });
const shortDate = new Intl.DateTimeFormat("zh-CN", { month: "long", day: "numeric", weekday: "long" });
const today = () => dayKey(new Date());
const state = { view: "dashboard", date: today(), period: "week", filter: "all", deviceFilter: "", ledgerFilter: "", chartMode: "devices", fold: true,
  running: null, settings: null, account: null, browserSetup: null, ledgerOpen: new Set(), appsOpen: new Set(), rows: [], appRows: [],
  devices: [], rejectedSources: 0, generation: 0, loading: false, dayPart: null, ledgerApps: true, ledgerOrigin: "dashboard",
  dashboard: { focus: null, sourcesOpen: false },
  ledgerScope: null };
let noticeTimer;
let lastResult;

function el(tag, name, text) {
  const node = document.createElement(tag);
  if (name) node.className = name;
  if (text != null) node.textContent = text;
  return node;
}

function icon(name) {
  const holder = document.createElement("template");
  holder.innerHTML = '<svg class="icon" aria-hidden="true" viewBox="0 0 24 24">' + (shapes[name] || shapes.doc) + '</svg>';
  return holder.content.firstElementChild;
}

function withChildren(parent, ...children) { parent.append(...children.filter(Boolean)); return parent; }
function button(label, handler, css = "", glyph) {
  const control = el("button", css);
  control.type = "button";
  if (glyph) control.append(icon(glyph));
  if (label) control.append(document.createTextNode(label));
  control.addEventListener("click", handler);
  return control;
}
function empty(text) { return el("p", "empty", text); }
function panel(title, trailing) { return withChildren(el("section", "panel"), withChildren(el("div", "panel-title"), el("h2", null, title), trailing)); }

function message(text) {
  const node = get("notice");
  node.textContent = text;
  node.hidden = false;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => { node.hidden = true; }, 5000);
}

function renderRail() {
  const nav = get("nav");
  nav.replaceChildren(...views.filter(([id]) => id !== "settings" && id !== "ledger").map(([id, label, glyph]) => {
    const item = button(label, () => selectView(id), "nav-item", glyph);
    if (id === state.view) item.setAttribute("aria-current", "page");
    return item;
  }));
  if (state.view === "settings") get("account-settings").setAttribute("aria-current", "page");
  else get("account-settings").removeAttribute("aria-current");
  get("breadcrumbs").replaceChildren(document.createTextNode("Tickmark  /  "), el("strong", null, views.find(view => view[0] === state.view)[1]));
  renderStatus();
}
function renderStatus() {
  const collector = get("collector-status");
  if (collector) {
    const known = state.running !== null;
    collector.classList.toggle("on", state.running === true);
    collector.classList.toggle("warn", known && !state.running);
    get("collector-status-text").textContent = !known ? "检查中" : state.running ? "采集中" : "采集已停止";
    collector.title = !known ? "正在检查本机采集状态" : state.running ? "本机采集运行中，点击查看设置" : "本机采集已停止，点击开始采集";
    collector.setAttribute("aria-label", collector.title);
  }
  const hubCollector = document.querySelector("[data-hub-collector]");
  if (hubCollector) hubCollector.textContent = state.running ? "本机采集中" : "本机采集已停止";
  for (const label of document.querySelectorAll("[data-collector-status]")) {
    label.textContent = state.running ? "运行中" : "已停止";
  }
  for (const control of document.querySelectorAll("[data-capture-control]")) {
    control.textContent = state.running ? "停止采集" : "开始采集";
    control.classList.toggle("primary", !state.running);
  }
}
function selectView(view, hour = null, ledgerScope = null) {
  if (state.view === view) return;
  get("notice").hidden = true;
  if (view === "ledger") state.ledgerOrigin = state.view;
  state.view = view;
  state.dayPart = hour;
  state.ledgerScope = ledgerScope;
  state.rows = [];
  state.appRows = [];
  renderRail();
  get("page").scrollTop = 0;
  render();
  load();
}
function openRecordDetail({ date = state.date, filter = "all", apps = null, deviceId = state.deviceFilter, scope = null, hour = null } = {}) {
  state.date = date;
  state.filter = filter;
  state.ledgerApps = apps == null ? state.devices.some(device => device.hasAppRows) : apps;
  state.ledgerFilter = deviceId;
  selectView("ledger", hour, scope);
}
function selectDate(key) {
  if (!key || state.date === key) return;
  state.date = key;
  state.dashboard.focus = null;
  state.dayPart = null;
  state.ledgerScope = null;
  state.rows = [];
  state.appRows = [];
  render();
  load();
}
function range() {
  if (state.view === "summary") return periodRange(state.date, state.period);
  return dayRange(state.date);
}
function loadRange() {
  return state.view === "dashboard" ? periodRange(state.date, "week") : range();
}
function dateTools(key = state.date, update = selectDate, limitToday = false) {
  const controls = el("div", "tools");
  const day = dateFromKey(key);
  const move = sign => {
    if (state.view === "summary" && state.period === "month") return dayKey(new Date(day.getFullYear(), day.getMonth() + sign, 1));
    return dayKey(shiftDay(day, (state.view === "summary" ? 7 : 1) * sign));
  };
  for (const [label, direction, symbol] of [["上一时段", -1, "prev"], ["下一时段", 1, "next"]]) {
    const arrow = button("", () => update(move(direction)), "icon-button", symbol);
    if (limitToday && move(direction) > today()) arrow.disabled = true;
    arrow.title = label;
    arrow.setAttribute("aria-label", label);
    controls.append(arrow);
  }
  const date = el("input");
  date.type = "date";
  date.value = key;
  if (limitToday) date.max = today();
  date.setAttribute("aria-label", "统计日期");
  date.addEventListener("change", () => { if (date.validity.valid) update(date.value); });
  return withChildren(controls, date, button("今天", () => update(today())));
}
function heading(title, subtitle, controls) {
  return withChildren(el("div", "heading"),
    withChildren(el("div"), el("h1", null, title), subtitle && el("p", "subtitle", subtitle)), controls);
}
function segment(values, selected, onChange) {
  return withChildren(el("div", "segmented"), ...values.map(([value, label]) => {
    const node = button(label, () => onChange(value));
    node.setAttribute("aria-pressed", String(selected === value));
    return node;
  }));
}
function stateMeta(row) { return row.playback && row.state === 0 ? { label: "视频/放映", key: "playback" } : STATES[row.state] ?? STATES[3]; }
function labelRow(row) {
  if (!row) return "无记录";
  return row.state !== 0 ? stateMeta(row).label : [row.appName, row.page].filter(Boolean).join(" · ") || "应用未识别";
}
function deviceName(id) {
  const device = state.devices.find(item => item.id === id);
  if (!device) return id;
  const duplicates = state.devices.filter(item => item.name === device.name);
  if (duplicates.length === 1) return device.name;
  return device.name + " (" + (duplicates.findIndex(item => item.id === id) + 1) + ")";
}
const devicePalette = ["var(--device-1)", "var(--device-2)", "var(--device-3)", "var(--device-4)", "var(--device-5)"];
function deviceColor(id) {
  const index = state.devices.findIndex(device => device.id === id);
  return devicePalette[(index < 0 ? 0 : index) % devicePalette.length];
}
function deviceChip(ids) {
  const names = [...ids].map(deviceName);
  const device = state.devices.find(device => device.id === ids.values().next().value);
  const label = withChildren(el("span", "device-chip"), el("span", null, names.length > 1 ? names.length + " 台设备" : names[0]));
  label.title = names.join("、");
  label.setAttribute("aria-label", label.title);
  label.prepend(icon(device?.kind === "phone" || device?.kind === "tablet" ? "phone" : "computer"));
  return label;
}
function deviceSelect(value, update) {
  const select = el("select", "device-select");
  select.setAttribute("aria-label", "筛选设备");
  const all = el("option", null, "全部设备");
  all.value = "";
  select.append(all);
  for (const device of state.devices) {
    const option = el("option", null, deviceName(device.id) + " · " + device.platform);
    option.value = device.id;
    select.append(option);
  }
  select.value = value;
  select.addEventListener("change", () => update(select.value));
  return select;
}
function appGlyph(name) {
  if (/chrome|edge|firefox|浏览器/i.test(name)) return "site";
  if (/code|idea|studio|rustrover|webstorm/i.test(name)) return "code";
  if (/微信|qq|teams|wechat/i.test(name)) return "chat";
  if (/terminal|powershell|终端/i.test(name)) return "terminal";
  if (/word|excel|powerpoint|pdf|文档|wps/i.test(name)) return "doc";
  return null;
}
function progress(value, color = "var(--blue)") {
  const node = el("div", "progress");
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 100 4");
  svg.setAttribute("preserveAspectRatio", "none");
  svg.setAttribute("aria-hidden", "true");
  const rect = document.createElementNS(svg.namespaceURI, "rect");
  rect.setAttribute("width", String(Math.min(100, Math.max(0, value))));
  rect.setAttribute("height", "4");
  rect.setAttribute("fill", color);
  svg.append(rect);
  return withChildren(node, svg);
}
function legend(items) {
  return withChildren(el("div", "legend"), ...items.map(([key, label]) =>
    withChildren(el("span"), el("i", key), document.createTextNode(label))));
}

const appPalette = Array.from({ length: 12 }, (_, index) => `var(--app-${index + 1})`);
let appColors = new Map();
function buildAppColors(rows) {
  const apps = aggregateApps(rows);
  if (apps.length === appColors.size && apps.every(app => appColors.has(app.id))) return;
  appColors = new Map(apps.map((app, index) => [app.id, appPalette[index % appPalette.length]]));
}
function appIdentity(row) {
  return JSON.stringify(row.sharedAppId ? ["shared", row.sharedAppId] : [row.device, row.appId]);
}
function appColor(id) {
  const known = appColors.get(id);
  if (known) return known;
  let hash = 2166136261;
  for (const char of String(id)) { hash ^= char.codePointAt(0); hash = Math.imul(hash, 16777619); }
  hash ^= hash >>> 13; hash = Math.imul(hash, 0x5bd1e995); hash ^= hash >>> 15;
  return appPalette[(hash >>> 0) % appPalette.length];
}

function dailyComposition(start, end, rows = state.rows, appRows = state.appRows) {
  const root = panel("分时应用用时", el("small", null, "色带表示应用用时占比"));
  const grid = el("div", "day-parts");
  const quiet = el("div", "day-quiet");
  const parts = dayParts(rows, appRows, start, Math.min(end, Date.now()));
  const populated = parts.filter(part => part.apps.length);
  grid.style.setProperty("--part-count", Math.max(1, populated.length));
  grid.style.setProperty("--part-count-narrow", Math.max(1, Math.min(2, populated.length)));
  const peak = Math.max(0, ...parts.map(part => part.usage.total ?? 0));
  for (const part of parts) {
    const label = String(part.hour).padStart(2, "0") + ":00–" + String(part.hour + 6).padStart(2, "0") + ":00";
    const go = apps => {
      openRecordDetail({ apps: apps && state.devices.some(device => device.hasAppRows), deviceId: state.deviceFilter, hour: part.hour });
    };
    if (!part.apps.length) {
      const status = part.usage.total > 0 ? duration(part.usage.total) + " · 无应用明细" :
        part.usage.total === 0 ? "无活跃用时" : part.future ? "未到时段" : "无记录";
      const text = part.name + " " + label + " · " + status;
      quiet.append(part.usage.total != null ? button(text, () => go(false), "part-quiet-link") : el("span", "part-quiet-note", text));
      continue;
    }
    const card = el("article", "day-part");
    const heading = withChildren(el("div", "part-heading"), el("h3", null, part.name), el("small", null, label));
    const total = el("strong", "part-total", part.usage.total == null ? "活跃 —" : "活跃 " + duration(part.usage.total));
    total.title = "各设备活跃区间去重；应用并行时各自计时";
    card.append(withChildren(el("div", "part-top"), heading, total));
    {
      const amount = part.apps.reduce((sum, app) => sum + app.duration, 0);
      const first = part.apps.slice(0, 3);
      const tail = amount - first.reduce((sum, app) => sum + app.duration, 0);
      const entries = tail > 0 ? [...first, { name: "其他 " + (part.apps.length - 3) + " 个应用", duration: tail }] : first;
      const bands = el("div", "composition-bar");
      bands.setAttribute("aria-hidden", "true");
      const list = el("ul", "composition-list");
      for (const app of entries) {
        const color = app.id ? appColor(app.id) : "var(--idle)";
        const band = el("span");
        band.style.flexGrow = app.duration;
        band.style.background = color;
        bands.append(band);
        const dot = el("i");
        dot.style.background = color;
        const name = el("span", "composition-name", app.name);
        name.title = app.name;
        list.append(withChildren(el("li"), dot, name, el("span", "duration", duration(app.duration, true))));
      }
      card.append(bands, list);
      if (peak > 0 && part.usage.total === peak && parts.filter(item => item.usage.total === peak).length === 1)
        heading.append(el("span", "part-peak", "用时最多"));
    }
    const open = button("查看记录", () => go(true), "report-link");
    open.setAttribute("aria-label", "查看" + part.name + "的使用记录");
    card.append(open);
    grid.append(card);
  }
  return withChildren(root, populated.length ? grid : empty("这一天还没有应用明细。"), quiet.childElementCount ? quiet : null);
}

function dashboardSources(coverage) {
  const sources = withChildren(el("details", "panel hub-sources"), el("summary", "panel-title", "数据来源与覆盖"));
  sources.open = state.dashboard.sourcesOpen;
  sources.addEventListener("toggle", () => { state.dashboard.sourcesOpen = sources.open; });
  for (const { device, known, elapsed, gap, unobserved } of coverage) {
    const name = deviceName(device.id);
    const glyph = { phone: "phone", tablet: "tablet", computer: "computer" }[device.kind] ?? "unknown";
    const label = withChildren(el("div", "hub-device-name"),
      withChildren(el("span", "hub-device-icon"), icon(glyph)),
      withChildren(el("div"), el("strong", null, name), el("small", null, device.platform)));
    const info = el("span", "hub-device-source", device.local ? (state.running ? "本机采集中" : "本机采集已停止")
      : device.cloud ? "云端记录" : "已导入报告");
    if (device.local) info.dataset.hubCollector = "";
    if (!device.local && device.updatedAtMs != null) {
      info.title = (device.cloud ? "设备最近活动于 " : "报告更新于 ") + new Date(device.updatedAtMs).toLocaleString("zh-CN");
    }
    const observed = elapsed ? `${(known / elapsed * 100).toFixed(1)}% 已观测` : "—";
    const coverage = withChildren(el("div", "hub-device-coverage"),
      progress(elapsed ? known / elapsed * 100 : 0, "var(--teal)"),
      el("small", null, observed));
    coverage.title = `已观测 ${duration(known)} · 采集缺口 ${duration(gap)} · 无记录 ${duration(unobserved)}`;
    const date = state.date;
    const ledger = button("查看记录", () => {
      openRecordDetail({ date, apps: true, deviceId: device.id });
    });
    ledger.setAttribute("aria-label", "查看" + name + "的使用记录");
    sources.append(withChildren(el("div", "hub-device-row"), label, coverage, info, ledger));
  }
  if (state.rejectedSources) sources.append(empty(state.rejectedSources + " 份设备报告未通过校验。"));
  sources.append(withChildren(el("div", "hub-source-actions"), button("管理数据源", () => selectView("settings"), "", "settings")));
  return sources;
}

function dashboardFigures(devices, usage, coverage) {
  const single = devices.length === 1;
  const partial = coverage.some(item => item.known < item.elapsed);
  const observed = coverage[0];
  const figures = single ? [
    ["设备使用时间", duration(usage.total), deviceName(devices[0].id), "clock"],
    ["采集覆盖", observed.elapsed ? (observed.known / observed.elapsed * 100).toFixed(1) + "%" : "—", "已观测时长 / 已过去时长", "target"]] :
    [["总屏幕使用时间", duration(usage.total), "跨设备去重", "clock"],
      ["同时使用", duration(usage.overlap), "至少两台设备同时活跃", "layers"],
      ["设备累计", duration(usage.deviceSum), "各设备活跃时间相加", "devices"]];
  if (partial) figures[0][2] += " · 按已有记录计算";
  return withChildren(el("dl", "kpis hub-kpis" + (single ? " single" : "")), ...figures.map(([label, value, hint, glyph]) =>
    withChildren(el("div", "kpi"),
      withChildren(el("dt"), icon(glyph), document.createTextNode(label)),
      el("dd", null, value), el("small", null, hint))));
}

function appsAt(rows, at) {
  const found = new Map();
  for (const row of rows) {
    if (row.state !== 0 || !row.appId || row.startMs > at || row.endMs <= at) continue;
    const id = appIdentity(row);
    const app = found.get(id) ?? { id, name: row.appName || row.appId, devices: new Set() };
    app.devices.add(row.device);
    found.set(id, app);
  }
  return [...found.values()].sort((a, b) => a.name.localeCompare(b.name, "zh-CN"));
}

function dashboardTimeline(devices, rows, appRows, dayStart, dayEnd, now) {
  const hub = state.dashboard;
  const focus = hub.focus;
  const start = focus ? focus.startMs : dayStart;
  const end = focus ? focus.endMs : dayEnd;
  const span = end - start;
  const visibleIds = devices.map(device => device.id);
  const sourceRows = clipRows(rows, start, end);
  const sourceApps = clipRows(appRows, start, end);
  const usage = usageMetrics(sourceRows, true);
  const byDevice = Map.groupBy(sourceRows, row => row.device);
  const appsByDevice = Map.groupBy(sourceApps.filter(row => row.state === 0 && row.appId), row => row.device);
  const clampRatio = value => Math.max(0, Math.min(.999999, value));
  const secondsLabel = span < 10 * 60e3;
  const clock = at => at >= dayEnd ? "24:00" : secondsLabel ? textTime.format(at) : textTime.format(at).slice(0, 5);
  const timeLabel = (from, to) => clock(from) + "–" + clock(to);
  const availableEnd = Math.min(end, Math.max(now, start));

  function quietLabel(range) {
    const rowsAt = sourceRows.filter(row => row.startMs < range.endMs && row.endMs > range.startMs);
    if (!rowsAt.length) return "无记录";
    const states = new Set(rowsAt.map(row => row.state));
    if ([...states].every(state => state !== 0)) {
      const hour = new Date(range.startMs).getHours();
      const night = range.endMs - range.startMs >= 2 * 3600e3 && (hour >= 22 || hour < 8);
      if (night) return "睡眠";
      if (states.size === 1 && states.has(1)) return "暂停";
      if (states.has(2)) return "离线";
      if (states.size === 1 && states.has(3)) return "采集缺口";
      return "无活动";
    }
    return "无活动";
  }
  const quietRanges = focus ? [] : quietSpans(activeSpans(sourceRows, start, end), start, end).flatMap(range => {
    if (range.startMs < now && range.endMs > now) {
      return [
        { ...range, endMs: now, label: quietLabel({ ...range, endMs: now }) },
        { ...range, startMs: now, label: "未来" }
      ];
    }
    return [{ ...range, label: range.startMs >= now ? "未来" : quietLabel(range) }];
  });
  const scale = buildTimelineScale(start, end, quietRanges, TIMELINE_SCALE);
  const ratio = at => scale.toRatio(at);
  const timeAt = value => scale.fromRatio(value);
  const compactDuration = ms => {
    const hours = ms / 3600e3;
    return hours >= 1 ? (Math.round(hours * 10) / 10).toString().replace(/\.0$/, "") + "h" :
      Math.max(1, Math.round(ms / 60e3)) + "m";
  };

  const normalSpan = focus ? span : Math.max(60e3,
    scale.segments.filter(segment => !segment.compressed).reduce((sum, segment) => sum + segment.to - segment.from, 0));
  const steps = [10e3, 15e3, 30e3, 60e3, 120e3, 300e3, 600e3, 900e3, 1800e3, 3600e3, 7200e3, 10800e3, 21600e3, 43200e3];
  const step = steps.find(value => normalSpan / value <= 6) ?? 86400e3;
  const tickValues = [];
  if (step < 3600e3) {
    for (let at = Math.ceil(start / step) * step; at < end; at += step)
      if (at > start + 500) tickValues.push(at);
  } else {
    const hours = step / 3600e3;
    const cursor = new Date(start);
    cursor.setMinutes(0, 0, 0);
    cursor.setHours(Math.ceil(cursor.getHours() / hours) * hours);
    while (+cursor < end) {
      if (+cursor > start + 500) tickValues.push(+cursor);
      cursor.setHours(cursor.getHours() + hours);
    }
  }
  const inCompressed = at => scale.compressed.some(segment => at > segment.from && at < segment.to);
  const boundaryTicks = [...new Set(scale.compressed.flatMap(segment => [segment.from, segment.to]))]
    .filter(at => at > start && at < end);
  const boundarySet = new Set(boundaryTicks);
  const majorTicks = [...new Set([...tickValues.filter(at => !inCompressed(at)), ...boundaryTicks])]
    .sort((a, b) => a - b);
  const minorTicks = tickValues.map(at => at + step / 2)
    .filter(at => at > start && at < end && !inCompressed(at));
  const edgeGap = Math.max(span * .045, 16e3);
  const nearCompressed = at => scale.compressed.some(segment => {
    const center = (segment.startRatio + segment.endRatio) / 2;
    return Math.abs(ratio(at) - center) < .045;
  });
  const labelTicks = [start, ...majorTicks.filter(at => at - start > edgeGap && end - at > edgeGap && !nearCompressed(at)), end];

  const key = legend([["hub-app-swatch", "应用"], ["idle", "暂停"], ["offline", "离线"], ["gap", "采集缺口"], ["hub-unknown", "无记录"], ["hub-future", "未来"]]);
  const meta = el("div", "hub-timeline-meta");
  if (focus) {
    const chip = el("button", "hub-focus-chip");
    chip.type = "button";
    chip.append(el("span", null, timeLabel(focus.startMs, focus.endMs)), el("small", null, duration(span)), icon("close"));
    chip.title = "退出聚焦，返回全天";
    chip.setAttribute("aria-label", "退出聚焦，返回全天");
    chip.addEventListener("click", clearFocus);
    meta.append(chip, el("small", "hub-timeline-hint", "拖动继续放大 · Esc 返回全天"));
  } else {
    meta.append(el("small", null, shortDate.format(dayStart)),
      el("small", "hub-timeline-hint", "悬停查看 · 拖动框选或点击色块"));
  }
  const section = panel("使用时间轴", meta);
  section.classList.add("hub-timeline");
  section.classList.toggle("is-focus", !!focus);
  section.addEventListener("keydown", event => {
    if (event.key === "Escape" && hub.focus) {
      event.preventDefault();
      clearFocus();
    }
  });
  const grid = el("div", "hub-time-grid");
  const axis = el("div", "axis hub-axis");
  for (const [index, at] of labelTicks.entries()) {
    const tick = el("span", "hub-axis-label" + (at === end ? " hub-axis-end" : ""), clock(at));
    if (index === 0 && at === start) tick.style.transform = "none";
    if (index === labelTicks.length - 1 && at === end) tick.style.transform = "translateX(-100%)";
    tick.style.left = ratio(at) * 100 + "%";
    axis.append(tick);
  }
  for (const at of majorTicks) {
    const tick = el("i", "major" + (boundarySet.has(at) ? " compressed-edge" : ""));
    tick.style.left = ratio(at) * 100 + "%";
    axis.append(tick);
  }
  for (const at of minorTicks) {
    const tick = el("i");
    tick.style.left = ratio(at) * 100 + "%";
    axis.append(tick);
  }
  for (const [index, segment] of scale.compressed.entries()) {
    const next = scale.compressed[index + 1];
    if (next && next.from - segment.to <= 60e3 && segment.to - segment.from < 2 * 3600e3) continue;
    const name = segment.label || "折叠";
    const text = segment.to - segment.from >= 2 * 3600e3 ? name + " " + compactDuration(segment.to - segment.from) : name;
    const label = el("span", "hub-axis-compressed", text);
    if (segment.from <= start) {
      label.style.left = ((segment.startRatio + segment.endRatio) / 2) * 100 + "%";
    } else {
      label.style.left = segment.startRatio * 100 + "%";
      label.style.transform = "none";
    }
    label.title = name + " " + timeLabel(segment.from, segment.to) + " · " + duration(segment.to - segment.from);
    axis.append(label);
  }
  grid.append(axis);

  function deviceItems(deviceId) {
    const states = byDevice.get(deviceId) ?? [];
    const apps = appsByDevice.get(deviceId) ?? [];
    const items = [];
    let appIndex = 0;
    for (const row of states) {
      if (row.state !== 0) { items.push({ ...row, devices: [deviceId] }); continue; }
      let cursor = row.startMs;
      while (appIndex < apps.length && apps[appIndex].endMs <= row.startMs) appIndex++;
      for (let index = appIndex; index < apps.length && apps[index].startMs < row.endMs; index++) {
        const app = apps[index];
        const from = Math.max(app.startMs, row.startMs);
        const to = Math.min(app.endMs, row.endMs);
        if (to <= from) continue;
        if (from > cursor) items.push({ ...row, startMs: cursor, endMs: from, appId: null, appName: null, unidentified: true, devices: [deviceId] });
        items.push({ ...app, startMs: from, endMs: to, state: 0, devices: [deviceId] });
        if (to > cursor) cursor = to;
      }
      if (cursor < row.endMs) items.push({ ...row, startMs: cursor, endMs: row.endMs, appId: null, appName: null, unidentified: true, devices: [deviceId] });
    }
    return coalesceTimelineItems(items, TIMELINE_SMOOTH_GAP_MS);
  }

  const lanes = devices.map(device => ({
    label: deviceName(device.id), deviceId: device.id, color: deviceColor(device.id), overlap: false,
    items: deviceItems(device.id)
  }));
  if (devices.length > 1) lanes.unshift({ label: "同时使用", color: "var(--hub-overlap)", items: usage.overlapSpans, overlap: true });

  const markLookup = new Map();
  for (const [index, laneInfo] of lanes.entries()) {
    const time = laneInfo.overlap ? duration(usage.overlap)
      : laneInfo.items.some(row => row.state !== 3) ? duration(usage.byDevice.get(laneInfo.deviceId) ?? 0) : "—";
    const dot = el("i", "hub-lane-dot");
    dot.style.background = laneInfo.color;
    const label = withChildren(el("div", "hub-lane-label"), dot,
      el("span", "hub-lane-name", laneInfo.label), el("small", "hub-lane-time", time));
    label.title = laneInfo.label + " · " + time;
    label.style.gridColumn = "1";
    label.style.gridRow = String(index + 2);
    const lane = el("div", "hub-lane" + (laneInfo.overlap ? " overlap" : ""));
    lane.style.gridColumn = "2";
    lane.style.gridRow = String(index + 2);
    for (const at of minorTicks) {
      const line = el("span", "hub-tick-line");
      line.style.left = ratio(at) * 100 + "%";
      lane.append(line);
    }
    for (const at of majorTicks) {
      const line = el("span", "hub-tick-line major" + (boundarySet.has(at) ? " compressed-edge" : ""));
      line.style.left = ratio(at) * 100 + "%";
      lane.append(line);
    }
    for (const segment of scale.compressed) {
      const compressed = el("span", "hub-compressed-band");
      compressed.style.left = segment.startRatio * 100 + "%";
      compressed.style.width = (segment.endRatio - segment.startRatio) * 100 + "%";
      lane.append(compressed);
    }
    if (now < end) {
      const future = el("span", "hub-future");
      future.style.left = ratio(Math.max(now, start)) * 100 + "%";
      lane.append(future);
    }
    for (const item of laneInfo.items) {
      const from = ratio(item.startMs);
      const displayed = ratio(item.endMs) - from;
      const app = item.state === 0 && item.appId && !item.unidentified ? appIdentity(item) : null;
      const mark = el("span", "hub-mark " + (STATES[item.state]?.key ?? "active") + (item.unidentified ? " unknown" : ""));
      mark.style.left = from * 100 + "%";
      mark.style.width = Math.max(.12, displayed * 100) + "%";
      mark.style.setProperty("--hub-lane-color", app ? appColor(app) : laneInfo.color);
      const text = laneInfo.overlap ? "" : item.state === 0
        ? (item.unidentified ? "应用未识别" : item.appName || "应用未识别")
        : item.state === 1 ? "暂停" : item.state === 2 ? "离线" : "";
      if (text && displayed >= (focus ? .035 : .04)) mark.append(el("span", "hub-mark-label", text));
      const smoothed = item.merged > 1 ? " · 已平滑 " + item.merged + " 段" : "";
      mark.title = timeLabel(item.startMs, item.endMs) + " · " + (laneInfo.overlap ? item.devices.map(deviceName).join("、") : labelRow(item)) + smoothed;
      markLookup.set(mark, { item, mark, overlap: laneInfo.overlap });
      lane.append(mark);
    }
    if (!laneInfo.items.length) lane.append(el("span", "hub-lane-empty", laneInfo.overlap ? "无同时使用" : "无记录"));
    grid.append(label, lane);
    if (!laneInfo.items.length) continue;
    let current = -1;
    lane.tabIndex = 0;
    lane.setAttribute("role", "group");
    lane.setAttribute("aria-label", `${laneInfo.label}，${laneInfo.items.length} 个区间，按方向键选择时段`);
    lane.addEventListener("keydown", event => {
      if (event.key === "Escape" && hub.focus) {
        event.preventDefault();
        clearFocus();
        return;
      }
      if (event.key === "Enter" && hub.focus) {
        event.preventDefault();
        openLedger(hub.focus.startMs, hub.focus.endMs, true);
        return;
      }
      const next = { ArrowLeft: Math.max(0, current - 1), ArrowRight: Math.min(laneInfo.items.length - 1, current + 1), Home: 0, End: laneInfo.items.length - 1 }[event.key];
      if (next == null) return;
      event.preventDefault();
      current = next;
      const item = laneInfo.items[next];
      enterFocus(item.startMs, item.endMs, markStatus(item));
    });
  }
  if (!lanes.length) grid.append(empty("正在读取设备…"));
  if (!focus && lanes.some(lane => lane.items.some(item => item.merged > 1)))
    meta.append(el("small", "hub-timeline-hint", "短切换已平滑"));

  const overlay = el("div", "hub-overlay");
  const crosshair = el("span", "hub-crosshair");
  const band = el("span", "hub-range");
  crosshair.hidden = band.hidden = true;
  overlay.append(crosshair);
  if (now > start && now < end) {
    const nowLine = el("span", "hub-now-line");
    nowLine.style.left = ratio(now) * 100 + "%";
    nowLine.title = "现在 " + textTime.format(now).slice(0, 5);
    overlay.append(nowLine);
  }
  overlay.append(band);
  if (lanes.length) {
    overlay.style.gridColumn = "2";
    overlay.style.gridRow = "2 / span " + lanes.length;
    grid.append(overlay);
  }

  const tip = el("div", "hub-tip");
  tip.hidden = true;
  tip.setAttribute("role", "tooltip");
  const dragChip = el("div", "hub-drag-chip");
  dragChip.hidden = true;
  let drag = null;
  let hovered = null;
  let hoverSecond = -1;
  let dragKey = "";

  function openLedger(from, to, appsMode) {
    openRecordDetail({
      apps: !!appsMode,
      deviceId: state.deviceFilter,
      scope: { startMs: from, endMs: to, devices: visibleIds, label: timeLabel(from, to) }
    });
  }

  function clearFocus() {
    if (!hub.focus) return;
    hub.focus = null;
    smoothRender();
  }

  function markStatus(item) {
    if (!item) return "";
    if (item.unidentified) return "应用未识别";
    if (item.device == null || item.state == null || item.state === 0) return "";
    return deviceName(item.device) + " · " + stateMeta(item).label;
  }

  function enterFocus(from, to, status = "") {
    let first = Math.max(start, Math.min(from, to));
    let last = Math.min(availableEnd, Math.max(from, to));
    if (last - first < 60e3) {
      last = Math.min(availableEnd, first + 60e3);
      if (last - first < 60e3) first = Math.max(start, last - 60e3);
    }
    if (last - first < 1000) return;
    hub.focus = { startMs: first, endMs: last, status };
    smoothRender();
  }

  function placeTip(x, y) {
    const bounds = section.getBoundingClientRect();
    const width = tip.offsetWidth;
    const height = tip.offsetHeight;
    const left = Math.min(Math.max(10, x - bounds.left + 14), Math.max(10, bounds.width - width - 10));
    let top = y - bounds.top + 18;
    if (top + height > bounds.height - 10) top = Math.max(10, y - bounds.top - height - 14);
    tip.style.left = left + "px";
    tip.style.top = top + "px";
  }

  function placeDragChip(x, y) {
    const bounds = section.getBoundingClientRect();
    const width = dragChip.offsetWidth;
    const left = Math.min(Math.max(8, x - bounds.left + 12), Math.max(8, bounds.width - width - 8));
    let top = y - bounds.top - dragChip.offsetHeight - 12;
    if (top < 8) top = y - bounds.top + 18;
    dragChip.style.left = left + "px";
    dragChip.style.top = top + "px";
  }

  function tipHover(at) {
    const folded = scale.compressed.find(segment => at >= segment.from && at < segment.to);
    if (folded) {
      tip.replaceChildren(
        withChildren(el("header"), el("span", "hub-tip-eyebrow", "折叠时段"), el("strong", null, textTime.format(at))),
        el("p", "hub-tip-meta", (folded.label || "无活动") + " · " + timeLabel(folded.from, folded.to)),
        el("p", "hub-tip-empty", "已折叠 " + duration(folded.to - folded.from) + "，点击查看该时段"));
      tip.hidden = false;
      return;
    }
    const future = at > now;
    const header = withChildren(el("header"), el("span", "hub-tip-eyebrow", future ? "未来" : "此刻"), el("strong", null, textTime.format(at)));
    const nodes = [header];
    if (future) {
      nodes.push(el("p", "hub-tip-empty", "该时段尚未发生。"));
      tip.replaceChildren(...nodes);
      tip.hidden = false;
      return;
    }
    const apps = appsAt(sourceApps, at);
    if (apps.length) {
      const devicesAt = new Set(apps.flatMap(app => [...app.devices]));
      nodes.push(el("p", "hub-tip-meta", apps.length + " 个应用 · " + devicesAt.size + " 台设备"));
      const list = el("ul", "hub-tip-apps");
      for (const app of apps.slice(0, 3)) {
        const dot = el("i");
        dot.style.background = appColor(app.id);
        list.append(withChildren(el("li"), dot, el("span", "name", app.name),
          el("span", "hub-tip-devices", [...app.devices].map(deviceName).join("、"))));
      }
      nodes.push(list);
      if (apps.length > 3) nodes.push(el("p", "hub-tip-note", "还有 " + (apps.length - 3) + " 个应用"));
    } else {
      const states = el("div", "hub-tip-states");
      for (const id of visibleIds) {
        const row = (byDevice.get(id) ?? []).find(item => item.startMs <= at && item.endMs > at);
        states.append(withChildren(el("span"), el("i"), document.createTextNode(deviceName(id) + " · " + (row ? labelRow(row) : "无记录"))));
      }
      nodes.push(states.childElementCount ? states : el("p", "hub-tip-empty", "这一刻没有记录。"));
    }
    tip.replaceChildren(...nodes);
    tip.hidden = false;
  }

  function focusSummary() {
    if (!focus) return null;
    const apps = aggregateApps(sourceApps);
    const activeDevices = new Set(sourceRows.filter(row => row.state === 0).map(row => row.device));
    const appTotal = apps.reduce((sum, app) => sum + app.duration, 0);
    const top = apps[0]?.duration || 1;
    const root = el("section", "hub-focus-summary");
    root.setAttribute("aria-live", "polite");
    const info = withChildren(el("div", "hub-focus-info"),
      withChildren(el("span", "eyebrow"), icon("target"), document.createTextNode("已选时段 · 应用排行")),
      el("strong", "hub-focus-range", timeLabel(focus.startMs, focus.endMs)),
      withChildren(el("p", "hub-focus-meta"),
        el("span", null, duration(span)),
        el("span", null, usage.total == null ? "活跃 —" : "活跃 " + duration(usage.total)),
        el("span", null, activeDevices.size + " 台设备"),
        focus.status ? el("span", "hub-focus-status", focus.status) : null),
      withChildren(el("div", "hub-focus-actions"),
        button("查看记录", () => openLedger(focus.startMs, focus.endMs, apps.length > 0), "primary"),
        button("返回全天", clearFocus)));
    root.append(info);
    if (apps.length) {
      const list = el("ol", "hub-focus-apps");
      for (const [index, app] of apps.slice(0, 5).entries()) {
        const sources = [...app.devices].map(deviceName).join("、");
        list.append(withChildren(el("li", "hub-focus-app"),
          el("span", "hub-app-rank", String(index + 1)),
          withChildren(el("div", "hub-focus-app-name"), el("strong", null, app.name), el("small", null, sources)),
          progress(app.duration / top * 100, appColor(app.id)),
          el("span", "duration", duration(app.duration)),
          el("small", "share", appTotal ? Math.round(app.duration / appTotal * 100) + "%" : "—")));
      }
      root.append(list);
      const notes = [];
      if (apps.length > 5) notes.push("还有 " + (apps.length - 5) + " 个应用");
      if (usage.total != null && appTotal - usage.total > 60000) notes.push("应用并行时分别计时");
      if (notes.length) root.append(el("p", "hub-focus-note", notes.join(" · ")));
    } else {
      root.append(el("p", "hub-focus-empty", focus.status ? "这一时段没有应用使用记录。" : "这一时段没有应用明细。"));
    }
    return root;
  }

  function markAt(x, y) {
    for (const node of document.elementsFromPoint(x, y)) {
      const entry = markLookup.get(node);
      if (entry) return entry;
    }
    return null;
  }
  function highlight(entry) {
    if (hovered?.mark === entry?.mark) return;
    hovered?.mark.classList.remove("hovered");
    hovered = entry;
    hovered?.mark.classList.add("hovered");
  }
  function orderRange(a, b) {
    const from = Math.max(start, Math.min(availableEnd, timeAt(Math.min(a, b))));
    const to = Math.max(from + 1, Math.min(availableEnd, timeAt(Math.max(a, b))));
    return [from, to];
  }
  function updateBand(a, b) {
    band.hidden = false;
    band.style.left = Math.min(a, b) * 100 + "%";
    band.style.width = Math.abs(a - b) * 100 + "%";
  }

  if (lanes.length) {
    overlay.addEventListener("pointerdown", event => {
      if (event.button !== 0 || availableEnd - start < 1000) return;
      const rect = overlay.getBoundingClientRect();
      const anchor = clampRatio((event.clientX - rect.left) / rect.width);
      if (timeAt(anchor) > now) return;
      drag = {
        pointerId: event.pointerId,
        anchor,
        current: anchor,
        startX: event.clientX,
        mark: markAt(event.clientX, event.clientY),
        moved: false
      };
      overlay.setPointerCapture(event.pointerId);
      hoverSecond = -1;
      dragKey = "";
      tip.hidden = true;
      dragChip.hidden = true;
      band.hidden = true;
      event.preventDefault();
    });
    overlay.addEventListener("pointermove", event => {
      const rect = overlay.getBoundingClientRect();
      const at = clampRatio((event.clientX - rect.left) / rect.width);
      crosshair.hidden = false;
      crosshair.style.left = at * 100 + "%";
      if (drag) {
        if (!drag.moved && Math.abs(event.clientX - drag.startX) > 4) drag.moved = true;
        if (drag.moved) {
          drag.current = Math.min(at, clampRatio(ratio(availableEnd)));
          updateBand(drag.anchor, drag.current);
          highlight(null);
          tip.hidden = true;
          const [from, to] = orderRange(drag.anchor, drag.current);
          const rangeKey = Math.floor(from / 60000) + ":" + Math.floor(to / 60000);
          if (rangeKey !== dragKey) {
            dragKey = rangeKey;
            dragChip.textContent = timeLabel(from, to) + " · " + duration(to - from);
          }
          dragChip.hidden = false;
          placeDragChip(event.clientX, event.clientY);
        }
        return;
      }
      highlight(markAt(event.clientX, event.clientY));
      const instant = timeAt(at);
      const second = Math.floor(instant / 1000);
      if (second !== hoverSecond || tip.hidden) {
        hoverSecond = second;
        tipHover(instant);
      }
      placeTip(event.clientX, event.clientY);
    });
    overlay.addEventListener("pointerup", event => {
      if (!drag) return;
      const { pointerId, anchor, current, mark, moved } = drag;
      drag = null;
      if (overlay.hasPointerCapture(pointerId)) overlay.releasePointerCapture(pointerId);
      crosshair.hidden = true;
      dragChip.hidden = true;
      band.hidden = true;
      tip.hidden = true;
      if (!moved) {
        if (mark) {
          enterFocus(mark.item.startMs, mark.item.endMs, markStatus(mark.item));
          return;
        }
        const at = timeAt(anchor);
        const folded = scale.compressed.find(segment => at >= segment.from && at < segment.to);
        if (folded) enterFocus(folded.from, folded.to, folded.label);
        return;
      }
      const [from, to] = orderRange(anchor, current);
      enterFocus(from, to);
    });
    overlay.addEventListener("pointercancel", () => {
      drag = null;
      crosshair.hidden = true;
      dragChip.hidden = true;
      band.hidden = true;
    });
    overlay.addEventListener("lostpointercapture", () => {
      if (!drag) return;
      drag = null;
      crosshair.hidden = true;
      dragChip.hidden = true;
      band.hidden = true;
    });
    overlay.addEventListener("pointerleave", () => {
      if (drag) return;
      crosshair.hidden = true;
      highlight(null);
      hoverSecond = -1;
      tip.hidden = true;
    });
  }

  const summary = focusSummary();
  section.append(lanes.length ? grid : empty(state.loading ? "正在读取设备…" : "暂无设备记录。"),
    withChildren(el("div", "hub-time-legend"), key));
  if (summary) section.append(summary);
  section.append(tip, dragChip);
  return section;
}

function dashboardPie(coverage, usage, apps) {
  const root = panel("设备用时分布", el("small", null, "占设备累计 · 悬停看应用"));
  const total = usage.deviceSum;
  const entries = coverage.map(({ device, known }) => ({ device, time: known ? usage.byDevice.get(device.id) ?? 0 : null }))
    .sort((a, b) => (b.time ?? -1) - (a.time ?? -1));
  const body = el("div", "hub-pie-body");
  const items = el("ul", "hub-pie-key");
  const percentText = value => value == null ? "—" : new Intl.NumberFormat("zh-CN", { style: "percent", maximumFractionDigits: 1 }).format(value);
  const appsByDevice = Map.groupBy(apps.filter(row => row.state === 0 && row.appId), row => row.device);
  const slices = [];
  if (total > 0) {
    let offset = 0;
    for (const entry of entries) {
      if (!(entry.time > 0)) continue;
      const start = offset / total;
      offset += entry.time;
      slices.push({ entry, start, end: offset / total });
    }
  }
  let pie = null;
  let glare = null;
  if (slices.length) {
    pie = el("div", "hub-pie");
    glare = el("span", "hub-pie-glare");
    pie.style.background = "conic-gradient(" + slices.map(slice =>
      deviceColor(slice.entry.device.id) + " " + slice.start * 100 + "% " + slice.end * 100 + "%").join(",") + ")";
    const compact = total >= 3600000 ? (total / 3600000).toFixed(1).replace(/\.0$/, "") + " 小时" : Math.max(1, Math.round(total / 60000)) + " 分";
    pie.append(glare, withChildren(el("span", "hub-pie-center"), el("strong", null, compact), el("small", null, "设备累计")));
    pie.setAttribute("aria-hidden", "true");
    body.append(pie);
  }

  const tip = el("div", "hub-pie-tip");
  tip.hidden = true;
  tip.setAttribute("role", "dialog");
  tip.setAttribute("aria-label", "设备应用排行");
  let active = null;
  let hideTimer = 0;
  const markRows = () => {
    for (const row of items.children) {
      if (active && row.dataset.device === String(active.device.id)) row.dataset.active = "";
      else row.removeAttribute("data-active");
    }
  };
  const close = () => {
    clearTimeout(hideTimer);
    active = null;
    tip.hidden = true;
    if (glare) glare.style.background = "none";
    if (pie) pie.removeAttribute("data-active");
    markRows();
  };
  const schedule = () => {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (tip.matches(":hover") || tip.contains(document.activeElement)) return;
      close();
    }, 260);
  };
  const keep = () => clearTimeout(hideTimer);
  const show = slice => {
    keep();
    if (active === slice.entry) return;
    active = slice.entry;
    const deviceApps = aggregateApps(appsByDevice.get(slice.entry.device.id) ?? []);
    const swatch = el("i", "hub-pie-tip-swatch");
    swatch.style.background = deviceColor(slice.entry.device.id);
    tip.replaceChildren(withChildren(el("header", "hub-pie-tip-head"), swatch,
      el("strong", null, deviceName(slice.entry.device.id)),
      el("span", "hub-pie-tip-share", percentText(total > 0 && slice.entry.time != null ? slice.entry.time / total : null))),
      withChildren(el("p", "hub-pie-tip-meta"),
        el("span", null, "累计 " + duration(slice.entry.time)),
        deviceApps.length ? el("span", null, deviceApps.length + " 个应用") : null));
    if (deviceApps.length) {
      const list = el("ol", "hub-pie-tip-list");
      for (const [index, app] of deviceApps.entries()) {
        const dot = el("i", "hub-pie-tip-dot");
        dot.style.background = appColor(app.id);
        const name = el("span", "hub-pie-tip-name", app.name);
        name.title = app.name;
        list.append(withChildren(el("li"),
          el("span", "hub-pie-tip-rank", String(index + 1)), dot, name,
          el("span", "hub-pie-tip-time", duration(app.duration))));
      }
      tip.append(list);
      if (deviceApps.length > 5) {
        const note = el("p", "hub-pie-tip-note", "滚动查看全部 " + deviceApps.length + " 个应用");
        list.classList.add("scrollable");
        list.addEventListener("scroll", () => {
          const end = list.scrollTop + list.clientHeight >= list.scrollHeight - 2;
          list.classList.toggle("at-end", end);
          note.textContent = end ? "已到底部 · 共 " + deviceApps.length + " 个应用" : "滚动查看全部 " + deviceApps.length + " 个应用";
        }, { passive: true });
        tip.append(note);
      }
    } else {
      tip.append(el("p", "hub-pie-tip-empty", "该设备暂无应用明细"));
    }
    if (state.devices.length > 1) {
      tip.append(button(state.deviceFilter ? "查看全部设备" : "只看这台设备", () => {
        state.deviceFilter = state.deviceFilter ? "" : slice.entry.device.id;
        state.dashboard.focus = null;
        render();
      }, "hub-pie-tip-action"));
    }
    tip.hidden = false;
    const bodyRect = body.getBoundingClientRect();
    const pieRect = pie.getBoundingClientRect();
    const width = tip.offsetWidth;
    const height = tip.offsetHeight;
    const left = Math.max(3, Math.min(pieRect.left - bodyRect.left + pieRect.width / 2 - width / 2, bodyRect.width - width - 3));
    let top = pieRect.bottom - bodyRect.top + 8;
    if (pieRect.bottom + 8 + height > window.innerHeight - 8 && pieRect.top - height - 8 > 8)
      top = pieRect.top - bodyRect.top - height - 8;
    tip.style.left = Math.round(left) + "px";
    tip.style.top = Math.round(top) + "px";
    glare.style.background = "conic-gradient(#0b152626 0 " + slice.start * 100 + "%, transparent " + slice.start * 100 + "% " +
      slice.end * 100 + "%, #0b152626 " + slice.end * 100 + "% 100%)";
    pie.dataset.active = "";
    markRows();
  };

  for (const { device, time } of entries) {
    const swatch = el("i", "swatch");
    swatch.style.background = deviceColor(device.id);
    const share = total > 0 && time != null ? time / total : null;
    const row = withChildren(el("li"), swatch, el("span", null, deviceName(device.id)),
      el("span", "duration", duration(time)), el("small", null, percentText(share)));
    row.dataset.device = device.id;
    const slice = slices.find(item => item.entry.device.id === device.id);
    if (slice) {
      row.tabIndex = 0;
      row.setAttribute("aria-label", deviceName(device.id) + "，" + duration(time) + "，占比 " + percentText(share));
      row.addEventListener("pointerenter", () => show(slice));
      row.addEventListener("pointerleave", () => { if (active) schedule(); });
      row.addEventListener("focus", () => show(slice));
      row.addEventListener("blur", () => { if (active) schedule(); });
      row.addEventListener("keydown", event => {
        if (event.key !== "Escape") return;
        event.stopPropagation();
        close();
      });
    }
    items.append(row);
  }
  if (!entries.length) body.append(empty("暂无设备记录。"));
  else body.append(items);

  if (pie) {
    pie.addEventListener("pointermove", event => {
      const rect = pie.getBoundingClientRect();
      const dx = event.clientX - rect.left - rect.width / 2;
      const dy = event.clientY - rect.top - rect.height / 2;
      const radius = Math.hypot(dx, dy);
      if (radius > rect.width / 2 - 1 || radius < rect.width * 0.23) { if (active) schedule(); return; }
      const fraction = ((Math.atan2(dx, -dy) + Math.PI * 2) % (Math.PI * 2)) / (Math.PI * 2);
      const slice = slices.find(item => fraction >= item.start && fraction < item.end);
      if (slice) show(slice);
      else if (active) schedule();
    });
    pie.addEventListener("pointerleave", () => { if (active) schedule(); });
    tip.addEventListener("pointerenter", keep);
    tip.addEventListener("pointerleave", () => { if (active) schedule(); });
    tip.addEventListener("focusout", event => { if (!tip.contains(event.relatedTarget)) schedule(); });
    tip.addEventListener("keydown", event => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      close();
    });
    body.append(tip);
  }
  return withChildren(root, body);
}

function dashboardApps(rows) {
  const apps = aggregateApps(rows);
  const root = panel("跨设备应用榜", withChildren(el("span", "hub-rank-head"),
    el("small", "hub-rank-scope", state.date === today() ? "今日" : "当天"),
    apps.length > 0 && el("small", "hub-rank-count", apps.length + " 个应用")));
  if (!apps.length) return withChildren(root, empty("当天暂无应用记录。"));
  const total = apps.reduce((sum, app) => sum + app.duration, 0);
  const list = el("ol", "hub-apps");
  for (const [index, app] of apps.entries()) {
    const sources = [...app.devices].map(id => {
      const platform = state.devices.find(device => device.id === id)?.platform;
      return [deviceName(id), platform].filter(Boolean).join(" · ");
    }).join("、");
    const share = total ? Math.round(app.duration / total * 100) : 0;
    list.append(withChildren(el("li", "hub-app"),
      el("span", "hub-app-rank", String(index + 1)),
      withChildren(el("div", "hub-app-name"), el("strong", null, app.name), el("small", null, sources)),
      progress(app.duration / apps[0].duration * 100, appColor(app.id)),
      withChildren(el("span", "hub-app-time"), el("span", "duration", duration(app.duration)), el("small", null, share + "%"))));
  }
  if (apps.length <= 5) return withChildren(root, list);
  const scroll = withChildren(el("div", "hub-apps-scroll scrollable"), list);
  scroll.tabIndex = 0;
  scroll.setAttribute("aria-label", "应用排行榜，共 " + apps.length + " 个应用，可滚动查看");
  const hint = el("p", "hub-apps-hint", "滚动查看全部 " + apps.length + " 个应用");
  scroll.addEventListener("scroll", () => {
    const end = scroll.scrollTop + scroll.clientHeight >= scroll.scrollHeight - 2;
    scroll.classList.toggle("at-end", end);
    hint.textContent = end ? "已到底部 · 共 " + apps.length + " 个应用" : "滚动查看全部 " + apps.length + " 个应用";
  }, { passive: true });
  return withChildren(root, scroll, hint);
}

function dashboardWeekContext(rows, devices) {
  const now = Date.now();
  const [weekStart, weekEnd] = periodRange(state.date, "week");
  const usage = usageMetrics(clipRows(rows, weekStart, Math.min(weekEnd, now)));
  return withChildren(el("div", "hub-week-context"),
    withChildren(el("span", "hub-week-context-label"), icon("chart"), document.createTextNode("本周")),
    el("strong", null, duration(usage.total)),
    el("small", null, devices.length > 1 ? "去重 · 同时使用 " + duration(usage.overlap) : "本周累计"),
    button("查看趋势", () => {
      state.period = "week";
      selectView("summary");
    }, "hub-week-context-link", "next"));
}

function dashboardPage() {
  const now = Date.now();
  const [start, end] = dayRange(state.date);
  const devices = state.devices.filter(device => !state.deviceFilter || state.deviceFilter === device.id);
  const rows = state.rows.filter(row => !state.deviceFilter || state.deviceFilter === row.device);
  const dayRows = clipRows(rows, start, end);
  const dayApps = clipRows(state.appRows.filter(row => !state.deviceFilter || state.deviceFilter === row.device), start, end);
  const usage = usageMetrics(dayRows);
  const coverage = deviceCoverage(devices, dayRows, start, Math.min(end, now));
  if (state.dashboard.focus && (state.dashboard.focus.startMs < start || state.dashboard.focus.endMs > end || state.dashboard.focus.endMs <= state.dashboard.focus.startMs)) state.dashboard.focus = null;
  const controls = withChildren(el("div", "tools hub-controls"),
    dateTools(state.date, selectDate, true),
    deviceSelect(state.deviceFilter, value => { state.deviceFilter = value; state.dashboard.focus = null; render(); }));
  return [heading("多设备仪表盘", shortDate.format(start), controls), dashboardFigures(devices, usage, coverage),
    dashboardWeekContext(rows, devices),
    dashboardTimeline(devices, dayRows, dayApps, start, end, now),
    withChildren(el("div", "hub-report-pair"), dashboardPie(coverage, usage, dayApps), dashboardApps(dayApps)),
    dashboardSources(coverage)];
}

function todayPage() {
  const [start, end] = range();
  const devices = state.devices.filter(device => !state.deviceFilter || state.deviceFilter === device.id);
  const rows = state.rows.filter(row => !state.deviceFilter || state.deviceFilter === row.device);
  const appRows = state.appRows.filter(row => !state.deviceFilter || state.deviceFilter === row.device);
  const usage = usageMetrics(rows);
  const kpis = keyFigures(usage, deviceCoverage(devices, rows, start, Math.min(end, Date.now())));
  const apps = aggregateApps(appRows);
  const listing = panel("应用用时", el("small", null, apps.length ? apps.length + " 个应用 · 按时长排序" : "按时长排序"));
  listing.append(withChildren(el("div", "app-head"), el("span", null, "应用 / 明细"), el("span", null, "设备"), el("span", "duration", "用时"), el("span", null, "相对用时")));
  for (const app of apps) {
    const expandable = app.pages.length > 0;
    const group = el(expandable ? "details" : "div", "app-group");
    const docCount = app.pages.filter(page => page.kind !== "unknown").length;
    const glyph = appGlyph(app.name);
    const image = el("span", "app-icon");
    image.append(glyph ? icon(glyph) : document.createTextNode(Array.from(app.name)[0]?.toUpperCase() ?? "·"));
    const disclosure = expandable ? icon("next") : null;
    if (disclosure) disclosure.classList.add("expand-icon");
    const label = withChildren(el("div"), el("strong", null, app.name));
    if (expandable) label.append(el("p", "app-sub", docCount + " 项明细"));
    const title = withChildren(el("div", "app-name"), image, label, disclosure);
    title.title = app.name;
    group.append(withChildren(el(expandable ? "summary" : "div", "app-row"), title, deviceChip(app.devices),
      el("span", "duration", duration(app.duration)), progress(app.duration / apps[0].duration * 100)));
    if (expandable) {
      group.open = state.appsOpen.has(app.id);
      group.addEventListener("toggle", () => { if (group.open) state.appsOpen.add(app.id); else state.appsOpen.delete(app.id); });
      for (const page of app.pages) group.append(withChildren(el("div", "child-row " + page.kind),
        withChildren(el("span", "name"), icon(page.kind === "document" ? "doc" : page.kind), document.createTextNode(page.name)),
        el("span", "duration", duration(page.duration, true))));
    }
    listing.append(group);
  }
  if (!apps.length) listing.append(empty(state.loading ? "正在读取本机记录…" : "当天没有采集到应用使用记录。"));
  const controls = dateTools();
  controls.append(deviceSelect(state.deviceFilter, value => { state.deviceFilter = value; render(); }));
  if (!rows.length && !state.running && !state.loading && state.date === today())
    controls.append(captureControl());
  return [heading("今日复盘", shortDate.format(start), controls), kpis, dailyComposition(start, end, rows, appRows), listing];
}

function ledgerLine(row, type = "div") {
  const info = stateMeta(row);
  const title = row.appName || "—";
  const detail = row.folded ? "已折叠 " + row.folded + " 次短切换" : row.page || "—";
  const line = withChildren(el(type, "ledger-grid ledger-row"), el("span", "range", textTime.format(row.startMs) + " – " + textTime.format(row.endMs)),
    el("span", "duration", duration(row.endMs - row.startMs, true)), deviceChip(new Set([row.device])),
    el("span", null, title), el("span", "faded", detail), el("span", "badge " + info.key, info.label));
  if (row.folded) line.dataset.folded = "";
  line.children[3].title = title;
  line.children[4].title = detail;
  return line;
}
function ledgerPage() {
  const [start, end] = range();
  const part = state.dayPart == null ? null : dayParts([], [], start, end).find(item => item.hour === state.dayPart);
  const scope = state.ledgerScope;
  const appsAvailable = state.devices.some(device => device.hasAppRows);
  const appsMode = state.ledgerApps && appsAvailable;
  const all = (appsMode ? state.appRows : state.rows).filter(row =>
    (!state.ledgerFilter || row.device === state.ledgerFilter) && (!scope || scope.devices.includes(row.device)));
  const scoped = scope ? clipRows(all, scope.startMs, scope.endMs) : part ? clipRows(all, part.from, part.until) : all;
  const limit = 5;
  const filter = appsMode ? "all" : state.filter;
  const visible = filter === "all" ? scoped : scoped.filter(row => stateMeta(row).key === filter);
  const groups = ledgerGroups(visible, !appsMode && state.fold && filter === "all" ? limit * 1000 : 0);
  const fold = el("input");
  fold.type = "checkbox";
  fold.checked = state.fold;
  fold.disabled = filter !== "all" || !limit;
  fold.addEventListener("change", () => { state.fold = fold.checked; render(); });
  const filters = withChildren(el("div", "filters"),
    !appsMode && segment([["all", "全部"], ["active", "正常计时"], ["playback", "视频/放映"], ["idle", "暂停"], ["offline", "离线"], ["gap", "缺口"]], state.filter,
      value => { state.filter = value; render(); }),
    withChildren(el("div", "tools"), deviceSelect(state.ledgerFilter, value => { state.ledgerFilter = value; render(); }),
      part && button(part.name + " ×", () => { state.dayPart = null; render(); }),
      scope && button(scope.label + " ×", () => { state.ledgerScope = null; render(); }),
      !appsMode && withChildren(el("label", "check-label"), fold, document.createTextNode("折叠 <" + limit + "s 短切换"))));
  const sessions = appsAvailable ? segment([["states", "设备记录"], ["apps", "应用会话"]],
    appsMode ? "apps" : "states", value => { state.ledgerApps = value === "apps"; state.filter = "all"; render(); }) : null;
  if (sessions) filters.prepend(sessions);
  const table = el("section", "panel ledger-panel");
  table.append(withChildren(el("div", "ledger-grid table-head"), ...["起止时间", "历时", "设备", "应用", "页面 / 文档", "状态"].map(text => el("span", null, text))));
  if (!groups.length) {
    const text = state.loading ? "正在读取本机记录…" :
      filter === "gap" ? "该范围没有采集缺口。" :
        appsMode ? "当前范围没有应用会话记录。" : "当前筛选下没有记录。";
    table.append(empty(text));
  }
  for (const group of groups) {
    if (!group.folded) table.append(ledgerLine(group));
    else {
      const details = el("details");
      const id = group.device + ":" + group.startMs;
      details.open = state.ledgerOpen.has(id);
      details.addEventListener("toggle", () => { if (details.open) state.ledgerOpen.add(id); else state.ledgerOpen.delete(id); });
      details.append(ledgerLine(group, "summary"), withChildren(el("div", "ledger-children"), ...group.parts.map(row => ledgerLine(row))));
      table.append(details);
    }
  }
  table.append(el("div", "ledger-footer", visible.length + " 段原始记录" + (visible.length !== groups.length ? " · " + groups.length + " 行显示" : "")));
  const origin = state.ledgerOrigin === "ledger" ? "dashboard" : state.ledgerOrigin;
  const originLabel = views.find(view => view[0] === origin)?.[1] ?? "概览";
  const controls = dateTools();
  controls.prepend(button("返回" + originLabel, () => selectView(origin), "", "prev"));
  return [heading("记录明细", shortDate.format(start), controls), filters, table];
}

function keyFigures(data, coverage) {
  const kpis = el("dl", "kpis");
  const deviceCount = coverage.length;
  const activeDevice = coverage[0];
  const figures = deviceCount === 1 ? [
    ["活跃用时", data.total, "本设备的已记录活跃时长"],
    ["暂停", activeDevice.known ? activeDevice.idle : null, "已记录的无操作时段"],
    ["离线", activeDevice.known ? activeDevice.offline : null, "已记录的锁屏或离线时段"],
    ["采集覆盖", activeDevice.elapsed ? (activeDevice.known / activeDevice.elapsed * 100).toFixed(1) + "%" : "—",
      "已观测用时 / 已过去的用时"]
  ] : [
    ["去重用时", data.total, "已记录的活跃区间取并集"],
    ["设备累计", data.deviceSum, "各台设备活跃用时相加"],
    ["同时使用", deviceCount > 1 ? data.overlap : null, "有至少两台所选设备同时活跃的时长"],
    ["活跃设备", data.total == null ? "—" : data.byDevice.size + " / " + deviceCount, "有活跃记录的设备 / 所选设备"]
  ];
  for (const [title, value, hint] of figures) {
    const entry = withChildren(el("div", "kpi"), el("dt", null, title),
      el("dd", null, typeof value === "string" ? value : duration(value)));
    entry.title = hint;
    kpis.append(entry);
  }
  return kpis;
}

function chart(days, devices) {
  const mode = devices.length < 2 ? "devices" : state.chartMode;
  const byDevice = mode === "devices";
  const root = panel("每日趋势", devices.length > 1 ? segment([["devices", "按设备"], ["unique", "去重与重叠"]], mode,
    value => { state.chartMode = value; render(); }) : null);
  const marks = el("div", "legend chart-legend");
  const series = byDevice ? devices.map(device => [deviceColor(device.id), deviceName(device.id)]) :
    [["var(--blue)", "去重用时"], ["var(--teal)", "其中同时使用"]];
  for (const [color, name] of series) {
    const dot = el("i");
    dot.style.background = color;
    marks.append(withChildren(el("span"), dot, document.createTextNode(name)));
  }
  root.append(marks);
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  const left = 48, right = 738, top = 8, bottom = 143;
  svg.setAttribute("viewBox", "0 0 750 166");
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", byDevice ? "按设备累计时长" : "去重与同时使用时长");
  const max = Math.max(1, ...days.map(day => (byDevice ? day.deviceSum : day.total) ?? 0));
  const unit = max <= 3600000 ? 900000 : max <= 14400000 ? 3600000 : 7200000;
  const stride = Math.max(unit, Math.ceil(max / (unit * 4)) * unit);
  const ceiling = stride * 4;
  const add = (tag, props, text, parent = svg) => {
    const node = document.createElementNS(svg.namespaceURI, tag);
    for (const [key, value] of Object.entries(props)) node.setAttribute(key, value);
    if (text != null) node.textContent = text;
    parent.append(node);
    return node;
  };
  for (let i = 0; i <= 4; i++) {
    const y = bottom - i * (bottom - top) / 4;
    add("line", { x1: left, x2: right, y1: y, y2: y, class: "gridline" });
    add("text", { x: left - 7, y: y + 4, "text-anchor": "end" }, i === 0 ? "0" : duration(stride * i));
  }
  const step = (right - left) / days.length;
  days.forEach((day, index) => {
    const center = left + (index + .5) * step;
    const width = Math.min(34, step * .64);
    const values = byDevice ? devices.map(device => [deviceName(device.id), day.byDevice.get(device.id) ?? 0, deviceColor(device.id)]) :
      [["单设备活跃", (day.total ?? 0) - (day.overlap ?? 0), "var(--blue)"], ["同时使用", day.overlap ?? 0, "var(--teal)"]];
    const bars = add("g", {});
    const tooltip = byDevice ? devices.map(device => deviceName(device.id) + " " +
      duration(day.knownDevices.has(device.id) ? day.byDevice.get(device.id) ?? 0 : null)).join(" · ") :
      "去重 " + duration(day.total) + " · 同时 " + duration(day.overlap);
    add("title", {}, day.key + " · " + tooltip, bars);
    let y = bottom;
    for (const [, value, color] of values) {
      if (!value) continue;
      const height = value / ceiling * (bottom - top);
      y -= height;
      add("rect", { x: center - width / 2, y, width, height, fill: color }, null, bars);
    }
    const future = +dateFromKey(day.key) > Date.now();
    if (day.total == null && !future)
      add("line", { x1: center - 3, x2: center + 3, y1: bottom - 1, y2: bottom - 1, class: "missing", "stroke-dasharray": "2 1" }, null, bars);
    if (days.length <= 7 || index % 3 === 0 || index === days.length - 1)
      add("text", { x: center, y: bottom + 15, "text-anchor": "middle", class: future ? "future-day" : "" }, day.label + " 日");
  });
  root.append(withChildren(el("div", "chart"), svg));
  return root;
}

function reportTable(title, labels, rows, subtitle) {
  const root = panel(title, subtitle && el("small", null, subtitle));
  const headers = withChildren(el("tr"), ...labels.map(name => {
    const heading = el("th", null, name);
    heading.scope = "col";
    return heading;
  }));
  const body = el("tbody");
  for (const row of rows) {
    const line = el("tr");
    for (const value of row) {
      line.append(value instanceof Node ? withChildren(el("td"), value) : el("td", null, value));
    }
    body.append(line);
  }
  root.append(withChildren(el("div", "report-scroll"), withChildren(el("table", "report-table"),
    withChildren(el("thead"), headers), body)));
  return root;
}

function rankTable(title, rows, total, missing) {
  const root = panel(title);
  if (!rows.length) return withChildren(root, empty(missing));
  const addRows = (parent, entries, from) => {
    for (const [index, row] of entries.entries()) {
      const name = el("span", "rank-name", row.name);
      name.title = row.name;
      const value = el("span", "duration", duration(row.duration, true));
      if (total) value.title = (row.duration / total * 100).toFixed(1) + "%";
      parent.append(withChildren(el("div", "rank-row"), el("small", null, String(index + from + 1)), name, value));
    }
  };
  addRows(root, rows.slice(0, 8), 0);
  if (rows.length > 8) {
    const more = withChildren(el("details", "rank-more"), el("summary", null, "查看其余 " + (rows.length - 8) + " 项"));
    addRows(more, rows.slice(8), 8);
    root.append(more);
  }
  return root;
}

function hourProfile(hours) {
  const key = legend([["no-observation", "无观测"], ["hour-low", "活跃占比低"], ["hour-high", "高"]]);
  const root = panel("小时活跃分布", key);
  const bands = el("div", "hour-bands");
  for (const [hour, values] of hours.entries()) {
    const name = String(hour).padStart(2, "0") + ":00";
    const ratio = values.knownMs ? values.active / values.knownMs : null;
    const tile = el("div", "hour-tile" + (ratio == null ? " no-observation" : ""));
    if (ratio != null) tile.style.background = "color-mix(in srgb,var(--blue) " + Math.round(8 + 83 * ratio) + "%,var(--base))";
    tile.title = name + " · " + (ratio == null ? "无观测" :
      duration(values.active, true) + " 活跃 / " + duration(values.knownMs, true) + " 已观测 · " + values.samples + " 天");
    const band = withChildren(el("div", "hour-band"), tile, el("small", null, String(hour).padStart(2, "0")));
    band.setAttribute("role", "img");
    band.setAttribute("aria-label", tile.title);
    bands.append(band);
  }
  return withChildren(root, bands);
}

function summaryPage() {
  const [start, end] = range();
  const devices = state.devices.filter(device => !state.deviceFilter || device.id === state.deviceFilter);
  const rows = clipRows(state.rows.filter(row => !state.deviceFilter || row.device === state.deviceFilter), start, Math.min(end, Date.now()));
  const apps = aggregateApps(state.appRows.filter(row => !state.deviceFilter || row.device === state.deviceFilter));
  const data = usageMetrics(rows);
  const { days, hours } = distribution(rows, start, end);
  const pages = kind => {
    const ranked = new Map();
    for (const app of apps) for (const page of app.pages) {
      if (page.kind !== kind) continue;
      const id = kind === "site" ? page.name : JSON.stringify([app.id, page.name]);
      const existing = ranked.get(id) ?? { name: page.name, duration: 0 };
      existing.duration += page.duration;
      if (kind === "document") existing.name = page.name + " · " + app.name;
      ranked.set(id, existing);
    }
    return [...ranked.values()].sort((a, b) => b.duration - a.duration);
  };
  const tools = withChildren(el("div", "tools"),
    segment([["week", "周"], ["month", "月"]], state.period, value => {
      if (state.period !== value) { state.period = value; state.rows = []; state.appRows = []; render(); load(); }
    }),
    deviceSelect(state.deviceFilter, value => { state.deviceFilter = value; render(); }), dateTools());
  const coverage = deviceCoverage(devices, rows, start, Math.min(end, Date.now()));
  const coverageTable = reportTable("设备用时与覆盖", ["设备", "活跃", "暂停", "离线", "缺口", "无记录", "覆盖率"],
    coverage.map(item => [deviceName(item.device.id), item.known ? duration(item.active, true) : "—", duration(item.idle, true),
      duration(item.offline, true), duration(item.gap, true), duration(item.unobserved, true),
      item.elapsed ? (item.known / item.elapsed * 100).toFixed(1) + "%" : "—"]));
  const pairs = data.pairs.map(pair => [pair.devices.map(deviceName).join(" · "), duration(pair.ms, true)]);
  const concurrent = [...data.concurrency].filter(([count]) => count > 1).sort(([a], [b]) => a - b);
  const multiple = devices.length > 1;
  const overlapTable = !multiple ? null : pairs.length ? reportTable("设备同时使用", ["设备组合", "共同活跃"],
    pairs, concurrent.map(([count, ms]) => count + " 台同时 " + duration(ms, true)).join(" · ")) :
    withChildren(panel("设备同时使用"), empty(data.total == null ? "此范围没有可对照的记录。" : "已记录区间没有同时活跃。"));
  const drilldown = day => {
    openRecordDetail({ date: day.key, apps: true, deviceId: state.deviceFilter });
  };
  const dailyHeaders = multiple ? ["日期", "去重用时", "设备累计", "同时使用", ...devices.map(device => deviceName(device.id))] : ["日期", "活跃用时"];
  const dailyTable = reportTable("每日明细", dailyHeaders,
    days.filter(day => +dateFromKey(day.key) <= Date.now()).map(day => [
      button(day.key, () => drilldown(day), "report-link"), duration(day.total, true),
      ...(multiple ? [duration(day.deviceSum, true), duration(day.overlap, true),
        ...devices.map(device => duration(day.knownDevices.has(device.id) ? day.byDevice.get(device.id) ?? 0 : null, true))] : [])
    ]), "选日期查看记录明细");
  return [heading("趋势", dayKey(new Date(start)) + " 至 " + dayKey(new Date(end - 1)), tools),
    keyFigures(data, coverage), chart(days, devices), hourProfile(hours),
    withChildren(el("div", "ranks"),
      rankTable("应用总榜", apps, apps.reduce((sum, app) => sum + app.duration, 0), "此范围没有应用记录。"),
      rankTable("文档与项目榜", pages("document"), null, "此范围没有已识别文档。"),
      rankTable("高频站点榜", pages("site"), null, "此范围没有站点记录。")),
    coverageTable, overlapTable, dailyTable].filter(Boolean);
}

function byteSize(bytes) {
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1048576) return (bytes / 1024).toFixed(1) + " KB";
  return (bytes / 1048576).toFixed(1) + " MB";
}
const formatUpdated = ms => new Intl.DateTimeFormat("zh-CN", { month: "long", day: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(new Date(ms));
function statusPill(text, tone = "off") {
  return withChildren(el("span", "status-pill " + tone), el("i", "status-dot"), el("span", null, text));
}
function settingsStatusCard({ title, value, tone, description, meta, actions = [] }) {
  return withChildren(el("section", "panel settings-status"),
    withChildren(el("div", "settings-status-head"), el("h2", null, title), statusPill(value, tone)),
    el("p", "settings-status-desc", description),
    meta && el("p", "settings-status-meta", meta),
    actions.length && withChildren(el("div", "settings-status-actions"), ...actions));
}
function captureControl() {
  const capture = button(state.running ? "停止采集" : "开始采集", async () => {
    capture.disabled = true;
    try {
      await invoke(state.running ? "stop_collector" : "start_collector");
      await new Promise(resolve => setTimeout(resolve, 900));
      await load(true);
    } catch (error) { message(String(error)); }
    finally { capture.disabled = false; }
  }, state.running ? "" : "primary");
  capture.dataset.captureControl = "";
  return capture;
}
function collectorCard() {
  return settingsStatusCard({
    title: "本机采集",
    value: state.running ? "运行中" : "已停止",
    tone: state.running ? "on" : "off",
    description: "记录本机前台应用、空闲与锁屏状态，是仪表盘的主要数据来源。",
    actions: [captureControl()]
  });
}
function browserCard() {
  const connected = !!state.settings?.edgeConnected;
  const running = state.running;
  const setup = state.browserSetup;
  const card = el("section", "panel settings-status settings-browser-card");
  card.dataset.connected = String(connected);
  card.dataset.running = String(running);
  card.append(
    withChildren(el("div", "settings-status-head"), el("h2", null, "浏览器扩展"),
      statusPill(connected ? "已连接" : running ? "未连接" : "待采集启动", connected ? "on" : "off")),
    el("p", "settings-status-desc", connected
      ? "网页标题由 Tickmark 扩展补充，具体页面会出现在「文档与项目榜」。"
      : running
        ? "尚未连接 Edge 扩展。按下方步骤加载扩展后，这里会在几秒内自动变为已连接。"
        : "开始采集后，Edge 扩展会自动连接并补充网页标题。"));
  if (connected) return card;

  const run = command => invoke(command).catch(error => message(error?.message || String(error)));
  const details = el("details", "settings-browser-help");
  details.append(el("summary", null, "如何连接"));
  const steps = el("ol", "settings-browser-steps");
  steps.append(
    el("li", null, "在 Edge 扩展页开启「开发人员模式」。"),
    el("li", null, "选择「加载解压缩的扩展」，选中扩展文件夹。"),
    el("li", null, "保持采集运行，几秒后这里会自动显示已连接。"));
  details.append(steps);
  if (!setup) details.append(el("p", "settings-browser-host", "正在检查本机通信…"));
  else {
    const host = el("p", "settings-browser-host" + (setup.registered && setup.hostExists ? "" : " warn"));
    host.textContent = setup.registered && setup.hostExists
      ? "本机通信已注册"
      : setup.registered ? "本机通信已注册，但通信程序缺失。" : "本机通信尚未注册。";
    details.append(host);
    if (!setup.registered || !setup.hostExists) {
      const register = button("注册本机通信", async () => {
        register.disabled = true;
        try {
          await invoke("register_browser_host");
          state.browserSetup = await invoke("browser_setup").catch(() => state.browserSetup);
          card.replaceWith(browserCard());
          message("本机通信已注册，请在 Edge 中重新加载扩展。");
        } catch (error) {
          message(error?.message || String(error));
          register.disabled = false;
        }
      }, "settings-browser-register");
      details.append(register);
    }
    details.append(el("p", "settings-browser-path", "扩展目录：" + setup.extensionDir));
  }
  card.append(details, withChildren(el("div", "settings-status-actions settings-browser-actions"),
    button("打开扩展文件夹", () => run("open_extension_folder"), "settings-browser-action"),
    button("打开 Edge 扩展页", () => run("open_edge_extensions"), "settings-browser-action"),
    button("重新检测", async event => {
      const control = event.currentTarget;
      control.disabled = true;
      try {
        const [value, setup] = await Promise.all([
          invoke("edge_connected"),
          invoke("browser_setup").catch(() => null)
        ]);
        if (setup) state.browserSetup = setup;
        if (state.settings) state.settings.edgeConnected = value;
        card.replaceWith(browserCard());
        if (!value) message("仍未检测到浏览器扩展连接。");
      } catch (error) { message(error?.message || String(error)); }
      finally { control.disabled = false; }
    }, "settings-browser-recheck")));
  return card;
}
function accountCard() {
  const account = state.account;
  if (!account) return settingsStatusCard({
    title: "账号与同步", value: "读取中", tone: "off", description: "正在读取账号状态…"
  });
  if (account.error) return settingsStatusCard({
    title: "账号与同步", value: "读取失败", tone: "off",
    description: "暂时无法读取账号状态，不影响本机采集和手动导入。"
  });
  if (account.configured === false) return settingsStatusCard({
    title: "账号与同步", value: "未启用", tone: "off",
    description: "此版本未连接账号服务，其他设备的报告需要在下方手动导入。"
  });
  const signedIn = !!account.account;
  return settingsStatusCard({
    title: "账号与同步",
    value: signedIn ? (account.offline ? "离线" : "已登录") : "未登录",
    tone: signedIn ? (account.offline ? "warn" : "on") : "off",
    description: signedIn
      ? "其他设备的报告会自动同步到本机，仪表盘会合并它们的时长与应用。"
      : "登录后，其他设备的报告会自动同步到本机；不登录也可以手动导入报告。",
    meta: signedIn ? account.account.email : null,
    actions: [button(signedIn ? "管理账号" : "登录账号",
      () => get(signedIn ? "account-open" : "account-login").click(), signedIn ? "" : "primary")]
  });
}
function dataCard() {
  const data = state.settings;
  const card = panel("数据与设备");
  card.append(withChildren(el("div", "settings-line"),
    withChildren(el("div", "settings-line-main"),
      el("strong", null, "本机数据库"),
      el("small", null, "记录保存在这台电脑上；其他设备通过账号同步或手动导入汇总到这里。")),
    el("span", "settings-line-value", data ? byteSize(data.databaseBytes) : "读取中…")));

  const picker = el("input");
  picker.type = "file";
  picker.accept = ".json,application/json";
  picker.hidden = true;
  const importButton = button("导入设备报告", () => picker.click());
  picker.addEventListener("change", async () => {
    const [file] = picker.files;
    if (!file) return;
    importButton.disabled = true;
    try {
      if (file.size > 16 * 1024 * 1024) throw Error("报告超过 16 MiB，请缩小导出日期范围。");
      const name = await invoke("import_device_report", { content: await file.text() });
      await load(true);
      message("已导入 " + name + " 的使用记录。");
    } catch (error) { message(error?.message || String(error)); }
    finally { picker.value = ""; importButton.disabled = false; }
  });
  card.append(picker, withChildren(el("div", "settings-subhead"),
    withChildren(el("div", null),
      el("h3", null, "其他设备"),
      el("p", "settings-hint", "从手机或平板导出的 JSON 报告可以在这里合并；登录账号时会自动同步。")),
    importButton));

  const others = state.devices.filter(device => !device.local);
  if (others.length) {
    const list = el("div", "device-list");
    for (const device of others) {
      const updated = device.updatedAtMs != null ? "更新于 " + formatUpdated(device.updatedAtMs) : "";
      list.append(withChildren(el("div", "device-item"),
        withChildren(el("span", "device-icon"), icon(device.kind === "phone" || device.kind === "tablet" ? "phone" : "computer")),
        withChildren(el("div", "device-copy"),
          el("strong", null, deviceName(device.id)),
          el("small", null, [device.platform, updated].filter(Boolean).join(" · ")))));
    }
    card.append(list);
  } else card.append(empty("还没有其他设备。"));
  if (state.rejectedSources) card.append(el("p", "settings-warning", state.rejectedSources + " 份报告未通过校验，已跳过。"));
  return card;
}
function diagnosticsCard() {
  const card = panel("数据与质量");
  card.append(withChildren(el("div", "settings-line"),
    withChildren(el("div", "settings-line-main"),
      el("strong", null, "原始记录"),
      el("small", null, "核对起止时间、采集缺口与未识别应用；日常查看不需要打开。")),
    withChildren(el("div", "settings-line-actions"),
      button("查看原始记录", () => openRecordDetail({ date: today(), apps: true, deviceId: "" })),
      button("查看采集缺口", () => openRecordDetail({ date: today(), filter: "gap", apps: false, deviceId: "" })))));
  return card;
}
function documentsCard() {
  const data = state.settings;
  const card = panel("文档与项目记录");
  card.append(withChildren(el("div", "settings-intro"),
    el("p", null, "开启后，Tickmark 会读取这些应用窗口的标题（例如文档名、项目名），用于生成「文档与项目榜」。关闭只影响标题记录，不影响时长统计。")));
  if (!data) {
    card.append(empty("正在读取应用列表…"));
    return card;
  }
  const apps = data.apps ?? [];
  if (!apps.length) {
    card.append(empty("还没有采集到应用。开始采集后，这里会列出可以记录标题的应用。"));
    return card;
  }
  const eligible = apps.filter(app => app.eligible);
  const browsers = apps.filter(app => !app.eligible);
  const count = el("span", "settings-count");
  const list = el("div", "settings-app-list");
  list.tabIndex = 0;
  list.setAttribute("aria-label", "应用列表");
  const search = el("input", "settings-search");
  search.type = "search";
  search.placeholder = "搜索应用";
  search.setAttribute("aria-label", "搜索应用");
  let bulkBusy = false;

  function update() {
    const on = eligible.filter(app => app.enabled).length;
    count.textContent = "已开启 " + on + " / " + eligible.length + " 个应用";
    bulkOn.disabled = bulkBusy || on === eligible.length;
    bulkOff.disabled = bulkBusy || on === 0;
  }
  async function setAll(value) {
    if (bulkBusy) return;
    bulkBusy = true;
    update();
    let failures = 0;
    for (const app of eligible.filter(app => app.enabled !== value)) {
      try {
        await invoke("set_document_capture", { appId: app.id, enabled: value });
        app.enabled = value;
      } catch { failures++; }
    }
    bulkBusy = false;
    update();
    renderList();
    if (failures) message(failures + " 个应用的设置未能保存。");
  }
  function row(app) {
    const node = withChildren(el("div", "settings-app"),
      el("span", "settings-app-avatar", (app.name.trim()[0] || "?").toUpperCase()),
      withChildren(el("div", "settings-app-name"), el("strong", null, app.name),
        app.eligible ? null : el("small", null, "页面标题由浏览器扩展采集")));
    if (!app.eligible) {
      node.append(el("span", "settings-badge", "扩展采集"));
      return node;
    }
    const control = el("button", "switch");
    control.type = "button";
    control.setAttribute("role", "switch");
    control.setAttribute("aria-checked", String(app.enabled));
    control.setAttribute("aria-label", "记录 " + app.name + " 的文档标题");
    control.append(el("span", "switch-knob"));
    control.addEventListener("click", async () => {
      const next = !app.enabled;
      control.setAttribute("aria-checked", String(next));
      control.disabled = true;
      try {
        await invoke("set_document_capture", { appId: app.id, enabled: next });
        app.enabled = next;
        update();
      } catch (error) {
        control.setAttribute("aria-checked", String(app.enabled));
        message(error?.message || String(error));
      } finally { control.disabled = false; }
    });
    node.append(control);
    return node;
  }
  function syncScroll() {
    const overflow = list.scrollHeight > list.clientHeight + 1;
    list.classList.toggle("scrollable", overflow);
    list.classList.toggle("at-end", !overflow || list.scrollTop + list.clientHeight >= list.scrollHeight - 2);
  }
  function renderList() {
    const query = search.value.trim().toLowerCase();
    const match = app => !query || app.name.toLowerCase().includes(query);
    const nodes = eligible.filter(match).map(row);
    const hitBrowsers = browsers.filter(match);
    if (hitBrowsers.length) {
      nodes.push(el("p", "settings-group", "浏览器 · 由扩展采集标题"));
      nodes.push(...hitBrowsers.map(row));
    }
    list.replaceChildren(...(nodes.length ? nodes : [empty("没有匹配的应用。")]));
    syncScroll();
  }

  const bulkOn = button("全部开启", () => setAll(true), "settings-bulk");
  const bulkOff = button("全部关闭", () => setAll(false), "settings-bulk");
  card.append(withChildren(el("div", "settings-toolbar"), count, bulkOn, bulkOff, search), list);
  search.addEventListener("input", renderList);
  list.addEventListener("scroll", syncScroll, { passive: true });
  update();
  renderList();
  requestAnimationFrame(() => { if (list.isConnected) syncScroll(); });
  return card;
}
function settingsPage() {
  return [
    heading("设置", "管理采集、数据来源与文档记录"),
    withChildren(el("div", "settings-stack"),
      withChildren(el("div", "settings-status-grid"), collectorCard(), browserCard(), accountCard()),
      dataCard(),
      diagnosticsCard(),
      documentsCard())
  ];
}

function render() {
  const page = get("page");
  const scroll = page.scrollTop;
  const renderer = { dashboard: dashboardPage, today: todayPage, ledger: ledgerPage, summary: summaryPage, settings: settingsPage }[state.view];
  page.dataset.view = state.view;
  page.replaceChildren(...renderer());
  page.scrollTop = scroll;
}
function smoothRender() {
  if (typeof document.startViewTransition === "function") {
    try {
      document.startViewTransition(() => render());
      return;
    } catch {
      /* fall through to an immediate render */
    }
  }
  render();
}
async function load(force = false) {
  const request = ++state.generation;
  const view = state.view;
  const [start, end] = loadRange();
  state.loading = true;
  try {
    const [running, data, settings, account, browserSetup] = await Promise.all([
      invoke("collector_running"),
      invoke("dashboard_intervals", { start, end }),
      view === "settings" || !state.settings ? invoke("dashboard_settings") : null,
      view === "settings" ? invoke("account_status").catch(() => ({ error: true })) : null,
      view === "settings" ? invoke("browser_setup").catch(() => null) : null
    ]);
    if (request !== state.generation) return;
    const changedStatus = state.running !== running;
    state.running = running;
    renderStatus();
    if (settings) state.settings = settings;
    if (account) state.account = account;
    if (browserSetup) state.browserSetup = browserSetup;
    if (data) {
      state.devices = data.devices.map(({ rows, appRows, ...device }) => ({ ...device, hasAppRows: appRows != null }));
      state.rows = clipRows(data.devices.flatMap(device => device.rows.map(row => ({ ...row, device: device.id }))), start, Math.min(end, Date.now()))
        .sort((a, b) => a.startMs - b.startMs || a.endMs - b.endMs);
      state.appRows = clipRows(data.devices.flatMap(device => (device.appRows ?? device.rows).map(row => ({ ...row, device: device.id }))),
        start, Math.min(end, Date.now())).sort((a, b) => a.startMs - b.startMs || a.endMs - b.endMs);
      buildAppColors(state.appRows);
      state.rejectedSources = data.rejectedSources;
      for (const key of ["deviceFilter", "ledgerFilter"]) {
        if (!state.devices.some(device => device.id === state[key])) state[key] = "";
      }
      const hub = state.dashboard;
      if (hub.focus) {
        const [start, end] = dayRange(state.date);
        if (hub.focus.startMs < start || hub.focus.endMs > end || hub.focus.endMs <= hub.focus.startMs) hub.focus = null;
      }
    }
    const fingerprint = JSON.stringify([view, state.date, state.period, data, settings, account, browserSetup]);
    const changed = fingerprint !== lastResult;
    lastResult = fingerprint;
    state.loading = false;
    if (force || changed || changedStatus) render();
  } catch (error) {
    if (request !== state.generation) return;
    state.loading = false;
    message(String(error));
  }
}

get("brandIcon").append(icon("chart"));
get("lockIcon").append(icon("lock"));
get("collector-status").addEventListener("click", async () => {
  if (state.running) { selectView("settings"); return; }
  const node = get("collector-status");
  node.disabled = true;
  try {
    await invoke("start_collector");
    await new Promise(resolve => setTimeout(resolve, 900));
    await load(true);
    message("已开始本机采集");
  } catch (error) {
    message(String(error));
  } finally {
    node.disabled = false;
  }
});
connectAccount(invoke, {
  openSettings: () => selectView("settings"),
  signedIn: () => message("已登录 Tickmark"),
  accountChanged: () => load(true)
}, icon);
document.addEventListener("keydown", event => {
  if (event.key !== "Escape" || !state.dashboard.focus) return;
  if (state.view !== "dashboard" || document.querySelector("dialog[open]")) return;
  event.preventDefault();
  state.dashboard.focus = null;
  smoothRender();
});
renderRail();
render();
load(true);
setInterval(() => {
  if (state.loading) return;
  if (state.view === "settings" || get("page").matches(":focus-within")) {
    invoke("collector_running").then(running => {
      state.running = running;
      renderStatus();
      const card = document.querySelector(".settings-browser-card");
      if (card && card.dataset.running !== String(running)) card.replaceWith(browserCard());
    }).catch(() => {});
    const edgeCard = document.querySelector(".settings-browser-card");
    if (edgeCard) invoke("edge_connected").then(connected => {
      if (state.settings) state.settings.edgeConnected = connected;
      const card = document.querySelector(".settings-browser-card");
      if (!card || card.dataset.connected === String(connected)) return;
      state.browserSetup = null;
      invoke("browser_setup").catch(() => null).then(setup => {
        if (setup) state.browserSetup = setup;
        document.querySelector(".settings-browser-card")?.replaceWith(browserCard());
      });
    }).catch(() => {});
  } else {
    load();
  }
}, 4000);
