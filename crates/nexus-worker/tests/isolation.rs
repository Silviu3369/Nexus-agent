//! ISO-01 / ISO-02 (`SECURITATE_SI_VALIDARE_RO.md`) against the real
//! backend: the probe runs in the strict container and tries what the
//! container must not allow. Every denial is paired with the same probe run
//! outside the container, so a "denied" is the container's doing and not an
//! accident of the test (a missing file, a closed port).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nexus_worker::{Access, ContainerSpec, Grant, Limits};
use serde_json::Value;

const PROFILE: &str = "Nexus.Worker.Test";

struct Sandbox {
    root: PathBuf,
    work: PathBuf,
    bin: PathBuf,
    outside: PathBuf,
    probe: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("nexus-iso-{}", uuid::Uuid::new_v4()));
        let (work, bin, outside) = (root.join("work"), root.join("bin"), root.join("outside"));
        for d in [&work, &bin, &outside] {
            std::fs::create_dir_all(d).unwrap();
        }
        // The probe runs from a folder the container may read, not from
        // the build directory.
        let probe = bin.join("nexus-probe.exe");
        std::fs::copy(env!("CARGO_BIN_EXE_nexus-probe"), &probe).unwrap();
        std::fs::write(outside.join("secret.txt"), b"not for the container").unwrap();
        std::fs::write(work.join("input.txt"), b"for the container").unwrap();
        Self {
            root,
            work,
            bin,
            outside,
            probe,
        }
    }

    fn spec(&self, args: &[&str]) -> ContainerSpec {
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        ContainerSpec {
            profile: PROFILE.to_string(),
            lpac: true,
            network: false,
            // What child processes need under LPAC (Windows reads policy
            // from the registry while creating a process).
            capabilities: vec!["registryRead".to_string()],
            grants: vec![
                Grant {
                    path: self.work.clone(),
                    access: Access::ReadWrite,
                },
                Grant {
                    path: self.bin.clone(),
                    access: Access::Read,
                },
            ],
            program: self.probe.clone(),
            args: args.iter().map(OsString::from).collect(),
            cwd: self.work.clone(),
            env: vec![
                ("SystemRoot".into(), system_root),
                ("TEMP".into(), self.work.clone().into_os_string()),
                ("TMP".into(), self.work.clone().into_os_string()),
            ],
            limits: Limits {
                wall_time: Duration::from_secs(30),
                ..Limits::default()
            },
        }
    }

    /// The probe in the container.
    fn inside(&self, args: &[&str]) -> Value {
        self.inside_with(self.spec(args))
    }

    fn inside_with(&self, spec: ContainerSpec) -> Value {
        let out = nexus_worker::run(&spec).expect("the container starts");
        let text = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str(text.lines().last().unwrap_or("")).unwrap_or_else(|_| {
            panic!(
                "no probe answer (exit {:#x}): {text} / {}",
                out.exit_code,
                String::from_utf8_lossy(&out.stderr)
            )
        })
    }

    /// The same probe as an ordinary process: the control.
    fn outside(&self, args: &[&str]) -> Value {
        let out = std::process::Command::new(&self.probe)
            .args(args)
            .output()
            .unwrap();
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn denied(v: &Value) -> bool {
    v["ok"] == false
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn the_probe_runs_as_a_less_privileged_app_container() {
    let s = Sandbox::new();
    let v = s.inside(&["token"]);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["app_container"], true, "{v}");
    assert_eq!(s.outside(&["token"])["app_container"], false);
}

#[test]
fn only_granted_paths_can_be_read_or_written() {
    let s = Sandbox::new();
    let secret = s.outside.join("secret.txt");
    assert!(
        !denied(&s.outside(&["read", p(&secret)])),
        "control: readable by an ordinary process"
    );
    assert!(
        denied(&s.inside(&["read", p(&secret)])),
        "an ungranted file"
    );
    assert_eq!(
        s.inside(&["read", p(&s.work.join("input.txt"))])["ok"],
        true
    );
    assert_eq!(s.inside(&["write", p(&s.work.join("out.txt"))])["ok"], true);
    assert!(
        s.work.join("out.txt").exists(),
        "the working folder is writable"
    );
    assert!(
        denied(&s.inside(&["write", p(&s.bin.join("x.txt"))])),
        "a read-only grant stays read-only"
    );
    assert!(denied(&s.inside(&["write", p(&s.outside.join("x.txt"))])));
    assert!(!s.outside.join("x.txt").exists());

    // The user's own folders and Nexus's data.
    let profile = std::env::var("USERPROFILE").unwrap();
    let local = std::env::var("LOCALAPPDATA").unwrap();
    let under = |base: &str, name: &str| Path::new(base).join(name).display().to_string();
    for dir in [
        profile.clone(),
        under(&profile, "Documents"),
        under(&profile, "Desktop"),
        under(&local, "Nexus"),
    ] {
        if !Path::new(&dir).exists() {
            continue;
        }
        assert!(!denied(&s.outside(&["list", &dir])), "control: {dir}");
        assert!(denied(&s.inside(&["list", &dir])), "{dir}");
    }
    let system = std::env::var("SystemRoot").unwrap();
    assert!(denied(
        &s.inside(&["write", &under(&system, "nexus-probe.txt")])
    ));
}

#[test]
fn the_users_credentials_are_out_of_reach() {
    let s = Sandbox::new();
    let control = s.outside(&["creds", "*"]);
    let inside = s.inside(&["creds", "*"]);
    assert!(
        denied(&inside) || inside["count"] == 0,
        "Credential Manager from the container: {inside} (outside: {control})"
    );
}

#[test]
fn this_machine_is_unreachable_and_the_internet_only_when_allowed() {
    let s = Sandbox::new();
    let listener = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for c in listener.incoming() {
            drop(c);
        }
    });
    let loopback = format!("127.0.0.1:{port}");
    assert!(
        !denied(&s.outside(&["tcp", &loopback])),
        "control: the port is open"
    );
    assert!(denied(&s.inside(&["tcp", &loopback])));
    let mut allowed = s.spec(&["tcp", &loopback]);
    allowed.network = true;
    assert!(
        denied(&s.inside_with(allowed)),
        "loopback stays closed even with the internet allowed"
    );
    // This machine's own network address (no packet is sent to learn it).
    let udp = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    if udp.connect("192.0.2.1:9").is_ok()
        && let Ok(local) = udp.local_addr()
        && !local.ip().is_unspecified()
    {
        let own = format!("{}:{port}", local.ip());
        let mut allowed = s.spec(&["tcp", &own]);
        allowed.network = true;
        assert!(!denied(&s.outside(&["tcp", &own])), "control: {own}");
        assert!(
            denied(&s.inside_with(allowed)),
            "this machine's address is closed too"
        );
    }
    // Without the capability, Windows refuses the connection itself.
    let v = s.inside(&["tcp", "1.1.1.1:443"]);
    assert!(denied(&v), "{v}");
    assert_eq!(
        v["code"], 10013,
        "refused locally (WSAEACCES), not by the network: {v}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_hosts_pipe_and_other_processes_are_out_of_reach() {
    let name = format!(r"\\.\pipe\nexus-iso-{}", uuid::Uuid::new_v4());
    let _server = nexus_host::bind_first_instance(&name).expect("a host pipe");
    let s = Sandbox::new();
    let pipe = name.clone();
    let inside = tokio::task::spawn_blocking(move || {
        let s2 = Sandbox::new();
        s2.inside(&["pipe", &pipe])
    })
    .await
    .unwrap();
    assert!(denied(&inside), "{inside}");
    assert_eq!(
        inside["code"], 5,
        "access denied by the pipe's security: {inside}"
    );
    assert!(
        !denied(&s.outside(&["pipe", &name])),
        "control: an ordinary process of this user connects"
    );

    let me = std::process::id().to_string();
    assert!(!denied(&s.outside(&["open-process", &me])), "control");
    assert!(denied(&s.inside(&["open-process", &me])));
}

#[test]
fn the_environment_is_only_what_the_spec_gives() {
    let s = Sandbox::new();
    let v = s.inside(&["env"]);
    let mut names: Vec<String> = v["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_uppercase())
        .collect();
    names.sort();
    assert_eq!(names, ["LOCALAPPDATA", "SYSTEMROOT", "TEMP", "TMP"]);
}

#[test]
fn the_whole_tree_is_held_to_its_limits() {
    let s = Sandbox::new();
    let small = |args: &[&str], limits: Limits| {
        let mut spec = s.spec(args);
        spec.limits = limits;
        nexus_worker::run(&spec).expect("the container starts")
    };
    let answer = |out: &nexus_worker::Outcome| -> Value {
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap_or(Value::Null)
    };

    // Memory, across the tree.
    let memory = Limits {
        memory_bytes: 64 * 1024 * 1024,
        wall_time: Duration::from_secs(30),
        ..Limits::default()
    };
    assert_eq!(answer(&small(&["alloc", "16"], memory))["ok"], true);
    assert_eq!(answer(&small(&["alloc", "256"], memory))["ok"], false);

    // Processes alive at once.
    let three = Limits {
        processes: 3,
        wall_time: Duration::from_secs(30),
        ..Limits::default()
    };
    let spawned = answer(&small(&["spawn", "5"], three));
    assert_eq!(
        spawned["started"], 2,
        "the probe and two children: {spawned}"
    );

    // Wall time.
    let wall = Limits {
        wall_time: Duration::from_secs(2),
        ..Limits::default()
    };
    let out = small(&["sleep", "30000"], wall);
    assert!(out.timed_out);
    assert!(out.duration < Duration::from_secs(10), "{:?}", out.duration);

    // CPU time.
    let cpu = Limits {
        cpu_time: Duration::from_secs(1),
        wall_time: Duration::from_secs(30),
        ..Limits::default()
    };
    let out = small(&["busy", "10000"], cpu);
    assert!(!out.timed_out);
    assert!(out.duration < Duration::from_secs(8), "{:?}", out.duration);
    assert_ne!(out.exit_code, 0, "ended by the job");

    // Output.
    let output = Limits {
        output_bytes: 10_000,
        wall_time: Duration::from_secs(30),
        ..Limits::default()
    };
    let out = small(&["output", "1000000"], output);
    assert!(out.truncated);
    assert!(out.stdout.len() <= 10_000);

    // Nothing outlives the main process.
    let out = small(&["child-and-exit"], Limits::default());
    let child = answer(&out)["child"].as_u64().expect("a child was started");
    let gone = s.outside(&["open-process", &child.to_string()]);
    assert!(denied(&gone), "the child ended with the tree: {gone}");
}
