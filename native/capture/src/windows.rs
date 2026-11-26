use crate::{data_dir, engine::{Engine, Frame, Slice, State}, preferences::Preferences, store::Store};
use std::{
    cell::RefCell, fs::OpenOptions, io, mem::{size_of, zeroed}, os::windows::ffi::OsStrExt,
    path::Path, ptr::{null, null_mut}, sync::atomic::{AtomicPtr, Ordering}, time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            LibraryLoader::GetModuleHandleW,
            Power::{POWERBROADCAST_SETTING, RegisterPowerSettingNotification, UnregisterPowerSettingNotification},
            RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification},
            StationsAndDesktops::{CloseDesktop, DESKTOP_READOBJECTS, GetUserObjectInformationW, OpenInputDesktop, UOI_NAME},
            SystemInformation::GetTickCount64,
            Threading::{CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject},
        },
        UI::{
            Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
            Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
            WindowsAndMessaging::*,
        },
    },
};

pub mod browser;
mod window_identity;
pub mod tray;

const SAMPLE: usize = 1;
const CAPTURE_CLASS: &str = "TickmarkCaptureWindow";
const CAPTURE_EVENT: u32 = WM_APP + 1;
const DISPLAY: GUID = GUID::from_u128(0x2b84c20e_ad23_4ddf_93db_05ffbd7efca5);
const STOP_ACCESS: u32 = EVENT_MODIFY_STATE;
const WAIT_SIGNALED: u32 = 0;
static WINDOW: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(null_mut());
thread_local! { static CAPTURE: RefCell<Option<Runtime>> = const { RefCell::new(None) }; }

struct Runtime {
    store: Store,
    engine: Engine,
    preferences: Preferences,
    browser: browser::Browser,
    foreground_since: i64,
    browser_reported: bool,
    pending: Vec<Slice>,
    offset: i64,
    last_tick: i64,
    last_saved: i64,
    stop: HANDLE,
    window: HWND,
    locked: bool,
    display_off: bool,
    disconnected: bool,
    suspended: bool,
    error: Option<String>,
}

impl Runtime {
    fn sample(&mut self, window: HWND, at: i64, reconcile: bool) -> Result<(), Box<dyn std::error::Error>> {
        // Read input before settling the previous target for both native events and polling.
        let input = last_input();
        let unix = unix_ms();
        let tick = tick();
        if (unix - (self.offset + tick)).abs() > 5_000 {
            self.pending.extend(self.engine.finish(self.last_tick, None));
            self.flush()?;
            self.offset = unix - tick;
            self.bridge_gap(self.offset + at)?;
            self.engine = Engine::new(at);
            self.last_tick = at;
        }
        let offline = self.locked || self.display_off || self.disconnected || self.suspended;
        if self.window != window || offline || input.is_none() {
            self.browser.clear_page();
            self.foreground_since = at;
            self.browser_reported = false;
        }
        let next = if offline {
            Frame::Offline
        } else if input.is_none() || !normal_input_desktop() {
            Frame::Unknown
        } else {
            let (mut target, _, presentation) = window_identity::identify(window, &self.preferences.documents);
            if let Some(target) = target.as_mut()
                && target.name.eq_ignore_ascii_case("Microsoft Edge")
                && let Some(host) = self.browser.page(window)
            {
                target.page = Some(host.to_owned());
            }
            let media_exempt = target.as_ref().is_some_and(|target| !target.transient) &&
                (presentation || self.browser.video_playing(window));
            Frame::Online { target, media_exempt }
        };

        if reconcile && at - self.last_tick > 5_000 && !offline {
            self.pending.extend(self.engine.observe(self.last_tick, None, Frame::Unknown));
        } else if reconcile && self.window != window && !self.window.is_null()
            && matches!(next, Frame::Online { .. })
        {
            self.pending.extend(self.engine.observe(self.last_tick, None, Frame::Online { target: None, media_exempt: false }));
        }
        self.pending.extend(self.engine.observe(at, input, next));
        self.window = window;
        self.last_tick = at;
        if offline || at - self.last_saved >= 2_000 && at - self.foreground_since >= browser::ALIGN_MS {
            self.flush()?;
            self.last_saved = at;
            let preferences = self.store.preferences()?;
            self.preferences = preferences;
        }
        Ok(())
    }

    fn browser_report(&mut self, reported_window: HWND, report: browser::Report, at: i64) -> Result<(), Box<dyn std::error::Error>> {
        let foreground = unsafe { GetForegroundWindow() };
        let matched = !reported_window.is_null() && reported_window == foreground;
        let owner = if matched { reported_window } else { null_mut() };
        if self.window != foreground {
            self.sample(foreground, at, true)?;
        }
        let previous = self.browser.page(foreground).map(str::to_owned);
        self.browser.accept(owner, report, at);
        let next = self.browser.page(foreground).map(str::to_owned);
        if let Some(host) = next.as_deref() {
            if !self.browser_reported && at - self.foreground_since <= browser::ALIGN_MS {
                let slices = self.engine.observe(at, last_input(), self.engine.current_frame());
                self.pending.extend(slices);
                for slice in self.pending.iter_mut().rev().take_while(|slice| slice.start >= self.foreground_since) {
                    if let Some(target) = slice.target.as_mut()
                        && target.page.is_none()
                        && target.name.eq_ignore_ascii_case("Microsoft Edge")
                    {
                        target.page = Some(host.to_owned());
                    }
                }
            }
            self.browser_reported = true;
        }
        if next != previous {
            self.sample(foreground, at, false)?;
        }
        Ok(())
    }

    fn flush(&mut self) -> rusqlite::Result<()> {
        self.store.append(self.offset, &self.pending)?;
        self.pending.clear();
        Ok(())
    }

    fn bridge_gap(&mut self, at: i64) -> rusqlite::Result<()> {
        if let Some(previous) = self.store.last_end()? && previous < at {
            self.store.append(0, &[Slice { start: previous, end: at, state: State::Gap, media_exempt: false, target: None }])?;
        }
        Ok(())
    }
}

pub fn running() -> bool {
    let event = unsafe { OpenEventW(STOP_ACCESS, 0, event_name(&data_dir()).as_ptr()) };
    if event.is_null() {
        return false;
    }
    unsafe { CloseHandle(event) };
    true
}

pub fn stop() -> io::Result<()> {
    let event = unsafe { OpenEventW(STOP_ACCESS, 0, event_name(&data_dir()).as_ptr()) };
    if event.is_null() {
        if io::Error::last_os_error().raw_os_error() == Some(2) {
            return Ok(());
        }
        return Err(io::Error::last_os_error());
    }
    let result = unsafe { SetEvent(event) };
    let error = io::Error::last_os_error();
    unsafe { CloseHandle(event) };
    if result == 0 { Err(error) } else { Ok(()) }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    // Hold a single-writer lease for this capture database.
    use std::os::windows::fs::OpenOptionsExt;
    let _lease = OpenOptions::new().write(true).create(true).truncate(false).share_mode(0)
        .open(dir.join("capture.lock"))?;
    let store = Store::open(&dir)?;
    let name = wide(CAPTURE_CLASS);
    let scoped_name = event_name(&dir);
    let instance = unsafe { GetModuleHandleW(null()) };
    let class = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance, lpszClassName: name.as_ptr(), ..unsafe { zeroed() } };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let window = unsafe {
        CreateWindowExW(0, name.as_ptr(), scoped_name.as_ptr(), WS_POPUP, 0, 0, 0, 0, null_mut(), null_mut(), instance, null())
    };
    if window.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    WINDOW.store(window, Ordering::Relaxed);
    let timer = unsafe { SetTimer(window, SAMPLE, 750, None) };
    let display = unsafe { RegisterPowerSettingNotification(window, &DISPLAY, 0) };
    let session = unsafe { WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) };
    let focus = unsafe { SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, null_mut(), Some(hook), 0, 0, WINEVENT_OUTOFCONTEXT) };
    let title = unsafe { SetWinEventHook(EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE, null_mut(), Some(hook), 0, 0, WINEVENT_OUTOFCONTEXT) };
    let event = unsafe { CreateEventW(null(), 1, 0, event_name(&dir).as_ptr()) };
    let result: Result<(), Box<dyn std::error::Error>> = (|| {
        if timer == 0 || event.is_null() || display == 0 || session == 0 || focus.is_null() || title.is_null() {
            return Err("无法订阅 Windows 输入会话、显示或前台事件。".into());
        }
        let now = tick();
        let unix = unix_ms();
        let preferences = store.preferences()?;
        let engine = Engine::new(now);
        let mut runtime = Runtime {
            store, engine, preferences, pending: vec![], offset: unix - now, last_tick: now, last_saved: now,
            browser: browser::Browser::new(), foreground_since: now, browser_reported: false,
            stop: event, window: null_mut(), locked: false, display_off: false, disconnected: false,
            suspended: false, error: None,
        };
        runtime.bridge_gap(unix)?;
        runtime.sample(unsafe { GetForegroundWindow() }, now, false)?;
        let error_log = dir.join("capture-error.log");
        if error_log.is_file() { let _ = std::fs::remove_file(error_log); }
        CAPTURE.with(|capture| capture.replace(Some(runtime)));

        let mut message = unsafe { zeroed() };
        loop {
            let next = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
            if next <= 0 {
                if next < 0 { return Err(io::Error::last_os_error().into()); }
                break;
            }
            unsafe { DispatchMessageW(&message) };
        }
        if let Some(mut runtime) = CAPTURE.with(|capture| capture.take()) {
            if let Some(error) = runtime.error.take() {
                return Err(error.into());
            }
            runtime.pending.extend(runtime.engine.finish(tick(), last_input()));
            runtime.flush()?;
        }
        Ok(())
    })();
    WINDOW.store(null_mut(), Ordering::Relaxed);
    unsafe {
        if !focus.is_null() { UnhookWinEvent(focus); }
        if !title.is_null() { UnhookWinEvent(title); }
        if display != 0 { UnregisterPowerSettingNotification(display); }
        if session != 0 { WTSUnRegisterSessionNotification(window); }
        if timer != 0 { KillTimer(window, SAMPLE); }
        if !event.is_null() { CloseHandle(event); }
        DestroyWindow(window);
    }
    result
}

unsafe extern "system" fn hook(_: HWINEVENTHOOK, event: u32, target: HWND, object: i32, child: i32, _: u32, clock: u32) {
    if event == EVENT_OBJECT_NAMECHANGE && (object != 0 || child != 0) { return; }
    let window = WINDOW.load(Ordering::Relaxed);
    if !window.is_null() {
        let payload = (u64::from(event) << 32) | u64::from(clock);
        unsafe { PostMessageW(window, CAPTURE_EVENT, target as usize, payload as isize) };
    }
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == browser::QUERY_STATUS {
        return CAPTURE.with(|value| value.try_borrow().ok().is_some_and(|value|
            value.as_ref().is_some_and(|runtime| runtime.browser.connected()))) as LRESULT;
    }
    if message == WM_COPYDATA {
        let Some(report) = (unsafe { browser::decode(l) }) else { return 0 };
        return CAPTURE.with(|value| {
            let Ok(mut value) = value.try_borrow_mut() else { return 0 };
            let Some(runtime) = value.as_mut() else { return 0 };
            match runtime.browser_report(w as HWND, report, tick()) {
                Ok(()) => 1,
                Err(error) => {
                    runtime.error = Some(error.to_string());
                    unsafe { PostQuitMessage(1) };
                    0
                }
            }
        });
    }
    if !matches!(message, WM_TIMER | CAPTURE_EVENT | WM_WTSSESSION_CHANGE | WM_POWERBROADCAST) {
        return unsafe { DefWindowProcW(window, message, w, l) };
    }
    CAPTURE.with(|value| {
        let Ok(mut value) = value.try_borrow_mut() else { return };
        let Some(runtime) = value.as_mut() else { return };
        let now = tick();
        let (target, at, reconcile) = match message {
            WM_TIMER => {
                if unsafe { WaitForSingleObject(runtime.stop, 0) } == WAIT_SIGNALED {
                    unsafe { PostQuitMessage(0) };
                    return;
                }
                (unsafe { GetForegroundWindow() }, now, true)
            }
            CAPTURE_EVENT => {
                let event = (l as u64 >> 32) as u32;
                let target = w as HWND;
                if event == EVENT_OBJECT_NAMECHANGE && target != runtime.window { return; }
                let received = expand_tick(l as u32, now).unwrap_or(now).max(runtime.last_tick);
                (target, received, false)
            }
            WM_WTSSESSION_CHANGE => {
                match w {
                    0x7 => runtime.locked = true,
                    0x8 => runtime.locked = false,
                    0x2 | 0x4 | 0x6 => runtime.disconnected = true,
                    0x1 | 0x3 | 0x5 => runtime.disconnected = false,
                    _ => return,
                }
                (unsafe { GetForegroundWindow() }, now, false)
            }
            WM_POWERBROADCAST => {
                match w {
                    0x4 => runtime.suspended = true,
                    0x6 | 0x7 | 0x12 => runtime.suspended = false,
                    0x8013 if l != 0 => unsafe {
                        let setting = l as *const POWERBROADCAST_SETTING;
                        if (*setting).PowerSetting.data1 != DISPLAY.data1 ||
                            (*setting).PowerSetting.data2 != DISPLAY.data2 ||
                            (*setting).PowerSetting.data3 != DISPLAY.data3 ||
                            (*setting).PowerSetting.data4 != DISPLAY.data4 || (*setting).DataLength < 4 { return; }
                        runtime.display_off = (*setting).Data[0] == 0;
                    },
                    _ => return,
                }
                (unsafe { GetForegroundWindow() }, now, false)
            }
            _ => return,
        };
        if let Err(error) = runtime.sample(target, at, reconcile) {
            runtime.error = Some(error.to_string());
            unsafe { PostQuitMessage(1) };
        }
    });
    0
}

fn normal_input_desktop() -> bool {
    let desktop = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
    if desktop.is_null() { return false; }
    let mut name = [0u16; 64];
    let mut needed = 0;
    let result = unsafe {
        GetUserObjectInformationW(desktop, UOI_NAME, name.as_mut_ptr().cast(), size_of::<[u16; 64]>() as u32, &mut needed)
    };
    unsafe { CloseDesktop(desktop) };
    result != 0 && String::from_utf16_lossy(&name[..name.iter().position(|item| *item == 0).unwrap_or(name.len())])
        .eq_ignore_ascii_case("Default")
}

fn last_input() -> Option<i64> {
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    if unsafe { GetLastInputInfo(&mut info) } == 0 { return None; }
    expand_tick(info.dwTime, tick())
}

fn event_name(path: &Path) -> Vec<u16> {
    use sha2::{Digest, Sha256};
    let key = path.to_string_lossy().to_lowercase().replace('/', "\\");
    wide(format!("Local\\TickmarkCapture-{:x}", Sha256::digest(key.as_bytes())))
}

fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn expand_tick(clock: u32, now: i64) -> Option<i64> {
    let age = (now as u32).wrapping_sub(clock);
    (age <= i32::MAX as u32).then_some(now - i64::from(age))
}

fn tick() -> i64 { unsafe { GetTickCount64() as i64 } }
fn unix_ms() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64 }
