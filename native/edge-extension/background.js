const hostName = "com.tickmark.edge";
let port;
let revision = 0;
let reconnectAt = 0;

function connect() {
  if (port || Date.now() < reconnectAt) return;
  const next = chrome.runtime.connectNative(hostName);
  port = next;
  next.onDisconnect.addListener(() => {
    void chrome.runtime.lastError;
    if (port === next) port = undefined;
    reconnectAt = Date.now() + 2000;
  });
}

function domain(url) {
  if (!url) return null;
  try {
    const value = new URL(url);
    if (value.protocol !== "http:" && value.protocol !== "https:") return null;
    const host = value.hostname.replace(/\.$/, "");
    // Send only a DNS host. File paths, URLs, IP addresses and titles stay in Edge.
    if (/^[0-9.]+$/.test(host) || host.includes(":")) return null;
    return host;
  } catch {
    return null;
  }
}

async function videoPlaying(tab) {
  if (!tab?.id || !tab.url || !/^https?:/.test(tab.url)) return false;
  try {
    const [result] = await chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => [...document.querySelectorAll("video")].some(video =>
        !video.paused && !video.ended && video.readyState >= 3 && video.videoWidth > 0)
    });
    return result?.result === true;
  } catch {
    return false;
  }
}

async function publish() {
  const current = ++revision;
  try {
    const window = await chrome.windows.getLastFocused({ windowTypes: ["normal", "popup"] });
    const focused = window.focused && !window.incognito;
    const [tab] = focused ? await chrome.tabs.query({ windowId: window.id, active: true }) : [];
    if (revision !== current) return;
    const host = !tab || tab.incognito || tab.pendingUrl ? null : domain(tab.url);
    const playing = !tab || tab.incognito || tab.pendingUrl ? false : await videoPlaying(tab);
    if (revision !== current) return;
    connect();
    port?.postMessage({ focused, host, videoPlaying: playing });
  } catch {
    if (revision !== current) return;
    connect();
    port?.postMessage({ focused: false, host: null, videoPlaying: false });
  }
}

chrome.windows.onFocusChanged.addListener(publish);
chrome.windows.onCreated.addListener(publish);
chrome.windows.onRemoved.addListener(publish);
chrome.tabs.onActivated.addListener(publish);
chrome.tabs.onUpdated.addListener((_id, change, tab) => {
  if (tab.active && (change.url !== undefined || change.status !== undefined)) publish();
});
chrome.tabs.onAttached.addListener(publish);
chrome.tabs.onReplaced.addListener(publish);
setInterval(publish, 2000);
publish();
