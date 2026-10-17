//! Starting a process in a strict AppContainer inside a Job Object.
//!
//! The container's identity is a named AppContainer profile (created once
//! per user; its SID is stable). Access is granted by adding an ACE for that
//! SID to exactly the paths named in the spec — nothing else is reachable
//! through it. With `lpac` the process does not even get what Windows grants
//! to all app containers. The process starts suspended, joins the Job
//! Object, and only then runs, so every child it creates is held to the same
//! limits and dies with the job.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError, HANDLE, HANDLE_FLAG_INHERIT, LocalFree,
    SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSidToSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW,
    NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, DeriveCapabilitySidsFromName, FreeSid, PSECURITY_DESCRIPTOR,
    PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_JOB_TIME, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemServices::SE_GROUP_ENABLED;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
    InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows_sys::Win32::System::WindowsProgramming::PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;

/// The `internetClient` capability: outbound connections to the internet.
/// Loopback and the local network stay closed (no
/// `privateNetworkClientServer`, no loopback exemption).
const INTERNET_CLIENT: &str = "S-1-15-3-1";
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const GENERIC_EXECUTE: u32 = 0x2000_0000;
const DELETE: u32 = 0x0001_0000;
/// Exit code given to what is left of the tree when it is ended ("NX").
pub const KILLED_EXIT_CODE: u32 = 0x4E58_0001;

#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("{call} failed: {code}")]
    Win32 { call: &'static str, code: u32 },
    #[error("{0}")]
    Invalid(String),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
}

fn last(call: &'static str) -> ContainerError {
    ContainerError::Win32 {
        call,
        // SAFETY: reads the calling thread's last-error value.
        code: unsafe { GetLastError() },
    }
}

/// What a granted path allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read and run (programs, their libraries, a project to look at).
    Read,
    /// Read, create, change and delete (the working copy).
    ReadWrite,
}

#[derive(Debug, Clone)]
pub struct Grant {
    pub path: PathBuf,
    pub access: Access,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Committed memory of the whole process tree.
    pub memory_bytes: usize,
    /// Processes alive at once in the tree.
    pub processes: u32,
    /// User-mode CPU time of the whole tree.
    pub cpu_time: Duration,
    /// Real time from start to the end of the main process.
    pub wall_time: Duration,
    /// Bytes kept of stdout and of stderr each; the rest is dropped.
    pub output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 1024 * 1024 * 1024,
            processes: 32,
            cpu_time: Duration::from_secs(300),
            wall_time: Duration::from_secs(600),
            output_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContainerSpec {
    /// The AppContainer profile (identity) this process runs as.
    pub profile: String,
    /// Less privileged: not even what Windows grants every app container.
    pub lpac: bool,
    /// Outbound internet (never this machine or the local network).
    pub network: bool,
    /// Named Windows capabilities beyond that (e.g. "registryRead"); empty
    /// unless a tool is shown to need one.
    pub capabilities: Vec<String>,
    pub grants: Vec<Grant>,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// The whole environment the process sees; nothing is inherited
    /// (LOCALAPPDATA is added, and Windows points it at the container's
    /// own folder).
    pub env: Vec<(OsString, OsString)>,
    pub limits: Limits,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub exit_code: u32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// More output than the limit arrived and was dropped.
    pub truncated: bool,
    /// The wall-time limit ended it.
    pub timed_out: bool,
    pub duration: Duration,
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// A SID allocated by Windows, freed the way it was allocated.
struct Sid {
    ptr: PSID,
    by_local_free: bool,
}

impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: the pointer came from the API named in `by_local_free`.
        unsafe {
            if self.by_local_free {
                LocalFree(self.ptr as _);
            } else {
                FreeSid(self.ptr);
            }
        }
    }
}

/// The container's SID, creating its profile the first time.
fn container_sid(profile: &str) -> Result<Sid, ContainerError> {
    if profile.is_empty() || profile.len() > 64 || profile.contains(['\\', '/', ' ']) {
        return Err(ContainerError::Invalid(format!(
            "invalid container profile name {profile:?}"
        )));
    }
    // Windows does not create one profile from two threads at once.
    static CREATING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_at_a_time = CREATING.lock().unwrap_or_else(|e| e.into_inner());
    let name = wide(OsStr::new(profile));
    let mut sid: PSID = null_mut();
    // SAFETY: valid NUL-terminated strings; `sid` receives an allocation.
    let hr = unsafe {
        CreateAppContainerProfile(
            name.as_ptr(),
            name.as_ptr(),
            name.as_ptr(),
            null(),
            0,
            &mut sid,
        )
    };
    let already = 0x8007_0000u32 as i32 | ERROR_ALREADY_EXISTS as i32;
    if hr == already {
        // SAFETY: as above.
        let hr = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
        if hr < 0 {
            return Err(ContainerError::Win32 {
                call: "DeriveAppContainerSidFromAppContainerName",
                code: hr as u32,
            });
        }
    } else if hr < 0 {
        return Err(ContainerError::Win32 {
            call: "CreateAppContainerProfile",
            code: hr as u32,
        });
    }
    Ok(Sid {
        ptr: sid,
        by_local_free: false,
    })
}

fn capability(sid: &str) -> Result<Sid, ContainerError> {
    let s = wide(OsStr::new(sid));
    let mut ptr: PSID = null_mut();
    // SAFETY: valid string; the SID is freed with LocalFree.
    if unsafe { ConvertStringSidToSidW(s.as_ptr(), &mut ptr) } == 0 {
        return Err(last("ConvertStringSidToSidW"));
    }
    Ok(Sid {
        ptr,
        by_local_free: true,
    })
}

/// The SID of a named capability ("registryRead", …).
fn named_capability(name: &str) -> Result<Sid, ContainerError> {
    let n = wide(OsStr::new(name));
    let (mut groups, mut group_count) = (null_mut::<PSID>(), 0u32);
    let (mut sids, mut sid_count) = (null_mut::<PSID>(), 0u32);
    // SAFETY: outputs are LocalAlloc'd arrays of LocalAlloc'd SIDs.
    if unsafe {
        DeriveCapabilitySidsFromName(
            n.as_ptr(),
            &mut groups,
            &mut group_count,
            &mut sids,
            &mut sid_count,
        )
    } == 0
    {
        return Err(last("DeriveCapabilitySidsFromName"));
    }
    // SAFETY: the arrays and SIDs come from the call above; the first
    // capability SID is kept (freed by `Sid`), everything else freed here.
    unsafe {
        for i in 0..group_count as usize {
            LocalFree(*groups.add(i) as _);
        }
        LocalFree(groups as _);
        let kept = if sid_count > 0 { *sids } else { null_mut() };
        for i in 1..sid_count as usize {
            LocalFree(*sids.add(i) as _);
        }
        LocalFree(sids as _);
        if kept.is_null() {
            return Err(ContainerError::Invalid(format!(
                "no capability named {name}"
            )));
        }
        Ok(Sid {
            ptr: kept,
            by_local_free: true,
        })
    }
}

/// Adds an ACE for the container to `path` (inherited by everything below
/// it), keeping every existing entry.
fn grant(path: &Path, sid: &Sid, access: Access) -> Result<(), ContainerError> {
    if !path.exists() {
        return Err(ContainerError::Invalid(format!(
            "granted path does not exist: {}",
            path.display()
        )));
    }
    let name = wide(path.as_os_str());
    let mut old: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: valid path; outputs owned by `descriptor`, freed below.
    let err = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut old,
            null_mut(),
            &mut descriptor,
        )
    };
    if err != ERROR_SUCCESS {
        return Err(ContainerError::Win32 {
            call: "GetNamedSecurityInfoW",
            code: err,
        });
    }
    let mask = match access {
        Access::Read => GENERIC_READ | GENERIC_EXECUTE,
        Access::ReadWrite => GENERIC_READ | GENERIC_WRITE | GENERIC_EXECUTE | DELETE,
    };
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.ptr as _,
        },
    };
    let mut new: *mut ACL = null_mut();
    // SAFETY: one valid entry; `new` is allocated and freed below.
    let err = unsafe { SetEntriesInAclW(1, &entry, old, &mut new) };
    let result = if err != ERROR_SUCCESS {
        Err(ContainerError::Win32 {
            call: "SetEntriesInAclW",
            code: err,
        })
    } else {
        // SAFETY: valid path and ACL.
        let err = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                new,
                null(),
            )
        };
        // SAFETY: allocated by SetEntriesInAclW.
        unsafe { LocalFree(new as _) };
        if err == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(ContainerError::Win32 {
                call: "SetNamedSecurityInfoW",
                code: err,
            })
        }
    };
    // SAFETY: allocated by GetNamedSecurityInfoW.
    unsafe { LocalFree(descriptor as _) };
    result
}

/// An anonymous pipe as (read, write): the child's end is inheritable,
/// ours is not.
fn pipe(child_writes: bool) -> Result<(OwnedHandle, OwnedHandle), ContainerError> {
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let (mut read, mut write) = (null_mut(), null_mut());
    // SAFETY: outputs are two new handles.
    if unsafe { CreatePipe(&mut read, &mut write, &attrs, 0) } == 0 {
        return Err(last("CreatePipe"));
    }
    let ours = if child_writes { read } else { write };
    // SAFETY: a handle we own.
    unsafe { SetHandleInformation(ours, HANDLE_FLAG_INHERIT, 0) };
    // SAFETY: two new handles, owned from here on.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read as _),
            OwnedHandle::from_raw_handle(write as _),
        )
    })
}

/// Reads up to `limit` bytes, then drains the rest so the writer never blocks.
fn collect(handle: OwnedHandle, limit: usize) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    let mut file = std::fs::File::from(handle);
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut truncated = false;
        let mut buf = [0u8; 8192];
        loop {
            match file.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = limit.saturating_sub(kept.len());
                    if n > room {
                        truncated = true;
                    }
                    kept.extend_from_slice(&buf[..n.min(room)]);
                }
            }
        }
        (kept, truncated)
    })
}

fn quote(arg: &OsStr) -> Vec<u16> {
    // The rules CommandLineToArgvW reverses.
    let s: Vec<u16> = arg.encode_wide().collect();
    let needs = s.is_empty()
        || s.iter()
            .any(|&c| c == b' ' as u16 || c == b'\t' as u16 || c == b'"' as u16);
    if !needs {
        return s;
    }
    let mut out = vec![b'"' as u16];
    let mut backslashes = 0;
    for &c in &s {
        if c == b'\\' as u16 {
            backslashes += 1;
        } else if c == b'"' as u16 {
            out.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
            out.push(c);
            backslashes = 0;
        } else {
            out.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
            out.push(c);
            backslashes = 0;
        }
    }
    out.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    out.push(b'"' as u16);
    out
}

fn job(limits: &Limits) -> Result<OwnedHandle, ContainerError> {
    // SAFETY: an anonymous job; the handle is owned below.
    let raw = unsafe { CreateJobObjectW(null(), null()) };
    if raw.is_null() {
        return Err(last("CreateJobObjectW"));
    }
    // SAFETY: a new handle we own.
    let job = unsafe { OwnedHandle::from_raw_handle(raw as _) };
    // SAFETY: zeroed POD structures are valid inputs.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
        | JOB_OBJECT_LIMIT_JOB_MEMORY
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_JOB_TIME;
    info.BasicLimitInformation.ActiveProcessLimit = limits.processes.max(1);
    info.BasicLimitInformation.PerJobUserTimeLimit =
        i64::try_from(limits.cpu_time.as_nanos() / 100).unwrap_or(i64::MAX);
    info.JobMemoryLimit = limits.memory_bytes;
    // SAFETY: valid handle and structure.
    if unsafe {
        SetInformationJobObject(
            raw,
            JobObjectExtendedLimitInformation,
            &info as *const _ as _,
            std::mem::size_of_val(&info) as u32,
        )
    } == 0
    {
        return Err(last("SetInformationJobObject(limits)"));
    }
    let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_EXITWINDOWS
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
    };
    // SAFETY: valid handle and structure.
    if unsafe {
        SetInformationJobObject(
            raw,
            JobObjectBasicUIRestrictions,
            &ui as *const _ as _,
            std::mem::size_of_val(&ui) as u32,
        )
    } == 0
    {
        return Err(last("SetInformationJobObject(ui)"));
    }
    Ok(job)
}

/// Runs the program in the container and waits for it, within the limits.
pub fn run(spec: &ContainerSpec) -> Result<Outcome, ContainerError> {
    let sid = container_sid(&spec.profile)?;
    for g in &spec.grants {
        grant(&g.path, &sid, g.access)?;
    }
    let mut held: Vec<Sid> = Vec::new();
    if spec.network {
        held.push(capability(INTERNET_CLIENT)?);
    }
    for name in &spec.capabilities {
        held.push(named_capability(name)?);
    }
    let mut caps: Vec<SID_AND_ATTRIBUTES> = held
        .iter()
        .map(|c| SID_AND_ATTRIBUTES {
            Sid: c.ptr,
            Attributes: SE_GROUP_ENABLED as u32,
        })
        .collect();
    let security = SECURITY_CAPABILITIES {
        AppContainerSid: sid.ptr,
        Capabilities: if caps.is_empty() {
            null_mut()
        } else {
            caps.as_mut_ptr()
        },
        CapabilityCount: caps.len() as u32,
        Reserved: 0,
    };
    let opt_out: u32 = PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;

    let (out_read, out_write) = pipe(true)?;
    let (err_read, err_write) = pipe(true)?;
    // The child gets stdin at EOF.
    let (in_read, _) = pipe(false)?;
    let inherited = [raw(&in_read), raw(&out_write), raw(&err_write)];

    let attributes = if spec.lpac { 3 } else { 2 };
    let mut size = 0usize;
    // SAFETY: size query.
    unsafe { InitializeProcThreadAttributeList(null_mut(), attributes, 0, &mut size) };
    let mut list = vec![0u8; size];
    let list_ptr = list.as_mut_ptr() as _;
    // SAFETY: buffer of the queried size.
    if unsafe { InitializeProcThreadAttributeList(list_ptr, attributes, 0, &mut size) } == 0 {
        return Err(last("InitializeProcThreadAttributeList"));
    }
    let update = |attribute: u32, value: *const std::ffi::c_void, size: usize| {
        // SAFETY: `value` outlives the list (all live until CreateProcessW).
        if unsafe {
            UpdateProcThreadAttribute(
                list_ptr,
                0,
                attribute as usize,
                value,
                size,
                null_mut(),
                null(),
            )
        } == 0
        {
            Err(last("UpdateProcThreadAttribute"))
        } else {
            Ok(())
        }
    };
    let result = (|| -> Result<Outcome, ContainerError> {
        update(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            &security as *const _ as _,
            std::mem::size_of_val(&security),
        )?;
        update(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            inherited.as_ptr() as _,
            std::mem::size_of_val(&inherited),
        )?;
        if spec.lpac {
            update(
                PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
                &opt_out as *const _ as _,
                std::mem::size_of_val(&opt_out),
            )?;
        }

        let mut command = quote(spec.program.as_os_str());
        for a in &spec.args {
            command.push(b' ' as u16);
            command.extend(quote(a));
        }
        command.push(0);
        let mut environment: Vec<u16> = Vec::new();
        let mut env = spec.env.clone();
        // Windows maps LOCALAPPDATA to the container's own folder when it
        // starts the process, and refuses to start it without one
        // (ERROR_ENVVAR_NOT_FOUND).
        if !env
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("LOCALAPPDATA"))
            && let Some(local) = std::env::var_os("LOCALAPPDATA")
        {
            env.push(("LOCALAPPDATA".into(), local));
        }
        for (k, v) in &env {
            environment.extend(k.encode_wide());
            environment.push(b'=' as u16);
            environment.extend(v.encode_wide());
            environment.push(0);
        }
        if environment.is_empty() {
            environment.push(0);
        }
        environment.push(0);
        let program = wide(spec.program.as_os_str());
        let cwd = wide(spec.cwd.as_os_str());

        // SAFETY: zeroed POD structures are valid inputs.
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = inherited[0];
        startup.StartupInfo.hStdOutput = inherited[1];
        startup.StartupInfo.hStdError = inherited[2];
        startup.lpAttributeList = list_ptr;
        // SAFETY: zeroed output structure.
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let job = job(&spec.limits)?;
        let started = Instant::now();
        // SAFETY: every pointer is valid for the call; handles listed above.
        let created = unsafe {
            CreateProcessW(
                program.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                1,
                EXTENDED_STARTUPINFO_PRESENT
                    | CREATE_SUSPENDED
                    | CREATE_NO_WINDOW
                    | CREATE_UNICODE_ENVIRONMENT,
                environment.as_ptr() as _,
                cwd.as_ptr(),
                &startup.StartupInfo,
                &mut info,
            )
        };
        // Our copies of the child's ends: the pipes end when the tree does.
        drop((in_read, out_write, err_write));
        if created == 0 {
            return Err(last("CreateProcessW"));
        }
        // SAFETY: handles returned by CreateProcessW.
        let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess as _) };
        let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread as _) };
        // The tree is bound to the job before any of it runs.
        // SAFETY: valid handles.
        if unsafe { AssignProcessToJobObject(raw(&job), raw(&process)) } == 0 {
            let err = last("AssignProcessToJobObject");
            // SAFETY: valid handle.
            unsafe { TerminateJobObject(raw(&job), KILLED_EXIT_CODE) };
            unsafe {
                windows_sys::Win32::System::Threading::TerminateProcess(
                    raw(&process),
                    KILLED_EXIT_CODE,
                )
            };
            return Err(err);
        }
        // SAFETY: the suspended main thread.
        unsafe { ResumeThread(raw(&thread)) };
        drop(thread);
        let stdout = collect(out_read, spec.limits.output_bytes);
        let stderr = collect(err_read, spec.limits.output_bytes);

        let wait_ms = u32::try_from(spec.limits.wall_time.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: valid handle.
        let waited = unsafe { WaitForSingleObject(raw(&process), wait_ms) };
        let timed_out = waited == WAIT_TIMEOUT;
        // Whatever is left of the tree ends with the main process.
        // SAFETY: valid handle.
        unsafe { TerminateJobObject(raw(&job), KILLED_EXIT_CODE) };
        if waited != WAIT_OBJECT_0 && !timed_out {
            return Err(last("WaitForSingleObject"));
        }
        if timed_out {
            // SAFETY: valid handle.
            unsafe { WaitForSingleObject(raw(&process), 5_000) };
        }
        let mut code = 0u32;
        // SAFETY: valid handle.
        unsafe { GetExitCodeProcess(raw(&process), &mut code) };
        drop(job);
        let (stdout, out_truncated) = stdout.join().unwrap_or_default();
        let (stderr, err_truncated) = stderr.join().unwrap_or_default();
        Ok(Outcome {
            exit_code: code,
            stdout,
            stderr,
            truncated: out_truncated || err_truncated,
            timed_out,
            duration: started.elapsed(),
        })
    })();
    // SAFETY: initialised above; the buffer is still alive.
    unsafe { DeleteProcThreadAttributeList(list_ptr) };
    result
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    use std::os::windows::io::AsRawHandle;
    handle.as_raw_handle() as HANDLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_quoted_for_the_command_line() {
        let q = |s: &str| String::from_utf16(&quote(OsStr::new(s))).unwrap();
        assert_eq!(q("plain"), "plain");
        assert_eq!(q("two words"), "\"two words\"");
        assert_eq!(q(""), "\"\"");
        assert_eq!(q(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(q(r"C:\dir with space\"), r#""C:\dir with space\\""#);
    }

    #[test]
    fn profile_names_are_checked() {
        assert!(container_sid("bad name").is_err());
        assert!(container_sid("").is_err());
    }
}
