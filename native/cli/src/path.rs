use serde_json::json;

use crate::{
    cli::PathArgs,
    errors::CliError,
    output::{self, Format},
};

pub fn run(args: &PathArgs, format_raw: Option<&str>) -> Result<bool, CliError> {
    let actions = [args.status, args.add.is_some(), args.remove.is_some()]
        .into_iter()
        .filter(|flag| *flag)
        .count();
    if actions != 1 {
        return Err(CliError::usage(
            "path 需要且只能指定 --status、--add 或 --remove 之一。",
        ));
    }
    let format = output::resolve_format(format_raw)?;
    if args.status {
        let directory = imp::find_install()?;
        if format == Format::Json {
            output::emit_json(
                &json!({ "present": directory.is_some(), "directory": directory }),
                None,
            )?;
        } else if let Some(directory) = directory {
            output::print_block(&format!("PATH 已包含 {directory}\n"));
        } else {
            output::print_block("PATH 中还没有 Tickmark 目录\n");
        }
        return Ok(false);
    }
    if let Some(path) = &args.add {
        let changed = imp::add(path)?;
        if format == Format::Json {
            output::emit_json(&json!({ "path": path, "changed": changed }), None)?;
        } else {
            output::print_block(&format!(
                "{}\n",
                if changed {
                    "已加入用户 PATH，新开的终端可直接使用 tickmark。"
                } else {
                    "用户 PATH 已包含该目录，无需修改。"
                }
            ));
        }
        return Ok(false);
    }
    let path = args.remove.as_deref().unwrap_or_default();
    let changed = imp::remove(path)?;
    if format == Format::Json {
        output::emit_json(&json!({ "path": path, "changed": changed }), None)?;
    } else {
        output::print_block(&format!(
            "{}\n",
            if changed {
                "已从用户 PATH 移除该目录。"
            } else {
                "用户 PATH 中没有该目录，无需修改。"
            }
        ));
    }
    Ok(false)
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE,
        REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };

    use crate::errors::CliError;

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    struct Environment {
        key: HKEY,
    }

    impl Drop for Environment {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.key);
            }
        }
    }

    fn open() -> Result<Environment, CliError> {
        let mut key: HKEY = std::ptr::null_mut();
        let name = wide("Environment");
        let mut disposition = 0u32;
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                name.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                std::ptr::null(),
                &mut key,
                &mut disposition,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(CliError::error(format!(
                "无法打开用户环境变量注册表项（错误 {status}）。"
            )));
        }
        Ok(Environment { key })
    }

    fn read_value(key: HKEY) -> Result<(String, REG_VALUE_TYPE), CliError> {
        let name = wide("Path");
        let mut kind: REG_VALUE_TYPE = REG_EXPAND_SZ;
        let mut size: u32 = 0;
        let status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok((String::new(), REG_EXPAND_SZ));
        }
        if status != ERROR_SUCCESS {
            return Err(CliError::error(format!(
                "无法读取用户 PATH（错误 {status}）。"
            )));
        }
        let mut buffer = vec![0u8; size.max(2) as usize];
        let mut kind: REG_VALUE_TYPE = REG_EXPAND_SZ;
        let mut size = buffer.len() as u32;
        let status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buffer.as_mut_ptr(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(CliError::error(format!(
                "无法读取用户 PATH（错误 {status}）。"
            )));
        }
        buffer.truncate(size as usize);
        let units: Vec<u16> = buffer
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let text = String::from_utf16_lossy(&units)
            .trim_end_matches('\0')
            .to_string();
        Ok((text, kind))
    }

    fn write_value(key: HKEY, text: &str, kind: REG_VALUE_TYPE) -> Result<(), CliError> {
        let name = wide("Path");
        let data: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let status = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                kind,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(CliError::error(format!(
                "无法写入用户 PATH（错误 {status}）。"
            )));
        }
        let broad = wide("Environment");
        let mut result = 0usize;
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                broad.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                5_000,
                &mut result,
            );
        }
        Ok(())
    }

    fn normalize(entry: &str) -> String {
        entry
            .trim()
            .trim_end_matches('\\')
            .to_lowercase()
    }

    fn entries(text: &str) -> Vec<String> {
        text.split(';')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect()
    }

    pub fn find_install() -> Result<Option<String>, CliError> {
        let environment = open()?;
        let (current, _) = read_value(environment.key)?;
        Ok(entries(&current).into_iter().find(|entry| {
            std::path::Path::new(entry).join("tickmark.exe").is_file()
        }))
    }

    pub fn add(path: &str) -> Result<bool, CliError> {
        let environment = open()?;
        let (current, kind) = read_value(environment.key)?;
        let target = normalize(path);
        let mut items = entries(&current);
        if items.iter().any(|entry| normalize(entry) == target) {
            return Ok(false);
        }
        items.push(path.trim().to_owned());
        let kind = if kind == REG_SZ { REG_SZ } else { REG_EXPAND_SZ };
        write_value(environment.key, &items.join(";"), kind)?;
        Ok(true)
    }

    pub fn remove(path: &str) -> Result<bool, CliError> {
        let environment = open()?;
        let (current, kind) = read_value(environment.key)?;
        let target = normalize(path);
        let items = entries(&current);
        let kept: Vec<String> = items
            .iter()
            .filter(|entry| normalize(entry) != target)
            .cloned()
            .collect();
        if kept.len() == items.len() {
            return Ok(false);
        }
        let kind = if kind == REG_SZ { REG_SZ } else { REG_EXPAND_SZ };
        write_value(environment.key, &kept.join(";"), kind)?;
        Ok(true)
    }
}

#[cfg(not(windows))]
mod imp {
    use crate::errors::CliError;

    pub fn find_install() -> Result<Option<String>, CliError> {
        Err(CliError::unavailable("path 命令目前仅支持 Windows。"))
    }

    pub fn add(_path: &str) -> Result<bool, CliError> {
        Err(CliError::unavailable("path 命令目前仅支持 Windows。"))
    }

    pub fn remove(_path: &str) -> Result<bool, CliError> {
        Err(CliError::unavailable("path 命令目前仅支持 Windows。"))
    }
}
