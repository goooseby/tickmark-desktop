export const STATES = [
  { label: "正常计时", key: "active" },
  { label: "无操作暂停", key: "idle" },
  { label: "锁屏 / 离线", key: "offline" },
  { label: "采集缺口", key: "gap" }
];

export function dayKey(day) {
  return [day.getFullYear(), String(day.getMonth() + 1).padStart(2, "0"), String(day.getDate()).padStart(2, "0")].join("-");
}

export function dateFromKey(key) {
  const [year, month, day] = key.split("-").map(Number);
  return new Date(year, month - 1, day);
}

export function shiftDay(day, amount) {
  return new Date(day.getFullYear(), day.getMonth(), day.getDate() + amount);
}

export function dayRange(key) {
  const date = dateFromKey(key);
  return [+date, +shiftDay(date, 1)];
}

export function periodRange(key, period) {
  const day = dateFromKey(key);
  if (period === "month") return [+new Date(day.getFullYear(), day.getMonth(), 1), +new Date(day.getFullYear(), day.getMonth() + 1, 1)];
  const monday = shiftDay(day, -((day.getDay() + 6) % 7));
  return [+monday, +shiftDay(monday, 7)];
}

export function duration(ms, precise = false) {
  if (ms == null) return "—";
  if (ms > 0 && ms < 1000) return "<1 秒";
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor(s / 60) % 60;
  const parts = [];
  if (h) parts.push(h + " 小时");
  if (m) parts.push(m + " 分");
  if ((!h && !m) || precise && s % 60) parts.push(s % 60 + " 秒");
  return parts.join(" ");
}

export function intervalTotal(rows) {
  return rows.reduce((sum, row) => sum + row.endMs - row.startMs, 0);
}

export function usageMetrics(rows, includeOverlapSpans = false) {
  const active = rows.filter(row => row.state === 0);
  const known = rows.some(row => row.state !== 3);
  const byDevice = new Map();
  const pairMs = new Map();
  const concurrency = new Map();
  const overlapSpans = [];
  const events = active.flatMap(row => [
    [row.startMs, row.device, 1], [row.endMs, row.device, -1]
  ]).sort((a, b) => a[0] - b[0]);
  const devices = new Map();
  let total = 0;
  let overlap = 0;
  let previous = events[0]?.[0];
  for (let i = 0; i < events.length;) {
    const time = events[i][0];
    const span = time - previous;
    if (span && devices.size) {
      const ids = [...devices.keys()].sort();
      total += span;
      concurrency.set(ids.length, (concurrency.get(ids.length) ?? 0) + span);
      if (ids.length > 1) {
        overlap += span;
        if (includeOverlapSpans) {
          const last = overlapSpans.at(-1);
          if (last?.endMs === previous && last.devices.length === ids.length && last.devices.every((id, index) => id === ids[index]))
            last.endMs = time;
          else overlapSpans.push({ startMs: previous, endMs: time, devices: ids });
        }
      }
      for (const id of ids) byDevice.set(id, (byDevice.get(id) ?? 0) + span);
      for (const [j, id] of ids.entries()) for (const other of ids.slice(j + 1)) {
        const key = JSON.stringify([id, other]);
        pairMs.set(key, (pairMs.get(key) ?? 0) + span);
      }
    }
    while (events[i]?.[0] === time) {
      const [, device, change] = events[i++];
      const count = (devices.get(device) ?? 0) + change;
      if (count) devices.set(device, count);
      else devices.delete(device);
    }
    previous = time;
  }
  return {
    total: known ? total : null, overlap: known ? overlap : null, byDevice, concurrency, overlapSpans,
    deviceSum: known ? [...byDevice.values()].reduce((sum, ms) => sum + ms, 0) : null,
    pairs: [...pairMs].map(([key, ms]) => ({ devices: JSON.parse(key), ms })).sort((a, b) => b.ms - a.ms)
  };
}

export function deviceCoverage(devices, rows, start, end) {
  const elapsed = Math.max(0, end - start);
  const tracks = Map.groupBy(clipRows(rows, start, end), row => row.device);
  return devices.map(device => {
    const track = tracks.get(device.id) ?? [];
    const totals = [0, 0, 0, 0];
    for (const row of track) totals[row.state] += row.endMs - row.startMs;
    const [active, idle, offline, gap] = totals;
    return { device, active, idle, offline, gap, unobserved: Math.max(0, elapsed - intervalTotal(track)),
      known: active + idle + offline, elapsed };
  });
}

export function distribution(rows, start, end) {
  const days = [];
  const hours = Array.from({ length: 24 }, () => ({ active: 0, knownMs: 0, samples: 0 }));
  for (let day = new Date(start); +day < end; day = shiftDay(day, 1)) {
    const next = Math.min(+shiftDay(day, 1), end);
    const track = clipRows(rows, +day, next);
    const values = usageMetrics(track);
    const knownDevices = new Set(track.filter(row => row.state !== 3).map(row => row.device));
    const buckets = Array.from({ length: 24 }, () => ({ active: 0, knownMs: 0 }));
    let at = +day;
    while (at < next) {
      const local = new Date(at);
      const boundary = Math.min(next, nextHour(at, local));
      const spans = clipRows(track, at, boundary);
      const bucket = buckets[local.getHours()];
      bucket.active += intervalTotalUnion(spans.filter(row => row.state === 0));
      bucket.knownMs += intervalTotalUnion(spans.filter(row => row.state !== 3));
      at = boundary;
    }
    buckets.forEach((bucket, hour) => {
      if (!bucket.knownMs) return;
      hours[hour].active += bucket.active;
      hours[hour].knownMs += bucket.knownMs;
      hours[hour].samples++;
    });
    days.push({ key: dayKey(day), label: day.getDate(), ...values, knownDevices });
  }
  return { days, hours };
}

function nextHour(at, local) {
  const end = at + 3600000 - (local.getMinutes() * 60000 + local.getSeconds() * 1000 + local.getMilliseconds());
  const offset = local.getTimezoneOffset();
  if (new Date(end - 1).getTimezoneOffset() === offset) return end;
  let lower = at;
  let upper = end;
  while (upper - lower > 1) {
    const mid = Math.floor((upper + lower) / 2);
    if (new Date(mid).getTimezoneOffset() === offset) lower = mid;
    else upper = mid;
  }
  return upper;
}

function intervalTotalUnion(rows) {
  let end = 0;
  let total = 0;
  for (const row of [...rows].sort((a, b) => a.startMs - b.startMs)) {
    total += Math.max(0, row.endMs - Math.max(row.startMs, end));
    end = Math.max(row.endMs, end);
  }
  return total;
}

export function clipRows(rows, start, end) {
  if (end <= start) return [];
  return rows.filter(row => row.startMs < end && row.endMs > start)
    .map(row => ({ ...row, startMs: Math.max(row.startMs, start), endMs: Math.min(row.endMs, end) }));
}

export function aggregateApps(rows) {
  const apps = new Map();
  for (const row of rows) {
    if (row.state !== 0 || !row.appId) continue;
    const id = JSON.stringify(row.sharedAppId ? ["shared", row.sharedAppId] : [row.device, row.appId]);
    const app = apps.get(id) ?? { id, name: row.appName || row.appId, duration: 0, pages: new Map(), devices: new Set() };
    const span = row.endMs - row.startMs;
    app.duration += span;
    app.devices.add(row.device);
    if (row.page && span > 0) {
      const kind = row.pageKind === "site" ? "site" : "document";
      const pageKey = kind + ":" + row.page;
      const page = app.pages.get(pageKey) ?? { name: row.page, kind, duration: 0 };
      page.duration += span;
      app.pages.set(pageKey, page);
    }
    apps.set(id, app);
  }
  return [...apps.values()].sort((a, b) => b.duration - a.duration || a.name.localeCompare(b.name, "zh-CN"))
    .map(app => {
      const pages = [...app.pages.values()].sort((a, b) => b.duration - a.duration);
      const remainder = app.duration - pages.reduce((total, page) => total + page.duration, 0);
      if (pages.length && remainder > 0)
        pages.push({ name: "页面未识别", kind: "unknown", duration: remainder });
      return { ...app, pages };
    });
}

export function dayParts(usageRows, appRows, start, end) {
  return [[0, "凌晨"], [6, "上午"], [12, "下午"], [18, "晚上"]].map(([hour, name]) => {
    const boundary = new Date(start);
    boundary.setHours(hour, 0, 0, 0);
    const next = new Date(start);
    next.setHours(hour + 6, 0, 0, 0);
    const from = +boundary;
    const until = +next;
    const visible = Math.min(end, until);
    return { hour, name, from, until, future: end <= from,
      usage: usageMetrics(clipRows(usageRows, from, visible)),
      apps: aggregateApps(clipRows(appRows, from, visible)) };
  });
}

// Presentation groups short excursions only when they return to the same app.
// Keep all raw intervals in the group for expanded inspection and accounting.
export function ledgerGroups(rows, thresholdMs) {
  if (!thresholdMs) return rows.map(row => ({ ...row, parts: [row] }));
  const tracks = Map.groupBy(rows, row => row.device);
  return [...tracks.values()].flatMap(track => groupTrack(track, thresholdMs)).sort((a, b) => a.startMs - b.startMs || a.endMs - b.endMs);
}

function groupTrack(rows, thresholdMs) {
  const groups = [];
  for (let i = 0; i < rows.length; i++) {
    const first = rows[i];
    const parts = [first];
    while (first.state === 0 && first.appId) {
      const tail = parts.at(-1);
      const brief = rows[i + 1];
      const back = rows[i + 2];
      if (brief?.state !== 0 || back?.state !== 0 || tail.endMs !== brief.startMs
        || brief.endMs !== back.startMs || brief.endMs - brief.startMs >= thresholdMs
        || first.appId === brief.appId || first.appId !== back.appId
        || first.page !== back.page || Boolean(first.playback) !== Boolean(back.playback)) break;
      parts.push(brief, back);
      i += 2;
    }
    groups.push({ ...first, endMs: parts.at(-1).endMs, parts, folded: (parts.length - 1) / 2 });
  }
  return groups;
}

// Display-only smoothing: join adjacent runs of the same app, or the same
// non-active state, when the gap is a capture flicker rather than a real switch.
export function coalesceTimelineItems(items, maxGapMs = 2000) {
  const identity = row => JSON.stringify(row.sharedAppId ? ["shared", row.sharedAppId] : [row.device, row.appId]);
  const merged = [];
  for (let index = 0; index < items.length; index++) {
    const item = { ...items[index] };
    const previous = merged.at(-1);
    if (previous) {
      const previousId = previous.state === 0 && !previous.unidentified ? identity(previous) : null;
      const next = items[index + 1];
      const nextId = next && next.state === 0 && !next.unidentified ? identity(next) : null;
      const flicker = item.state === 0 && previousId && nextId && previousId === nextId &&
        (item.unidentified || identity(item) !== previousId) &&
        item.endMs - item.startMs <= maxGapMs &&
        item.startMs - previous.endMs <= maxGapMs &&
        next.startMs - item.endMs <= maxGapMs;
      if (flicker) {
        const beforeGap = Math.max(0, item.startMs - previous.endMs);
        const afterGap = Math.max(0, next.startMs - item.endMs);
        previous.endMs = Math.max(previous.endMs, next.endMs);
        previous.merged = (previous.merged ?? 1) + 2;
        previous.gapMs = (previous.gapMs ?? 0) + (item.endMs - item.startMs) + beforeGap + afterGap;
        index++;
        continue;
      }
      const gap = item.startMs - previous.endMs;
      const sameRun = previous.state === item.state && gap >= 0 && gap <= maxGapMs &&
        (item.state !== 0 || (!previous.unidentified && !item.unidentified && identity(previous) === identity(item)));
      if (sameRun) {
        previous.endMs = Math.max(previous.endMs, item.endMs);
        previous.merged = (previous.merged ?? 1) + 1;
        previous.gapMs = (previous.gapMs ?? 0) + gap;
        continue;
      }
    }
    merged.push(item);
  }
  return merged;
}

export function activeSpans(rows, start, end) {
  const spans = clipRows(rows.filter(row => row.state === 0), start, end)
    .map(row => ({ startMs: row.startMs, endMs: row.endMs }))
    .sort((a, b) => a.startMs - b.startMs || a.endMs - b.endMs);
  const merged = [];
  for (const span of spans) {
    const previous = merged.at(-1);
    if (previous && span.startMs <= previous.endMs) previous.endMs = Math.max(previous.endMs, span.endMs);
    else merged.push({ ...span });
  }
  return merged;
}

export function quietSpans(active, start, end) {
  const quiet = [];
  let cursor = start;
  for (const span of active) {
    if (span.startMs > cursor) quiet.push({ startMs: cursor, endMs: Math.min(span.startMs, end) });
    cursor = Math.max(cursor, span.endMs);
    if (cursor >= end) break;
  }
  if (cursor < end) quiet.push({ startMs: cursor, endMs: end });
  return quiet.filter(span => span.endMs > span.startMs);
}

// Non-uniform display scale. Long quiet spans are capped to a small share of
// the axis; informative spans keep their real duration. Time-to-pixel and
// pixel-to-time both stay piecewise linear and reversible.
export function buildTimelineScale(start, end, quiet = [], options = {}) {
  const {
    minQuietMs = 20 * 60e3,
    maxQuietSpans = 4,
    quietShare = 0.05,
    minQuietWeight = 5 * 60e3
  } = options;
  const clean = quiet
    .map(span => ({ ...span, startMs: Math.max(start, span.startMs), endMs: Math.min(end, span.endMs) }))
    .filter(span => span.endMs > span.startMs)
    .sort((a, b) => a.startMs - b.startMs || a.endMs - b.endMs);
  const candidates = clean
    .filter(span => span.endMs - span.startMs >= minQuietMs)
    .sort((a, b) => (b.endMs - b.startMs) - (a.endMs - a.startMs))
    .slice(0, maxQuietSpans);
  const compressedKeys = new Set(candidates.map(span => span.startMs + ":" + span.endMs));
  const compressedReal = candidates.reduce((sum, span) => sum + span.endMs - span.startMs, 0);
  const normalReal = Math.max(0, end - start - compressedReal);
  const cap = Math.max(minQuietWeight, normalReal * quietShare);

  const segments = [];
  let cursor = start;
  for (const span of clean) {
    if (span.startMs > cursor) segments.push({ from: cursor, to: span.startMs, compressed: false, weight: span.startMs - cursor });
    const duration = span.endMs - span.startMs;
    const compressed = compressedKeys.has(span.startMs + ":" + span.endMs) && duration > cap;
    segments.push({
      from: span.startMs, to: span.endMs, compressed,
      weight: compressed ? cap : duration, label: span.label, detail: span.detail
    });
    cursor = Math.max(cursor, span.endMs);
  }
  if (cursor < end) segments.push({ from: cursor, to: end, compressed: false, weight: end - cursor });
  if (!segments.length) segments.push({ from: start, to: end, compressed: false, weight: end - start });

  const total = segments.reduce((sum, segment) => sum + segment.weight, 0);
  let offset = 0;
  for (const segment of segments) {
    segment.startRatio = offset / total;
    offset += segment.weight;
    segment.endRatio = offset / total;
  }

  const findTime = at => {
    const clamped = Math.max(start, Math.min(end, at));
    let low = 0, high = segments.length - 1;
    while (low < high) {
      const mid = (low + high) >> 1;
      if (clamped >= segments[mid].to) low = mid + 1;
      else high = mid;
    }
    return segments[low];
  };
  const findRatio = value => {
    const clamped = Math.max(0, Math.min(1, value));
    let low = 0, high = segments.length - 1;
    while (low < high) {
      const mid = (low + high) >> 1;
      if (clamped >= segments[mid].endRatio) low = mid + 1;
      else high = mid;
    }
    return segments[low];
  };
  const toRatio = at => {
    if (at <= start) return 0;
    if (at >= end) return 1;
    const segment = findTime(at);
    const within = (at - segment.from) / (segment.to - segment.from || 1);
    return segment.startRatio + within * (segment.endRatio - segment.startRatio);
  };
  const fromRatio = value => {
    if (value <= 0) return start;
    if (value >= 1) return end;
    const segment = findRatio(value);
    const within = (value - segment.startRatio) / (segment.endRatio - segment.startRatio || 1);
    return segment.from + within * (segment.to - segment.from);
  };
  return {
    segments,
    compressed: segments.filter(segment => segment.compressed),
    toRatio,
    fromRatio
  };
}
