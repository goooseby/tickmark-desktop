import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readdir, readFile, unlink, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { setTimeout as delay } from "node:timers/promises";

const root = path.resolve(import.meta.dirname, "../..");
const output = path.join(root, "artifacts", "account-review");
await mkdir(output, { recursive: true });
const data = await mkdtemp(path.join(output, "fixture-"));
const desktop = path.join(data, "tickmark-desktop.exe");
await copyFile(path.join(root, "artifacts", "rust", "debug", "tickmark-desktop.exe"), desktop);
const fixtureDb = new DatabaseSync(path.join(data, "capture.db"));
fixtureDb.exec("CREATE TABLE capture_spans(start_ms INTEGER PRIMARY KEY, end_ms INTEGER NOT NULL, state INTEGER NOT NULL, app_id TEXT, app_name TEXT, page TEXT)");
const insertSpan = fixtureDb.prepare("INSERT INTO capture_spans VALUES (?, ?, ?, ?, ?, ?)");
const anchor = Date.now() - 4 * 60 * 60 * 1000;
for (const [index, app, label, page] of [
  [0, "code", "Visual Studio Code", "tickmark"],
  [1, "edge", "Microsoft Edge", "learn.microsoft.com"],
  [2, "word", "Microsoft Word", "研究计划.docx"],
  [3, "code", "Visual Studio Code", "tickmark"],
]) {
  const start = anchor + index * 45 * 60 * 1000;
  insertSpan.run(start, start + (25 + index * 4) * 60 * 1000, 0, app, label, page);
}
fixtureDb.close();
const reportDir = path.join(data, "report-devices");
await mkdir(reportDir);
for (const [id, name, kind, appName] of [
  ["main-phone", "随身手机", "phone", "即时通讯"],
  ["study-tablet", "学习平板", "tablet", "阅读器"],
]) {
  await writeFile(path.join(reportDir, id + ".json"), JSON.stringify({
    id, name, kind, platform: "Android", revision: 1, updatedAtMs: Date.now(),
    rows: [{ startMs: Date.now() - 180_000, endMs: Date.now() - 60_000, state: 0, appId: id + "-sample", appName }]
  }));
}
const email = "review@example.com";
const cloudFixture = process.env.TICKMARK_CLOUD_FIXTURE;
const refreshToken = "fixture-refresh-token-1";
const rotatedToken = "fixture-refresh-token-2";
const calls = [];
let failRefresh = false;
let failLogout = false;
let throttle = true;
let fixtureError;

function session(token) {
  return {
    account: { id: "fixture-account", email },
    accessToken: "fixture-access-token",
    accessExpiresInSeconds: 900,
    refreshToken: token
  };
}

function reply(res, status, body) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

const server = createServer(async (req, res) => {
  try {
    const route = new URL(req.url, "http://127.0.0.1").pathname;
    if (req.method === "GET" && route === "/v1/changes") {
      reply(res, 200, { epoch: "fixture", changes: [], devices: [], nextAfter: 0, hasMore: false });
      return;
    }
    if (req.method === "PUT" && /^\/v1\/devices\/[a-f0-9-]+\/shards$/.test(route)) {
      await new Response(req).arrayBuffer();
      res.writeHead(204).end();
      return;
    }
    assert.equal(req.method, "POST");
    const payload = JSON.parse(await new Response(req).text());
    calls.push([req.url, payload]);
    if (req.url === "/v1/auth/challenges") {
      assert.deepEqual(payload, { email });
      if (throttle) {
        throttle = false;
        res.writeHead(429, { "Retry-After": "1" }).end();
      } else {
        reply(res, 200, { challengeId: "fixture-challenge", expiresInSeconds: 300, retryAfterSeconds: 60 });
      }
    } else if (req.url === "/v1/auth/verify") {
      assert.equal(payload.email, email);
      assert.equal(payload.challengeId, "fixture-challenge");
      assert.match(payload.device.id, /^[0-9a-f-]{36}$/);
      assert.equal(payload.device.platform, "windows");
      assert.equal(payload.device.kind, "computer");
      if (payload.code === "012345") reply(res, 200, session(refreshToken));
      else reply(res, 401, { code: "invalid_code" });
    } else if (req.url === "/v1/auth/refresh") {
      assert.equal(payload.refreshToken, refreshToken);
      assert.match(payload.requestId, /^[0-9a-f-]{36}$/);
      if (failRefresh) req.socket.destroy();
      else reply(res, 200, session(rotatedToken));
    } else if (req.url === "/v1/auth/logout") {
      assert.equal(payload.refreshToken, rotatedToken);
      if (failLogout) reply(res, 503, { code: "unavailable" });
      else res.writeHead(204).end();
    } else assert.fail("Unexpected route");
  } catch (error) {
    fixtureError = error;
    res.writeHead(500).end();
  }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));

let browser, app, serial = 0;
const port = 18600 + Math.floor(Math.random() * 1000);
const profile = path.join(data, "webview2");
async function launch(configure = true) {
  const env = {
    ...process.env,
    TICKMARK_DATA_DIR: data,
    WEBVIEW2_USER_DATA_FOLDER: profile,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=" + port
  };
  if (configure === "default") delete env.TICKMARK_ACCOUNT_API_TEST;
  else if (configure) env.TICKMARK_ACCOUNT_API_TEST = cloudFixture ?? "http://127.0.0.1:" + server.address().port;
  else env.TICKMARK_ACCOUNT_API_TEST = "";
  app = spawn(desktop, [], { env, stdio: "ignore" });
  let target;
  for (let attempt = 0; attempt < 60; attempt++) {
    try {
      const pages = await (await fetch("http://127.0.0.1:" + port + "/json/list")).json();
      target = pages.find(page => page.type === "page" && page.url.includes("tauri"));
      if (target) break;
    } catch {}
    await delay(150);
  }
  assert.ok(target, "Tauri WebView target");
  browser = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    browser.addEventListener("open", resolve, { once: true });
    browser.addEventListener("error", reject, { once: true });
  });
  await until("Boolean(window.__TAURI__ && document.querySelector('#account-open'))");
  await call("Emulation.setDeviceMetricsOverride", { width: 1200, height: 800, deviceScaleFactor: 1, mobile: false });
  await call("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
}

async function stop() {
  browser?.close();
  if (app && app.exitCode === null && app.signalCode === null) {
    const exited = new Promise(resolve => app.once("exit", resolve));
    app.kill();
    await exited;
    await delay(500);
  }
}

function call(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = ++serial;
    const timeout = setTimeout(() => {
      browser.removeEventListener("message", receive);
      reject(new Error(method + " timed out"));
    }, 8000);
    function receive(event) {
      const message = JSON.parse(event.data);
      if (message.id !== id) return;
      clearTimeout(timeout);
      browser.removeEventListener("message", receive);
      if (message.error) reject(new Error(message.error.message));
      else resolve(message.result);
    }
    browser.addEventListener("message", receive);
    browser.send(JSON.stringify({ id, method, params }));
  });
}

async function evaluate(expression) {
  const result = await call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? expression);
  return result.result.value;
}

async function until(expression, attempts = 50) {
  for (let attempt = 0; attempt < attempts; attempt++) {
    if (fixtureError) throw fixtureError;
    if (await evaluate(expression)) return;
    await delay(100);
  }
  throw new Error("Timed out: " + expression);
}

async function screenshot(name) {
  const { data } = await call("Page.captureScreenshot", { format: "png" });
  await writeFile(path.join(output, (process.argv[2] ?? "final") + "-" + name + ".png"), Buffer.from(data, "base64"));
}

const element = id => "document.getElementById(" + JSON.stringify("account-" + id) + ")";
const waitVisible = id => until("!" + element(id) + ".hidden");
const click = id => evaluate(element(id) + ".click()");
const openMenu = async () => {
  await evaluate(element("open") + ".focus()");
  await click("open");
  await until(element("popover") + ".matches(':popover-open')");
};
const openLogin = async () => {
  await waitVisible("login");
  await click("login");
  assert.equal(await evaluate(element("popover") + ".matches(':popover-open')"), false);
  assert.equal(await evaluate(element("login-panel") + ".matches(':modal')"), true);
  assert.ok(await evaluate(element("email") + ".getBoundingClientRect().height > 0 && " + element("code") + ".getBoundingClientRect().height > 0"));
};
const submit = (input, value, form) => evaluate(element(input) + ".value=" + JSON.stringify(value) + ";" + element(form) + ".requestSubmit()");
const sendCode = value => evaluate(element("email") + ".value=" + JSON.stringify(value) + ";" + element("send") + ".click()");

async function verifyCloud() {
  await launch();
  await openMenu();
  await openLogin();
  await sendCode("sync-test@example.com");
  await until("document.activeElement.id === 'account-code'");
  const response = await fetch(cloudFixture + "/__fixture/code");
  assert.ok(response.ok);
  await submit("code", (await response.text()).trim(), "form");
  await until("!" + element("login-panel") + ".open");
  await until("Array.from(document.querySelectorAll('.hub-device-row')).some(row => row.textContent.includes('同步验收手机') && row.textContent.includes('云端记录'))", 200);
  await screenshot("cloud-dashboard");
  await evaluate(`document.querySelector('button[aria-label="查看同步验收手机的使用记录"]').click()`);
  await until("Array.from(document.querySelectorAll('button')).some(button => button.textContent === '应用会话')");
  await evaluate("Array.from(document.querySelectorAll('button')).find(button => button.textContent === '应用会话').click()");
  await until("document.querySelector('.ledger-row')?.textContent.includes('App')");
  await screenshot("cloud-ledger");
  if (process.env.TICKMARK_CLOUD_DATABASE) {
    const db = new DatabaseSync(process.env.TICKMARK_CLOUD_DATABASE, { readOnly: true });
    const count = db.prepare("SELECT count(*) AS n FROM shards WHERE payload LIKE '%windows-upload-fixture%'");
    const before = count.get().n;
    const local = new DatabaseSync(path.join(data, "capture.db"));
    const start = Date.now() + 100;
    local.prepare("INSERT INTO capture_spans(start_ms,end_ms,state,app_id,app_name) VALUES (?, ?, 0, ?, ?)")
      .run(start, start + 500, "windows-upload-fixture", "桌面上传验收");
    local.close();
    try {
      for (let attempt = 0; attempt < 40 && count.get().n === before; attempt++) await delay(250);
      assert.ok(count.get().n > before, "Windows interval persisted in the server database");
    } finally { db.close(); }
  }
  await openMenu();
  await waitVisible("signed-in");
  await click("logout");
  await waitVisible("login");
  await evaluate("Array.from(document.querySelectorAll('#nav button')).find(button => button.textContent === '多设备仪表盘').click()");
  await until("!document.querySelector('.hub-sources').textContent.includes('同步验收手机')");
  console.log("Native Windows account pulled Android data from the persistent HTTP server and removed the cloud view on logout.");
}

try {
  if (cloudFixture) {
    await verifyCloud();
  } else {
  await launch();
  await until("document.querySelectorAll('.hub-device-row').length === 3");
  assert.equal(await evaluate("document.querySelector('h1').textContent"), "多设备仪表盘");
  await screenshot("dashboard");
  await call("Emulation.setDeviceMetricsOverride", { width: 770, height: 560, deviceScaleFactor: 1, mobile: false });
  await screenshot("dashboard-narrow");
  await call("Emulation.setDeviceMetricsOverride", { width: 1200, height: 800, deviceScaleFactor: 1, mobile: false });
  await evaluate(`document.querySelector('button[aria-label="查看学习平板的使用记录"]').click()`);
  await until("document.querySelector('.ledger-row')?.textContent.includes('阅读器')");
  assert.equal(await evaluate(`document.querySelector('select[aria-label="筛选设备"]').value`), "study-tablet");
  await evaluate("Array.from(document.querySelectorAll('#nav button')).find(button => button.textContent === '今日复盘').click()");
  await until("document.querySelector('h1').textContent === '今日复盘'");
  await openMenu();
  await waitVisible("login");
  assert.equal(await evaluate(element("open") + ".getAttribute('aria-expanded')"), "true");
  assert.ok(await evaluate(element("popover") + ".getBoundingClientRect().bottom < " + element("open") + ".getBoundingClientRect().top"));
  await screenshot("guest-menu");
  await call("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
  await until("!" + element("popover") + ".matches(':popover-open')");
  assert.equal(await evaluate("document.activeElement.id"), "account-open");
  await openMenu();
  await click("menu-settings");
  await until(element("settings") + ".getAttribute('aria-current') === 'page'");
  assert.equal(await evaluate(element("popover") + ".matches(':popover-open')"), false);
  await evaluate("document.querySelector('#nav button').click()");
  await call("Input.dispatchKeyEvent", { type: "keyDown", key: ",", code: "Comma", modifiers: 2, windowsVirtualKeyCode: 188 });
  await until(element("settings") + ".getAttribute('aria-current') === 'page'");
  await call("Input.dispatchKeyEvent", { type: "keyUp", key: ",", code: "Comma", windowsVirtualKeyCode: 188 });
  await evaluate("document.querySelector('#nav button').click()");
  await click("settings");
  await until("document.querySelector('h1').textContent === '设置'");
  await evaluate("document.querySelector('#nav button').click()");
  await openMenu();
  await openLogin();
  assert.equal(await evaluate("document.activeElement.id"), "account-email");
  await call("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
  await until("!" + element("login-panel") + ".open");
  await until("document.activeElement.id === 'account-open'");
  await openMenu();
  await openLogin();
  await click("back");
  await until("document.activeElement.id === 'account-open'");
  await openMenu();
  await openLogin();
  await screenshot("email");
  await sendCode("incomplete");
  await until(element("email") + ".getAttribute('aria-invalid') === 'true'");
  assert.equal(calls.length, 0);
  await sendCode(email);
  await until(element("error") + ".textContent.includes('频繁')");
  await until("!" + element("send") + ".disabled");
  await sendCode(email);
  await until("document.activeElement.id === 'account-code' && " + element("send") + ".disabled");
  await screenshot("code");
  await call("Emulation.setDeviceMetricsOverride", { width: 770, height: 560, deviceScaleFactor: 1, mobile: false });
  await call("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });
  assert.ok(await evaluate(`(() => {
    const rect = ${element("login-panel")}.getBoundingClientRect();
    return rect.x >= 0 && rect.y >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight;
  })()`));
  await screenshot("code-dark");
  await call("Emulation.setDeviceMetricsOverride", { width: 1200, height: 800, deviceScaleFactor: 1, mobile: false });
  await call("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
  await submit("code", "012", "form");
  await until(element("error") + ".textContent.includes('六位')");
  assert.equal(calls.filter(([route]) => route === "/v1/auth/verify").length, 0);
  await submit("code", "999999", "form");
  await until(element("error") + ".textContent.includes('不正确')");
  assert.equal(await evaluate(element("code") + ".getAttribute('aria-invalid')"), "true");
  await screenshot("invalid-code");
  await evaluate(`${element("email")}.value = "other@example.com";
    ${element("email")}.dispatchEvent(new Event("input", { bubbles: true }));
    ${element("code")}.focus();
    ${element("form")}.requestSubmit();`);
  await until(element("error") + ".textContent.includes('请先获取')");
  assert.equal(calls.filter(([route]) => route === "/v1/auth/verify").length, 1);
  await evaluate(`${element("email")}.value = ${JSON.stringify(email)};
    ${element("email")}.dispatchEvent(new Event("input", { bubbles: true }));`);
  await evaluate(`(() => {
    const data = new DataTransfer();
    data.setData("text/plain", "012 345");
    ${element("code")}.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
  })()`);
  assert.equal(await evaluate(element("code") + ".value"), "012345");
  await evaluate(element("form") + ".requestSubmit()");
  await until("!" + element("login-panel") + ".open");
  await openMenu();
  await waitVisible("signed-in");
  assert.equal(await evaluate(element("identity") + ".textContent"), email);
  assert.equal(await evaluate(element("trigger-label") + ".textContent"), email);
  const rawStatus = await evaluate("window.__TAURI__.core.invoke('account_status')");
  assert.deepEqual(Object.keys(rawStatus).sort(), ["account", "configured", "logoutPending", "offline"]);
  const vaultDir = path.join(data, "accounts-v1");
  const vault = (await readdir(vaultDir)).find(name => name.endsWith(".bin"));
  assert.ok(vault);
  const ciphertext = await readFile(path.join(vaultDir, vault));
  assert.ok(!ciphertext.includes(refreshToken) && !ciphertext.includes(email));
  await screenshot("signed-in");

  await stop();
  failRefresh = true;
  await launch();
  await openMenu();
  await until(element("open") + ".dataset.state === 'offline'");
  await screenshot("offline");
  await stop();
  failRefresh = false;
  await launch();
  await openMenu();
  await waitVisible("signed-in");
  const refreshes = calls.filter(([route]) => route === "/v1/auth/refresh");
  assert.equal(refreshes.length, 2);
  assert.deepEqual(refreshes[0], refreshes[1]);

  failLogout = true;
  await call("Emulation.setDeviceMetricsOverride", { width: 770, height: 560, deviceScaleFactor: 1, mobile: false });
  await call("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });
  await click("logout");
  await waitVisible("login");
  await until(element("menu-status") + ".textContent.includes('撤销服务器会话')");
  await screenshot("offline-logout-dark");
  await stop();
  failLogout = false;
  await launch();
  await openMenu();
  await openLogin();
  assert.equal(calls.filter(([route]) => route === "/v1/auth/logout").length, 2);
  await sendCode(email);
  await until("document.activeElement.id === 'account-code'");
  await submit("code", "012345", "form");
  await until("!" + element("login-panel") + ".open");
  await openMenu();
  await waitVisible("signed-in");
  const successes = calls.filter(([route, payload]) => route === "/v1/auth/verify" && payload.code === "012345");
  assert.equal(successes[0][1].device.id, successes[1][1].device.id);

  await stop();
  await unlink(path.join(reportDir, "main-phone.json"));
  await unlink(path.join(reportDir, "study-tablet.json"));
  await launch(false);
  await until("document.querySelectorAll('.hub-device-row').length === 1");
  assert.match(await evaluate("document.querySelector('.hub-sources').textContent"), /目前只读取到本机设备/);
  await screenshot("dashboard-single");
  await openMenu();
  await until(element("menu-status") + ".textContent.includes('尚未连接')");
  assert.equal(await evaluate(element("login-panel") + ".open"), false);
  await screenshot("unconfigured");
  await stop();
  await launch("default");
  await openMenu();
  await waitVisible("login");
  assert.equal((await evaluate("window.__TAURI__.core.invoke('account_status')")).configured, true);
  console.log("Native WebView account flow passed, including rate-limit, wrong code, DPAPI, offline restart, idempotent refresh, queued logout, and stable installation ID.");
  }
} finally {
  await stop();
  server.close();
}
