const errorText = {
  invalid_email: "请输入有效的邮箱地址。",
  invalid_code: "验证码不正确，请检查后重试。",
  challenge_expired: "验证码已过期，请重新发送。",
  challenge_exhausted: "验证码尝试次数已用完，请重新发送。",
  rate_limited: "操作过于频繁，请稍后重试。",
  not_configured: "此版本尚未连接账号服务。",
  invalid_configuration: "账号服务配置有误。",
  unavailable: "暂时无法连接账号服务，请稍后重试。",
  local_storage: "无法读取或保存本机账号信息，请重试。",
  already_signed_in: "本机已登录，请先退出当前账号。",
  unexpected_response: "账号服务返回了无法识别的结果。"
};

export function connectAccount(invoke, navigation, icon) {
  const get = id => document.getElementById("account-" + id);
  const open = get("open");
  const popover = get("popover");
  const dialog = get("login-panel");
  const login = get("login");
  const menuSettings = get("menu-settings");
  const avatar = get("avatar");
  const name = get("trigger-label");
  const form = get("form");
  const email = get("email");
  const code = get("code");
  const sendButton = get("send");
  const submitCode = form.querySelector("[type=submit]");
  const error = get("error");
  const menuError = get("menu-error");
  const feedback = get("feedback");
  const menuStatus = get("menu-status");
  const logout = get("logout");
  const logoutLabel = logout.querySelector("span");
  const guestIcon = icon("person");
  let status, statusError, challenge, operation;
  let retryAt = 0;
  let verifyRetryAt = 0;
  let checking = false;
  let visibleAccount;

  for (const [id, glyph] of [["settings", "settings"], ["menu-settings", "settings"], ["login", "enter"], ["logout", "exit"], ["back", "close"]]) {
    get(id).prepend(icon(glyph));
  }
  const isOpen = () => popover.matches(":popover-open");

  const currentEmail = () => email.value.trim().toLowerCase();
  const matchingChallenge = () => challenge?.email === currentEmail() ? challenge : null;

  function showError(text = "") {
    error.textContent = text;
    error.hidden = !text;
    menuError.hidden = true;
    for (const field of [email, code]) {
      field.removeAttribute("aria-invalid");
      field.removeAttribute("aria-describedby");
    }
  }

  function showFailure(failure, action, message) {
    const text = message ?? errorText[failure?.code] ?? errorText.unavailable;
    if (action === "logout") {
      get("logout-row").append(menuError);
      menuError.textContent = text;
      menuError.hidden = false;
      return;
    }
    const forEmail = failure?.code === "invalid_email";
    const field = forEmail ? email : code;
    get(forEmail ? "email-row" : "code-row").append(error);
    showError(text);
    if (failure?.code === "invalid_code" || failure?.code === "invalid_email") field.setAttribute("aria-invalid", "true");
    field.setAttribute("aria-describedby", error.id);
  }

  function refreshControls() {
    const current = performance.now();
    const seconds = Math.max(0, Math.ceil((retryAt - current) / 1000));
    const verifySeconds = Math.max(0, Math.ceil((verifyRetryAt - current) / 1000));
    for (const field of [email, code, logout]) {
      field.disabled = Boolean(operation);
    }
    sendButton.disabled = Boolean(operation) || seconds > 0;
    const sendLabel = matchingChallenge() ? "重新发送" : "获取验证码";
    sendButton.textContent = operation === "send" ? "发送中…" : seconds ? seconds + " 秒后重发" : sendLabel;
    sendButton.setAttribute("aria-label", seconds ? seconds + " 秒后可发送验证码" : sendLabel);
    submitCode.disabled = Boolean(operation) || verifySeconds > 0;
    submitCode.textContent = operation === "verify" ? "登录中…" : verifySeconds ? verifySeconds + " 秒后重试" : "登录";
    logoutLabel.textContent = operation === "logout" ? "正在退出…" : "退出登录";
    get("retry").disabled = checking || Boolean(operation);
  }

  function render() {
    const account = status?.account;
    if (visibleAccount !== account?.id) {
      visibleAccount = account?.id;
      navigation.accountChanged();
    }
    const canSignIn = Boolean(status?.configured && !account);
    open.dataset.state = account ? (status.offline ? "offline" : "online") : "signed-out";
    const label = account?.email ?? "账号";
    if (name.textContent !== label) name.textContent = label;
    open.title = label;
    open.setAttribute("aria-label", account ? "账号：" + label : "账号");
    if (account) {
      const initial = account.email[0].toUpperCase();
      if (avatar.textContent !== initial) avatar.textContent = initial;
    } else if (!avatar.contains(guestIcon)) avatar.replaceChildren(guestIcon);
    get("signed-in").hidden = !account;
    get("logout-row").hidden = !account;
    login.hidden = !canSignIn;
    get("retry").hidden = !statusError;
    get("identity").textContent = account?.email ?? "";
    let summary = "";
    if (statusError) summary = errorText[statusError] ?? errorText.unavailable;
    else if (!status) summary = "正在读取账号状态…";
    else if (account && status.offline) summary = "账号服务暂时离线";
    else if (!status.configured) summary = errorText.not_configured;
    else if (status.logoutPending) summary = "已退出本机。连接恢复后会撤销服务器会话。";
    if (menuStatus.textContent !== summary) menuStatus.textContent = summary;
    refreshControls();
  }

  async function checkStatus() {
    if (checking || operation) return;
    checking = true;
    refreshControls();
    try {
      const updated = await invoke("account_status");
      if (!operation) {
        status = updated;
        statusError = null;
      }
    } catch (failure) {
      if (!operation) statusError = failure?.code ?? "unavailable";
    } finally {
      checking = false;
      render();
    }
  }

  async function run(action, task) {
    if (operation) return;
    operation = action;
    showError();
    refreshControls();
    try {
      return await task();
    } catch (failure) {
      if (failure?.code === "rate_limited") {
        const seconds = Math.max(1, Math.min(900, failure.retryAfterSeconds ?? 60));
        const until = performance.now() + seconds * 1000;
        if (action === "verify") verifyRetryAt = Math.max(verifyRetryAt, until);
        if (action === "send") retryAt = Math.max(retryAt, until);
      }
      if (challenge && ["challenge_expired", "challenge_exhausted"].includes(failure?.code)) {
        challenge.expiresAt = performance.now();
      }
      showFailure(failure, action);
    } finally {
      operation = null;
      render();
    }
  }

  async function send() {
    if (performance.now() < retryAt) return;
    if (!email.validity.valid) {
      showFailure({ code: "invalid_email" }, "send");
      email.focus();
      return;
    }
    const to = currentEmail();
    const sent = await run("send", async () => {
      const result = await invoke("account_challenge", { email: to });
      const now = performance.now();
      challenge = { id: result.challengeId, email: to, expiresAt: now + result.expiresInSeconds * 1000 };
      retryAt = now + result.retryAfterSeconds * 1000;
      code.value = "";
      feedback.textContent = "验证码已请求，请查收邮件。";
      return true;
    });
    if (dialog.open) {
      if (sent) code.focus();
      else email.focus();
    }
  }

  popover.addEventListener("beforetoggle", event => {
    open.setAttribute("aria-expanded", String(event.newState === "open"));
    if (event.newState !== "open") return;
    render();
    checkStatus();
  });
  popover.addEventListener("toggle", event => {
    if (event.newState === "open" && !popover.contains(document.activeElement)) {
      (login.hidden ? menuSettings : login).focus();
    }
  });
  popover.addEventListener("keydown", event => {
    if (event.key === "Escape" && !event.isComposing) {
      event.preventDefault();
      popover.hidePopover();
      open.focus();
    }
  });
  login.addEventListener("click", () => {
    popover.hidePopover();
    showError();
    dialog.showModal();
    render();
    (matchingChallenge() ? code : email).focus();
  });
  get("back").addEventListener("click", () => dialog.close());
  dialog.addEventListener("close", () => {
    if (dialog.open) return;
    showError();
    render();
    open.focus();
  });
  function goToSettings() {
    if (isOpen()) popover.hidePopover();
    navigation.openSettings();
    get("settings").focus();
  }
  get("settings").addEventListener("click", goToSettings);
  menuSettings.addEventListener("click", goToSettings);
  window.addEventListener("keydown", event => {
    if (!dialog.open && event.ctrlKey && !event.altKey && !event.shiftKey && event.key === ",") {
      event.preventDefault();
      goToSettings();
    }
  });
  sendButton.addEventListener("click", send);
  code.addEventListener("input", () => {
    code.value = code.value.replace(/[^0-9]/g, "");
    if (code.hasAttribute("aria-describedby")) showError();
  });
  email.addEventListener("input", () => {
    if (!matchingChallenge()) code.value = "";
    showError();
    refreshControls();
  });
  code.addEventListener("paste", event => {
    const digits = event.clipboardData?.getData("text").replace(/[^0-9]/g, "");
    if (!digits || digits.length !== 6) return;
    event.preventDefault();
    code.value = digits;
    showError();
  });
  form.addEventListener("submit", async event => {
    event.preventDefault();
    if (operation || performance.now() < verifyRetryAt) return;
    const requested = matchingChallenge();
    if (!requested && document.activeElement === email) {
      await send();
      return;
    }
    if (!requested) {
      showFailure(null, "verify", "请先获取验证码。");
      if (!sendButton.disabled) sendButton.focus();
      return;
    }
    if (performance.now() >= requested.expiresAt) {
      showFailure({ code: "challenge_expired" }, "verify");
      return;
    }
    if (!/^[0-9]{6}$/.test(code.value)) {
      showFailure({ code: "invalid_code" }, "verify", "请输入六位验证码。");
      code.focus();
      return;
    }
    const signedIn = await run("verify", async () => {
      status = await invoke("account_verify", {
        email: requested.email, challengeId: requested.id, code: code.value
      });
      statusError = challenge = null;
      code.value = email.value = "";
      retryAt = verifyRetryAt = 0;
      return true;
    });
    if (signedIn) {
      if (dialog.open) dialog.close();
      navigation.signedIn();
    } else if (dialog.open && code.getAttribute("aria-invalid") === "true") {
      code.focus();
      code.select();
    }
  });
  logout.addEventListener("click", async () => {
    const signedOut = await run("logout", async () => {
      status = await invoke("account_logout");
      statusError = null;
      return true;
    });
    if (signedOut && isOpen()) (login.hidden ? menuSettings : login).focus();
  });
  get("retry").addEventListener("click", checkStatus);
  window.setInterval(() => { if (dialog.open && (challenge || retryAt)) refreshControls(); }, 250);
  window.setInterval(checkStatus, 30_000);
  checkStatus();
}
