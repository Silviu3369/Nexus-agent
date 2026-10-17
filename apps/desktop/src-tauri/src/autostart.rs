//! Starting Nexus.Host when the user signs in to Windows. Off unless the
//! user turns it on in Settings: it is one value in the user's own Run key
//! (HKCU\Software\Microsoft\Windows\CurrentVersion\Run) naming the
//! nexus-host.exe next to this dashboard. Nothing else is changed, and
//! turning it off removes that value. The window itself is not started.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use serde::Serialize;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW,
};

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE: &str = "Nexus Host";

/// What Settings shows: whether the Host starts at sign-in, and with which
/// program (another copy of Nexus may have set it).
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AutostartView {
    pub enabled: bool,
    /// The command Windows runs at sign-in, if set.
    pub command: Option<String>,
    /// It starts the Host next to this dashboard.
    pub points_here: bool,
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn check(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: a key we opened.
        unsafe { RegCloseKey(self.0) };
    }
}

fn open(key: &str) -> io::Result<Key> {
    let name = wide(key);
    let mut handle: HKEY = null_mut();
    // SAFETY: valid name; `handle` receives an open key, closed by `Key`.
    check(unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            name.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            null(),
            &mut handle,
            null_mut(),
        )
    })?;
    Ok(Key(handle))
}

pub fn read(key: &str, value: &str) -> io::Result<Option<String>> {
    let k = open(key)?;
    let name = wide(value);
    let mut kind = 0u32;
    let mut size = 0u32;
    // SAFETY: size query.
    let code =
        unsafe { RegQueryValueExW(k.0, name.as_ptr(), null(), &mut kind, null_mut(), &mut size) };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(code)?;
    if kind != REG_SZ {
        return Ok(None);
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: a buffer of the queried size.
    check(unsafe {
        RegQueryValueExW(
            k.0,
            name.as_ptr(),
            null(),
            &mut kind,
            buf.as_mut_ptr() as *mut u8,
            &mut size,
        )
    })?;
    let text = String::from_utf16_lossy(&buf);
    Ok(Some(text.trim_end_matches('\0').to_string()))
}

pub fn write(key: &str, value: &str, command: &str) -> io::Result<()> {
    let k = open(key)?;
    let name = wide(value);
    let data = wide(command);
    // SAFETY: REG_SZ data with its terminating NUL.
    check(unsafe {
        RegSetValueExW(
            k.0,
            name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            (data.len() * 2) as u32,
        )
    })
}

pub fn remove(key: &str, value: &str) -> io::Result<()> {
    let k = open(key)?;
    let name = wide(value);
    // SAFETY: a valid key and name.
    let code = unsafe { RegDeleteValueW(k.0, name.as_ptr()) };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(code)
}

/// The command for the Host next to this dashboard, quoted for spaces.
pub fn command_for(host: &Path) -> String {
    format!("\"{}\"", host.display())
}

pub fn view(command: Option<String>, host: &Path) -> AutostartView {
    let points_here = command
        .as_deref()
        .is_some_and(|c| c.eq_ignore_ascii_case(&command_for(host)));
    AutostartView {
        enabled: command.is_some(),
        command,
        points_here,
    }
}

/// The Host shipped next to this dashboard.
fn host_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate dashboard: {e}"))?;
    Ok(exe.with_file_name(crate::host::HOST_EXE))
}

#[tauri::command]
pub fn get_autostart() -> Result<AutostartView, String> {
    let host = host_path()?;
    let command =
        read(RUN_KEY, RUN_VALUE).map_err(|e| format!("cannot read the sign-in setting: {e}"))?;
    Ok(view(command, &host))
}

/// Only ever called from the user's own choice in Settings.
#[tauri::command]
pub fn set_autostart(enabled: bool) -> Result<AutostartView, String> {
    let host = host_path()?;
    if enabled {
        if !host.exists() {
            return Err(format!(
                "{} not found at {}",
                crate::host::HOST_EXE,
                host.display()
            ));
        }
        write(RUN_KEY, RUN_VALUE, &command_for(&host))
            .map_err(|e| format!("cannot save the sign-in setting: {e}"))?;
    } else {
        remove(RUN_KEY, RUN_VALUE)
            .map_err(|e| format!("cannot remove the sign-in setting: {e}"))?;
    }
    get_autostart()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key of its own, never the real Run key.
    struct TestKey(String);

    impl Drop for TestKey {
        fn drop(&mut self) {
            use windows_sys::Win32::System::Registry::RegDeleteTreeW;
            let name = wide(&self.0);
            // SAFETY: deletes only the test's own key.
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, name.as_ptr()) };
        }
    }

    #[test]
    fn the_value_is_written_read_and_removed() {
        let key = TestKey(format!(
            r"Software\Nexus-autostart-test-{}",
            std::process::id()
        ));
        assert_eq!(read(&key.0, RUN_VALUE).unwrap(), None);
        let host = Path::new(r"C:\Program Files\Nexus\nexus-host.exe");
        write(&key.0, RUN_VALUE, &command_for(host)).unwrap();
        let stored = read(&key.0, RUN_VALUE).unwrap();
        assert_eq!(
            stored.as_deref(),
            Some(r#""C:\Program Files\Nexus\nexus-host.exe""#)
        );
        let v = view(stored, host);
        assert!(v.enabled && v.points_here);
        assert!(!view(Some("\"D:\\other\\nexus-host.exe\"".into()), host).points_here);
        remove(&key.0, RUN_VALUE).unwrap();
        remove(&key.0, RUN_VALUE).unwrap();
        assert_eq!(read(&key.0, RUN_VALUE).unwrap(), None);
        assert!(!view(None, host).enabled);
    }
}
