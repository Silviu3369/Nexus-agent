//! IPC-02 "fake host": before the pairing token leaves this process, confirm
//! the process serving the pipe runs as the expected Windows user. Pipe names
//! are machine-global, so another account could create ours first. A hostile
//! process under this same account is outside the threat model
//! (`SECURITATE_SI_VALIDARE_RO.md` §1).

use std::io;
use std::os::windows::io::AsRawHandle;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned open by a successful Win32 call and
        // is closed exactly once here.
        unsafe { CloseHandle(self.0) };
    }
}

/// SID string (e.g. `S-1-5-21-…`) of the user this process runs as.
pub fn current_user_sid() -> io::Result<String> {
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing.
    token_user_sid(unsafe { GetCurrentProcess() })
}

/// SID string of the user owning the process at the other end of `pipe`.
pub fn pipe_server_user_sid(pipe: &impl AsRawHandle) -> io::Result<String> {
    let mut pid = 0u32;
    // SAFETY: `pipe` is a live client pipe handle; `pid` is a valid out-pointer.
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle() as HANDLE, &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: plain Win32 call; a null result is checked below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = OwnedHandle(process);
    token_user_sid(process.0)
}

fn token_user_sid(process: HANDLE) -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `process` is a valid process handle; `token` is a valid out-pointer.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = OwnedHandle(token);

    let mut needed = 0u32;
    // SAFETY: size query with a null buffer; only `needed` is written.
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    // u64 storage keeps TOKEN_USER (which contains a pointer) aligned.
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `needed` bytes.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call above filled `buf` with a TOKEN_USER whose SID pointer
    // points inside `buf`, which outlives every use below.
    let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };

    let mut wide: *mut u16 = ptr::null_mut();
    // SAFETY: valid SID from the token; `wide` receives a LocalAlloc'd string.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut wide) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `wide` is a NUL-terminated UTF-16 string owned until LocalFree.
    let sid = unsafe {
        let len = (0..).take_while(|&i| *wide.add(i) != 0).count();
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(wide, len));
        LocalFree(wide.cast());
        text
    };
    Ok(sid)
}

#[cfg(test)]
mod tests {
    #[test]
    fn current_user_sid_is_a_windows_sid() {
        let sid = super::current_user_sid().expect("current process token must be readable");
        assert!(sid.starts_with("S-1-5-"), "unexpected SID format: {sid}");
    }
}
