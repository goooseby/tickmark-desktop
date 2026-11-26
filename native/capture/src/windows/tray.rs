use super::{event_name, running, stop, wide};
use crate::data_dir;
use std::{
    io,
    mem::{size_of, zeroed},
    path::PathBuf,
    process::Command,
    ptr::{null, null_mut},
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering::Relaxed},
    thread,
    time::Duration,
};
use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
};

const CALLBACK: u32 = WM_APP + 42;
const KEY_SELECT: u32 = NIN_SELECT | 1;
const POLL: usize = 1;
const OPEN: usize = 100;
const CAPTURE: usize = 101;
const DIRECTORY: usize = 102;
const QUIT: usize = 103;
static ICON: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(null_mut());
static ADDED: AtomicBool = AtomicBool::new(false);
static VERSIONED: AtomicBool = AtomicBool::new(false);
static RECORDING: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

pub struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) { unsafe { CloseHandle(self.0) }; }
}

fn scoped_name(suffix: &str) -> Vec<u16> {
    let mut name = event_name(&data_dir());
    name.pop();
    name.extend(suffix.encode_utf16());
    name.push(0);
    name
}

fn claim(suffix: &str) -> io::Result<Option<InstanceGuard>> {
    let name = scoped_name(suffix);
    let handle = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
    let status = unsafe { GetLastError() };
    if handle.is_null() {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let guard = InstanceGuard(handle);
    Ok((status != ERROR_ALREADY_EXISTS).then_some(guard))
}

pub fn claim_dashboard() -> io::Result<Option<InstanceGuard>> {
    claim("-dashboard")
}

/// The handle is borrowed from the Tauri window for the duration of SetPropW.
pub fn mark_dashboard(window: HWND) -> io::Result<()> {
    let property = scoped_name("-dashboard-window");
    if unsafe { SetPropW(window, property.as_ptr(), 1usize as HANDLE) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn dashboard_window() -> HWND {
    struct Search { name: Vec<u16>, result: HWND }
    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> i32 {
        let search = unsafe { &mut *(data as *mut Search) };
        if !unsafe { GetPropW(window, search.name.as_ptr()) }.is_null() {
            search.result = window;
            return 0;
        }
        1
    }
    let mut search = Search { name: scoped_name("-dashboard-window"), result: null_mut() };
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
    search.result
}

pub fn activate_dashboard() -> bool {
    let existing = dashboard_window();
    if existing.is_null() { return false; }
    unsafe {
        if IsIconic(existing) != 0 { ShowWindow(existing, SW_RESTORE); }
        else { ShowWindow(existing, SW_SHOW); }
        if SetForegroundWindow(existing) == 0 {
            let flash = FLASHWINFO {
                cbSize: size_of::<FLASHWINFO>() as u32,
                hwnd: existing,
                dwFlags: FLASHW_TRAY,
                uCount: 3,
                dwTimeout: 0,
            };
            FlashWindowEx(&flash);
        }
    }
    true
}

pub fn activate_existing_dashboard() {
    for _ in 0..30 {
        if activate_dashboard() { return; }
        thread::sleep(Duration::from_millis(50));
    }
}

pub fn ensure_tray() -> io::Result<()> {
    Command::new(sibling("tickmark-tray.exe")?).spawn()?;
    Ok(())
}

pub fn start_capture() -> io::Result<()> {
    if !running() {
        Command::new(sibling("tickmark-capture.exe")?).spawn()?;
    }
    Ok(())
}

fn sibling(file: &str) -> io::Result<PathBuf> {
    Ok(std::env::current_exe()?.with_file_name(file))
}

fn open_dashboard() -> io::Result<()> {
    if !activate_dashboard() {
        Command::new(sibling("tickmark-desktop.exe")?).spawn()?;
    }
    Ok(())
}

fn open_data_folder() -> io::Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    Command::new("explorer.exe").arg(dir).spawn()?;
    Ok(())
}

pub fn run_tray() -> io::Result<()> {
    let Some(_instance) = claim("-tray")? else { return Ok(()); };
    let class_name = wide("TickmarkNotificationArea");
    let instance = unsafe { GetModuleHandleW(null()) };
    let class = WNDCLASSW { lpfnWndProc: Some(callback), hInstance: instance,
        lpszClassName: class_name.as_ptr(), ..unsafe { zeroed() } };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(io::Error::last_os_error());
    }

    let window = unsafe {
        CreateWindowExW(WS_EX_TOOLWINDOW, class_name.as_ptr(), wide("Tickmark 托盘").as_ptr(),
            WS_POPUP, 0, 0, 0, 0, null_mut(), null_mut(), instance, null())
    };
    if window.is_null() { return Err(io::Error::last_os_error()); }
    let icon = load_icon();
    if icon.is_null() {
        unsafe { DestroyWindow(window) };
        return Err(io::Error::last_os_error());
    }
    ICON.store(icon, Relaxed);
    TASKBAR_CREATED.store(unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) }, Relaxed);
    refresh(window);
    let timer = unsafe { SetTimer(window, POLL, 1200, None) };
    let result = if timer == 0 { Err(io::Error::last_os_error()) } else {
        let mut message: MSG = unsafe { zeroed() };
        loop {
            let next = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
            if next <= 0 { break if next < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }; }
            unsafe { DispatchMessageW(&message) };
        }
    };
    if unsafe { IsWindow(window) } != 0 { unsafe { DestroyWindow(window) }; }
    unsafe { DestroyIcon(icon) };
    result
}

fn load_icon() -> HICON {
    let ico = include_bytes!("../../../ui/icons/icon.ico");
    if ico.len() < 6 || ico[0] != 0 || ico[1] != 0 || ico[2] != 1 || ico[3] != 0 {
        return null_mut();
    }
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let target = unsafe { GetSystemMetrics(SM_CXSMICON) } as u32;
    let mut best = None;
    for index in 0..count {
        let entry = 6 + index * 16;
        if entry + 16 > ico.len() { break; }
        let width = if ico[entry] == 0 { 256 } else { u32::from(ico[entry]) };
        let height = if ico[entry + 1] == 0 { 256 } else { u32::from(ico[entry + 1]) };
        let Ok(size_bytes) = ico[entry + 8..entry + 12].try_into() else { continue; };
        let Ok(offset_bytes) = ico[entry + 12..entry + 16].try_into() else { continue; };
        let size = u32::from_le_bytes(size_bytes) as usize;
        let offset = u32::from_le_bytes(offset_bytes) as usize;
        let Some(end) = offset.checked_add(size) else { continue; };
        if end > ico.len() { continue; }
        let score = width.max(height).abs_diff(target);
        if best.is_none_or(|(best_score, _, _)| score < best_score) {
            best = Some((score, offset, size));
        }
    }
    let Some((_, offset, size)) = best else { return null_mut(); };
    let Some(image) = ico.get(offset..offset + size) else { return null_mut(); };
    // Win32 documents that this input must be DWORD-aligned, including after the ICO directory.
    let mut aligned = vec![0u32; image.len().div_ceil(4)];
    unsafe {
        std::ptr::copy_nonoverlapping(image.as_ptr(), aligned.as_mut_ptr().cast::<u8>(), image.len());
        CreateIconFromResourceEx(aligned.as_ptr().cast(), image.len() as u32, 1, 0x00030000,
            GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), LR_DEFAULTCOLOR)
    }
}

fn notify_data(window: HWND) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = window;
    data.uID = 1;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    data.uCallbackMessage = CALLBACK;
    data.hIcon = ICON.load(Relaxed);
    let text = if RECORDING.load(Relaxed) { "Tickmark · 正在采集" } else { "Tickmark · 采集已停止" };
    for (slot, unit) in data.szTip.iter_mut().zip(text.encode_utf16()) { *slot = unit; }
    data
}

fn refresh(window: HWND) {
    let recording = running();
    let changed = RECORDING.swap(recording, Relaxed) != recording;
    let mut data = notify_data(window);
    if !ADDED.load(Relaxed) {
        if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0 {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            VERSIONED.store(unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) } != 0, Relaxed);
            ADDED.store(true, Relaxed);
        }
    } else if changed && unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) } == 0 {
        ADDED.store(false, Relaxed);
    }
}

fn anchor(window: HWND, w: WPARAM) -> POINT {
    let mut box_: RECT = unsafe { zeroed() };
    let id = NOTIFYICONIDENTIFIER { cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
        hWnd: window, uID: 1, guidItem: GUID::from_u128(0) };
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { Shell_NotifyIconGetRect(&id, &mut box_) } >= 0 {
        point.x = box_.left + (box_.right - box_.left) / 2;
        point.y = box_.top;
    } else if VERSIONED.load(Relaxed) && w != 0 {
        point.x = (w as u16 as i16) as i32;
        point.y = ((w >> 16) as u16 as i16) as i32;
    } else {
        unsafe { GetCursorPos(&mut point) };
    }
    point
}

fn context_menu(window: HWND, position: POINT) {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() { return; }
    let recording = running();
    let add = |id, caption: &str, flags| unsafe { AppendMenuW(menu, flags, id, wide(caption).as_ptr()) };
    add(0, if recording { "正在采集" } else { "采集已停止" }, MF_STRING | MF_GRAYED);
    add(OPEN, "打开 Tickmark", MF_STRING);
    add(CAPTURE, if recording { "停止采集" } else { "开始采集" }, MF_STRING);
    add(DIRECTORY, "打开数据目录", MF_STRING);
    add(0, "", MF_SEPARATOR);
    add(QUIT, "退出 Tickmark", MF_STRING);
    unsafe {
        SetMenuDefaultItem(menu, OPEN as u32, 0);
        SetForegroundWindow(window);
        TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_CENTERALIGN | TPM_BOTTOMALIGN,
            position.x, position.y, 0, window, null());
        PostMessageW(window, WM_NULL, 0, 0);
        DestroyMenu(menu);
        if IsWindow(window) != 0 && GetForegroundWindow() == window {
            Shell_NotifyIconW(NIM_SETFOCUS, &notify_data(window));
        }
    }
}

fn dispatch(window: HWND, command: usize) {
    let result = match command {
        OPEN => open_dashboard(),
        CAPTURE => if running() { stop() } else { start_capture() },
        DIRECTORY => open_data_folder(),
        QUIT => quit(window),
        _ => return,
    };
    if let Err(error) = result { show_error(window, error); }
    if command == CAPTURE { refresh(window); }
}

fn quit(window: HWND) -> io::Result<()> {
    stop()?;
    for _ in 0..24 {
        if !running() {
            let dashboard = dashboard_window();
            unsafe {
                if !dashboard.is_null() { PostMessageW(dashboard, WM_CLOSE, 0, 0); }
                DestroyWindow(window);
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(125));
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "采集进程尚未停止，请稍后重试。"))
}

fn show_error(window: HWND, error: io::Error) {
    unsafe { MessageBoxW(window, wide(format!("操作未完成。\n{error}")).as_ptr(),
        wide("Tickmark").as_ptr(), MB_OK | MB_ICONWARNING) };
}

unsafe extern "system" fn callback(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match message {
        CALLBACK => {
            match l as u16 as u32 {
                WM_CONTEXTMENU => context_menu(window, anchor(window, w)),
                WM_RBUTTONUP if !VERSIONED.load(Relaxed) => context_menu(window, anchor(window, w)),
                NIN_SELECT | KEY_SELECT => {
                    if let Err(error) = open_dashboard() { show_error(window, error); }
                }
                WM_LBUTTONDBLCLK if !VERSIONED.load(Relaxed) => {
                    if let Err(error) = open_dashboard() { show_error(window, error); }
                }
                _ => {}
            }
            0
        }
        WM_COMMAND => { dispatch(window, w & 0xffff); 0 }
        WM_TIMER if w == POLL => { refresh(window); 0 }
        WM_DESTROY => {
            unsafe {
                KillTimer(window, POLL);
                if ADDED.swap(false, Relaxed) { Shell_NotifyIconW(NIM_DELETE, &notify_data(window)); }
                PostQuitMessage(0);
            }
            0
        }
        _ if message != 0 && message == TASKBAR_CREATED.load(Relaxed) => {
            ADDED.store(false, Relaxed);
            refresh(window);
            0
        }
        _ => unsafe { DefWindowProcW(window, message, w, l) },
    }
}
