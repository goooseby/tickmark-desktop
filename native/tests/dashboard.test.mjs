import test from "node:test";
import assert from "node:assert/strict";
import { aggregateApps, clipRows, dayParts, deviceCoverage, distribution, ledgerGroups, usageMetrics,
  coalesceTimelineItems, activeSpans, quietSpans, buildTimelineScale } from "../web/report.mjs";

const span = (startMs, endMs, appId, device = "windows", extra = {}) =>
  ({ startMs, endMs, state: 0, appId, appName: appId, device, ...extra });

test("three-device concurrency deduplicates total and overlap while preserving pair and device time", () => {
  const rows = [span(0, 10, "editor", "desk"), span(3, 9, "chat", "phone"), span(5, 11, "pdf", "tablet")];
  const result = usageMetrics(rows);
  assert.equal(result.total, 11);
  assert.equal(result.overlap, 7);
  assert.equal(result.deviceSum, 22);
  assert.deepEqual([...result.concurrency], [[1, 4], [2, 3], [3, 4]]);
  assert.deepEqual(result.pairs, [
    { devices: ["desk", "phone"], ms: 6 }, { devices: ["desk", "tablet"], ms: 5 },
    { devices: ["phone", "tablet"], ms: 4 }
  ]);
  assert.equal(usageMetrics([span(0, 5, "a"), span(5, 8, "b", "phone")]).overlap, 0);
  assert.equal(usageMetrics([span(0, 5, "a"), span(2, 8, "a")]).deviceSum, 8);
});

test("dashboard overlap selections preserve exactly the devices active in each range", () => {
  const rows = [span(0, 5, "edge", "desk"), span(5, 10, "code", "desk"),
    span(1, 9, "video", "phone"), span(6, 11, "notes", "tablet")];
  const { overlap, overlapSpans } = usageMetrics(rows, true);
  assert.equal(overlap, 9);
  assert.deepEqual(overlapSpans, [
    { startMs: 1, endMs: 6, devices: ["desk", "phone"] },
    { startMs: 6, endMs: 9, devices: ["desk", "phone", "tablet"] },
    { startMs: 9, endMs: 10, devices: ["desk", "tablet"] }
  ]);
});

test("coverage distinguishes explicit idle, explicit gap, and missing intervals", () => {
  const rows = [{ ...span(10, 20, null), state: 1 }, { ...span(20, 30, null), state: 3 }];
  const [first, second] = deviceCoverage([{ id: "windows" }, { id: "tablet" }], rows, 0, 40);
  assert.deepEqual([first.active, first.idle, first.gap, first.unobserved, first.known], [0, 10, 10, 20, 10]);
  assert.deepEqual([second.elapsed, second.known, second.unobserved], [40, 0, 40]);
  assert.equal(usageMetrics(rows).total, 0);
  assert.equal(usageMetrics(rows.slice(1)).total, null);
  assert.deepEqual(clipRows(rows, 5, 4), []);
});

test("daily and hour distributions clip crossing records and keep observed-zero buckets", () => {
  const start = +new Date(2026, 10, 22);
  const end = +new Date(2026, 10, 24);
  const midnight = +new Date(2026, 10, 23);
  const rows = [span(midnight - 60000, midnight + 60000, "chat"),
    span(midnight, midnight + 60000, "video", "tablet"),
    { ...span(midnight + 3600000, midnight + 3660000, null), state: 1 }];
  const { days, hours } = distribution(rows, start, end);
  assert.deepEqual(days.map(day => day.total), [60000, 60000]);
  assert.deepEqual(hours[0], { active: 60000, knownMs: 60000, samples: 1 });
  assert.deepEqual(hours[1], { active: 0, knownMs: 60000, samples: 1 });
});

test("a half-hour clock change preserves the local hour bucket", () => {
  const prior = process.env.TZ;
  process.env.TZ = "Australia/Lord_Howe";
  try {
    const day = +new Date(2026, 9, 4);
    const next = +new Date(2026, 9, 5);
    const rows = [span(day, next, "editor")];
    const { hours } = distribution(rows, day, next);
    assert.equal(hours[2].knownMs, 1800000);
    assert.equal(hours[3].knownMs, 3600000);
    assert.equal(hours.reduce((sum, hour) => sum + hour.active, 0), next - day);
  } finally {
    if (prior === undefined) delete process.env.TZ;
    else process.env.TZ = prior;
  }
});

test("day-part reports split at local 12:00 and preserve concurrent apps separately", () => {
  const start = +new Date(2026, 10, 24);
  const noon = +new Date(2026, 10, 24, 12);
  const rows = [span(noon - 60000, noon + 60000, null, "tablet")];
  const sessions = [span(noon - 60000, noon + 60000, "reader", "tablet"),
    span(noon, noon + 60000, "notes", "tablet")];
  const [night, morning, afternoon, evening] = dayParts(rows, sessions, start, noon + 60000);
  assert.equal(night.usage.total, null);
  assert.equal(morning.apps[0].duration, 60000);
  assert.equal(afternoon.usage.total, 60000);
  assert.equal(afternoon.apps.reduce((sum, app) => sum + app.duration, 0), 120000);
  assert.equal(evening.future, true);
});

test("pages add up to the application duration after clipping", () => {
  const rows = [
    span(0, 8, "editor", "windows", { page: "notes.md" }),
    span(8, 16, "editor"),
    span(16, 30, "editor", "windows", { page: "other.md" })
  ];
  const [app] = aggregateApps(clipRows(rows, 5, 21));
  assert.equal(app.duration, 16);
  assert.equal(app.pages.reduce((sum, page) => sum + page.duration, 0), app.duration);
  assert.deepEqual(app.pages.at(-1), { name: "页面未识别", kind: "unknown", duration: 8 });
});

test("only short returns on the same device fold, preserving raw intervals", () => {
  const rows = [
    span(0, 100, "editor"), span(50, 200, "chat", "android"),
    span(100, 104, "chat"), span(104, 120, "editor"),
    span(120, 122, "terminal"), span(122, 150, "editor")
  ];
  const [group, phone] = ledgerGroups(rows, 5);
  assert.equal(group.folded, 2);
  assert.equal(group.parts.length, 5);
  assert.equal(phone.device, "android");
  assert.equal(ledgerGroups(rows, 0).length, rows.length);
  assert.equal(ledgerGroups([span(0,10,"editor"), span(10,15,"chat"), span(15,20,"editor")], 5).length, 3);
});

test("an app only shows an unidentified remainder when it has identified details", () => {
  const [edge] = aggregateApps([span(0, 6000, "Edge"), span(6000, 9000, "Edge")]);
  assert.equal(edge.duration, 9000);
  assert.deepEqual(edge.pages, []);
  const site = { page: "example.org", pageKind: "site" };
  assert.deepEqual(aggregateApps([span(0, 6000, "Edge", "windows", site)])[0].pages,
    [{ name: "example.org", kind: "site", duration: 6000 }]);
});

test("display smoothing joins capture flickers but not real app switches", () => {
  const items = [
    { ...span(0, 10, "xhs", "phone"), startMs: 0, endMs: 10_000 },
    { ...span(10, 12, "xhs", "phone"), startMs: 10_000, endMs: 12_000 },
    { ...span(13, 20, "xhs", "phone"), startMs: 13_000, endMs: 20_000 },
    { ...span(20, 25, "chat", "phone"), startMs: 20_000, endMs: 25_000 },
    { ...span(25, 30, "chat", "phone"), startMs: 25_000, endMs: 30_000 },
    { ...span(35, 40, "xhs", "phone"), startMs: 35_000, endMs: 40_000 }
  ];
  const merged = coalesceTimelineItems(items, 2000);
  assert.equal(merged.length, 3);
  assert.deepEqual([merged[0].startMs, merged[0].endMs, merged[0].merged], [0, 20_000, 3]);
  assert.deepEqual([merged[1].startMs, merged[1].endMs, merged[1].merged], [20_000, 30_000, 2]);
  assert.equal(merged[2].startMs, 35_000);
});

test("display smoothing absorbs tiny unidentified gaps and short excursions", () => {
  const items = [
    { ...span(0, 10, "xhs", "phone"), startMs: 0, endMs: 10_000 },
    { ...span(10, 10.4, null, "phone"), startMs: 10_000, endMs: 10_400, appId: null, unidentified: true },
    { ...span(10.4, 20, "xhs", "phone"), startMs: 10_400, endMs: 20_000 },
    { ...span(20, 20.8, "permission", "phone"), startMs: 20_000, endMs: 20_800 },
    { ...span(20.8, 30, "xhs", "phone"), startMs: 20_800, endMs: 30_000 },
    { ...span(30, 40, "edge", "phone"), startMs: 30_000, endMs: 40_000 }
  ];
  const merged = coalesceTimelineItems(items, 2000);
  assert.equal(merged.length, 2);
  assert.deepEqual([merged[0].startMs, merged[0].endMs, merged[0].merged], [0, 30_000, 5]);
  assert.equal(merged[1].appId, "edge");
});

test("quiet spans complement merged active intervals", () => {
  const rows = [span(0, 10, "a", "desk"), span(5, 20, "b", "phone"), span(40, 50, "c", "desk")];
  const active = activeSpans(rows, 0, 60);
  assert.deepEqual(active, [{ startMs: 0, endMs: 20 }, { startMs: 40, endMs: 50 }]);
  assert.deepEqual(quietSpans(active, 0, 60), [
    { startMs: 20, endMs: 40 }, { startMs: 50, endMs: 60 }
  ]);
});

test("timeline scale compresses long quiet spans and keeps time reversible", () => {
  const day = 24 * 3600e3;
  const quiet = [
    { startMs: 0, endMs: 8 * 3600e3, label: "睡眠" },
    { startMs: 10 * 60e3, endMs: 20 * 60e3, label: "短暂停" },
    { startMs: 18 * 3600e3 + 10 * 60e3, endMs: day, label: "未来" }
  ];
  const scale = buildTimelineScale(0, day, quiet);
  assert.equal(scale.compressed.length, 2);
  assert.deepEqual(scale.compressed.map(segment => segment.label), ["睡眠", "未来"]);
  for (const at of [0, 1, 8 * 3600e3, 12 * 3600e3, 18 * 3600e3, day]) {
    assert.ok(Math.abs(scale.fromRatio(scale.toRatio(at)) - at) < 1);
  }
  const sleep = scale.compressed[0];
  assert.ok(sleep.endRatio - sleep.startRatio < 0.1);
  const midday = scale.toRatio(12 * 3600e3);
  const before = scale.toRatio(11 * 3600e3);
  assert.ok(midday > before);
});

test("timeline scale handles an entirely quiet day", () => {
  const day = 24 * 3600e3;
  const scale = buildTimelineScale(0, day, [{ startMs: 0, endMs: day, label: "无活动" }]);
  assert.equal(scale.compressed.length, 1);
  assert.equal(scale.toRatio(0), 0);
  assert.equal(scale.toRatio(day), 1);
  assert.equal(scale.fromRatio(0.5), day / 2);
});
