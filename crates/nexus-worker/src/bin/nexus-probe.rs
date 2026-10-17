//! TEST HELPER — never shipped. Run inside the container by
//! `tests/isolation.rs`, it tries one operation and prints one JSON line
//! saying whether it worked (and the OS error if not). It never prints what
//! it read, so a failing probe cannot leak the content it reached.
//!
//! nexus-probe <op> [args...]
//!   read <file>            write <file>            list <dir>
//!   pipe <\\.\pipe\name>   tcp <host:port>          open-process <pid>
//!   spawn <count>          sleep <ms>               busy <ms>
//!   alloc <MiB>            output <bytes>           env
//!   creds <filter>         token                    child-and-exit

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn result(op: &str, outcome: Result<Value, std::io::Error>) -> Value {
    match outcome {
        Ok(extra) => json!({ "op": op, "ok": true, "detail": extra }),
        Err(e) => {
            json!({ "op": op, "ok": false, "code": e.raw_os_error(), "error": e.to_string() })
        }
    }
}

fn arg(args: &[String], i: usize) -> &str {
    args.get(i).map(String::as_str).unwrap_or("")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let op = arg(&args, 0).to_string();
    let out = match op.as_str() {
        "read" => result(
            &op,
            std::fs::read(arg(&args, 1)).map(|b| json!({ "bytes": b.len() })),
        ),
        "write" => result(
            &op,
            std::fs::write(arg(&args, 1), b"written by the probe").map(|_| json!(null)),
        ),
        "list" => result(
            &op,
            std::fs::read_dir(arg(&args, 1)).map(|d| json!({ "entries": d.count() })),
        ),
        "pipe" => result(
            &op,
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(arg(&args, 1))
                .map(|_| json!(null)),
        ),
        "tcp" => {
            let target = arg(&args, 1);
            let connected = target
                .to_socket_addrs()
                .and_then(|mut a| a.next().ok_or_else(|| std::io::Error::other("no address")))
                .and_then(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(5)));
            result(&op, connected.map(|_| json!(null)))
        }
        "open-process" => result(&op, open_process(arg(&args, 1))),
        "spawn" => {
            // Starts `count` sleeping copies of itself; reports how many started.
            let count: usize = arg(&args, 1).parse().unwrap_or(1);
            let me = std::env::current_exe().unwrap_or_default();
            let mut started = 0;
            let mut children = Vec::new();
            let mut first_error = None;
            for _ in 0..count {
                match std::process::Command::new(&me)
                    .args(["sleep", "3000"])
                    .spawn()
                {
                    Ok(c) => {
                        started += 1;
                        children.push(c);
                    }
                    Err(e) => {
                        first_error.get_or_insert(e.to_string());
                    }
                }
            }
            for mut c in children {
                let _ = c.kill();
            }
            json!({ "op": op, "ok": first_error.is_none(), "started": started, "error": first_error })
        }
        "sleep" => {
            std::thread::sleep(Duration::from_millis(arg(&args, 1).parse().unwrap_or(0)));
            json!({ "op": op, "ok": true })
        }
        "busy" => {
            let until = Instant::now() + Duration::from_millis(arg(&args, 1).parse().unwrap_or(0));
            let mut x = 0u64;
            while Instant::now() < until {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            }
            json!({ "op": op, "ok": true, "x": x % 2 })
        }
        "alloc" => {
            let mib: usize = arg(&args, 1).parse().unwrap_or(0);
            let mut v: Vec<u8> = Vec::new();
            match v.try_reserve_exact(mib * 1024 * 1024) {
                Ok(()) => {
                    v.resize(mib * 1024 * 1024, 1);
                    json!({ "op": op, "ok": true, "bytes": v.len() })
                }
                Err(e) => json!({ "op": op, "ok": false, "error": e.to_string() }),
            }
        }
        "output" => {
            let bytes: usize = arg(&args, 1).parse().unwrap_or(0);
            let mut stdout = std::io::stdout().lock();
            let chunk = [b'x'; 1024];
            let mut left = bytes;
            while left > 0 {
                let n = left.min(chunk.len());
                if stdout.write_all(&chunk[..n]).is_err() {
                    break;
                }
                left -= n;
            }
            let _ = writeln!(stdout);
            json!({ "op": op, "ok": true })
        }
        "env" => {
            let names: Vec<String> = std::env::vars_os()
                .map(|(k, _)| k.to_string_lossy().into_owned())
                .collect();
            json!({ "op": op, "ok": true, "names": names })
        }
        "creds" => creds(arg(&args, 1)),
        "token" => token(),
        "child-and-exit" => {
            // Leaves a long-lived child behind and exits at once.
            let me = std::env::current_exe().unwrap_or_default();
            match std::process::Command::new(me)
                .args(["sleep", "60000"])
                .spawn()
            {
                Ok(c) => json!({ "op": op, "ok": true, "child": c.id() }),
                Err(e) => json!({ "op": op, "ok": false, "error": e.to_string() }),
            }
        }
        _ => json!({ "op": op, "ok": false, "error": "unknown op" }),
    };
    println!("{out}");
}

fn open_process(pid: &str) -> Result<Value, std::io::Error> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE};
    let pid: u32 = pid.parse().map_err(|_| std::io::Error::other("bad pid"))?;
    // SAFETY: plain call; the handle, if any, is closed at once.
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a handle we own.
    unsafe { CloseHandle(handle) };
    Ok(json!(null))
}

fn creds(filter: &str) -> Value {
    use windows_sys::Win32::Security::Credentials::{CREDENTIALW, CredEnumerateW, CredFree};
    let filter: Vec<u16> = filter.encode_utf16().chain(std::iter::once(0)).collect();
    let mut count = 0u32;
    let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: valid filter; the list is freed below.
    let ok = unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut list) };
    if ok == 0 {
        let e = std::io::Error::last_os_error();
        return json!({ "op": "creds", "ok": false, "code": e.raw_os_error(), "error": e.to_string() });
    }
    // SAFETY: allocated by CredEnumerateW.
    unsafe { CredFree(list as _) };
    json!({ "op": "creds", "ok": true, "count": count })
}

fn token() -> Value {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TokenIsAppContainer};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token = std::ptr::null_mut();
    // SAFETY: the current process's token, closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return json!({ "op": "token", "ok": false });
    }
    let mut is_container = 0u32;
    let mut len = 0u32;
    // SAFETY: a u32 output for TokenIsAppContainer.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenIsAppContainer,
            &mut is_container as *mut _ as _,
            4,
            &mut len,
        )
    };
    // SAFETY: our token handle.
    unsafe { CloseHandle(token) };
    json!({ "op": "token", "ok": ok != 0, "app_container": is_container != 0 })
}
