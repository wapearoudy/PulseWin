//! Running one extension, and turning what it printed into a reading.
//!
//! Split from [`crate::extension`] the way the original splits
//! `ExtensionUsageService` from `ExtensionCatalog`: finding a program and
//! running it are two different mistakes to make, and only this half starts a
//! process.
//!
//! **Bounded, in both directions.** A program that runs past its time limit is
//! stopped rather than waited on — every other ring is behind it — and a
//! program that prints past the output ceiling has the rest dropped rather than
//! being allowed to grow PulseWin's memory. The pipe is still drained after the
//! ceiling, because a reader that walks away from a full pipe buffer is the
//! deadlock it was trying to avoid.

use std::ffi::{OsStr, OsString};
use std::process::Stdio;

use tokio::io::AsyncReadExt;

use crate::extension::{is_executable, Extension, ExtensionCatalog, ExtensionReport, MISSING, STOPPED, TIMED_OUT};
use crate::model::ProviderUsage;

/// A console window must not flash up in front of the panel.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Run one extension and report what it said.
///
/// The program is checked again here, not only when the folder was scanned: the
/// scan was at launch, and the program may have gone since — deleted, replaced
/// by a folder, or moved to a drive that is no longer mounted. A ring that
/// silently disappears is worse than one that says why it is empty.
pub async fn fetch(extension: &Extension) -> ProviderUsage {
    let account = extension.account_id();
    let name = extension.name.as_str();

    if !is_executable(&extension.executable) {
        return ProviderUsage::failed(&account, name, MISSING);
    }

    match run(extension).await {
        Outcome::Printed(stdout) => ExtensionReport::reading(&stdout, &account, name),
        Outcome::CouldNotStart => ProviderUsage::failed(&account, name, MISSING),
        Outcome::TimedOut => ProviderUsage::failed(&account, name, TIMED_OUT),
        Outcome::Exited => ProviderUsage::failed(&account, name, STOPPED),
    }
}

enum Outcome {
    Printed(Vec<u8>),
    CouldNotStart,
    TimedOut,
    Exited,
}

async fn run(extension: &Extension) -> Outcome {
    let child = match command(extension).spawn() {
        Ok(child) => child,
        Err(_) => return Outcome::CouldNotStart,
    };

    match tokio::time::timeout(extension.timeout, collect(child)).await {
        Ok(collected) if collected.success => Outcome::Printed(collected.stdout),
        Ok(_) => Outcome::Exited,
        // The child is dropped here, and `kill_on_drop` takes it with it.
        Err(_) => Outcome::TimedOut,
    }
}

/// The command that runs an extension's program.
///
/// **A batch file is not a program.** `CreateProcess` cannot start `.cmd` or
/// `.bat` on its own, so Windows gets `cmd.exe /c` — which is also why
/// `COMSPEC` and `PATHEXT` are in the environment below.
fn command(extension: &Extension) -> tokio::process::Command {
    let batch = extension
        .executable
        .extension()
        .map(|extension| {
            let extension = extension.to_string_lossy().to_ascii_lowercase();
            extension == "cmd" || extension == "bat"
        })
        .unwrap_or(false);

    let mut command = if batch {
        let mut command = tokio::process::Command::new("cmd.exe");
        command.arg("/c").arg(&extension.executable);
        command
    } else {
        tokio::process::Command::new(&extension.executable)
    };

    command
        .env_clear()
        .envs(environment(extension, &inherited()))
        // No arguments and an empty stdin: the contract is that the program
        // asks for nothing. A program that reads stdin gets EOF rather than
        // blocking on a terminal that is not there.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // The extension's own folder, so a program that ships data beside
        // itself finds it by a relative path.
        .current_dir(&extension.directory)
        // A run that is dropped — the deadline below — takes the child with
        // it, so a program that ignores everything does not stay running.
        .kill_on_drop(true);

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    command
}

/// What PulseWin itself was started with, before any of it is filtered.
fn inherited() -> Vec<(OsString, OsString)> {
    std::env::vars_os().collect()
}

/// **Only what a program needs to run, and the proxy.**
///
/// Not PulseWin's own environment: whatever a launcher or a terminal happened
/// to leave in it — a token exported for some other tool included — is nobody's
/// business but the tool it was meant for. A program that needs a credential
/// owns its own login, and this is what makes that true rather than merely
/// intended.
///
/// `PATH` is set rather than passed on, because the inherited one says where
/// *this* process looks, which is not where a script's `#!/usr/bin/env node`
/// needs to look. The proxy is passed on in any case it was written in, because
/// a program behind a corporate proxy that cannot reach the network has no way
/// to tell the reader that is why.
pub fn environment(extension: &Extension, inherited: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
    /// What a program needs to find its own files, its own login and its own
    /// temporary space — and nothing that names PulseWin's.
    const PASSED: [&str; 12] = [
        "HOME",
        "USERPROFILE",
        "USERNAME",
        "TEMP",
        "TMP",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "SHELL",
        "COMSPEC",
        "SystemRoot",
        "PATHEXT",
    ];
    const PROXIES: [&str; 4] = ["http_proxy", "https_proxy", "all_proxy", "no_proxy"];

    let mut environment: Vec<(OsString, OsString)> = inherited
        .iter()
        .filter(|(name, _)| kept(name, &PASSED, &PROXIES))
        .cloned()
        .collect();

    environment.push((OsString::from("PATH"), system_path()));
    environment.push((OsString::from("PULSE_EXTENSION_ID"), OsString::from(&extension.id)));
    environment.push((
        OsString::from("PULSE_EXTENSION_SCHEMA"),
        OsString::from(ExtensionCatalog::schema_version().to_string()),
    ));

    // A proxy set in PulseWin's own settings has to reach the program too:
    // it is the only place a Windows user behind a local proxy ever says so.
    crate::preferences::load().network_proxy.process_environment(environment)
}

fn kept(name: &OsStr, passed: &[&str], proxies: &[&str]) -> bool {
    let name = name.to_string_lossy();
    !name.is_empty()
        && (passed.iter().any(|kept| name.eq_ignore_ascii_case(kept))
            || proxies.iter().any(|kept| name.eq_ignore_ascii_case(kept)))
}

/// Where a program's own dependencies are going to be found.
///
/// The Windows list is the one Windows itself puts in a fresh process — plus
/// the OpenSSH folder, because a program that talks to a service over its own
/// CLI often shells out to `ssh`. It is built from `SystemRoot` rather than
/// copied from `PATH`, so a `PATH` someone widened for their terminal does not
/// widen what an extension can reach.
#[cfg(windows)]
fn system_path() -> OsString {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
    let root = PathBuf::from(root);
    let folders = [
        root.join("system32"),
        root.clone(),
        root.join("System32").join("Wbem"),
        root.join("System32").join("WindowsPowerShell").join("v1.0"),
        root.join("System32").join("OpenSSH"),
    ];
    std::env::join_paths(folders).unwrap_or_default()
}

/// The original's own list: the system and both Homebrew prefixes, which is
/// where a script's `#!/usr/bin/env node` or `python3` is going to be found.
#[cfg(not(windows))]
fn system_path() -> OsString {
    OsString::from("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
}

struct Collected {
    stdout: Vec<u8>,
    success: bool,
}

/// Drains both pipes at once, and then waits for the process.
///
/// **Both at once.** Reading one to EOF and only then reading the other
/// deadlocks the moment the child writes more than a pipe buffer to the one
/// nobody is reading — a debug build, a stack trace — and past the output
/// ceiling the bytes are dropped but the pipe is still drained, because a
/// reader that walks away is the same deadlock wearing a different hat.
async fn collect(mut child: tokio::process::Child) -> Collected {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let ceiling = ExtensionReport::output_ceiling();

    tokio::join!(drain(&mut out_pipe, &mut stdout, ceiling), drain(&mut err_pipe, &mut stderr, ceiling));

    // The exit status is all that is read of stderr: what a program wrote there
    // is its own business, and the reader is told it stopped, not what it said.
    let success = child.wait().await.map(|status| status.success()).unwrap_or(false);
    Collected { stdout, success }
}

async fn drain<R>(pipe: &mut Option<R>, into: &mut Vec<u8>, ceiling: usize)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(pipe) = pipe.as_mut() else { return };
    let mut chunk = [0u8; 8 * 1024];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if into.len() < ceiling {
                    into.extend_from_slice(&chunk[..read]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use super::*;
    use crate::extension::account_id;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let suffix = format!("{label}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let candidate = std::env::temp_dir().join(format!("pulsewin-{suffix}"));
        if fs::create_dir_all(&candidate).is_ok() {
            return candidate;
        }
        // Sandboxed test binaries can be denied writes outside the workspace.
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!("pulsewin-{suffix}"));
        fs::create_dir_all(&fallback).unwrap_or_else(|error| panic!("create_dir_all {}: {error:?}", fallback.display()));
        fallback
    }

    /// One extension folder with a real manifest and a real program, scanned
    /// the way the app scans it — so the test exercises the same two halves.
    #[cfg(unix)]
    fn service(label: &str, script: &str, timeout: Option<f64>) -> (Extension, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir(label);
        let directory = root.join("acme");
        fs::create_dir_all(&directory).unwrap();
        let timeout = timeout.map(|seconds| format!(r#","timeoutSeconds":{seconds}"#)).unwrap_or_default();
        fs::write(
            directory.join(ExtensionCatalog::manifest_name()),
            format!(r#"{{"schemaVersion":1,"id":"acme","name":"Acme","executable":"run"{timeout}}}"#),
        )
        .unwrap();
        let program = directory.join("run");
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let scan = ExtensionCatalog::scan_in(&root);
        assert_eq!(scan.problems, vec![], "the fixture has to scan cleanly");
        (scan.extensions.into_iter().next().unwrap(), root)
    }

    fn plain_extension() -> Extension {
        Extension {
            id: "acme".into(),
            name: "Acme".into(),
            directory: PathBuf::from("/tmp/acme"),
            executable: PathBuf::from("/tmp/acme/run"),
            timeout: std::time::Duration::from_secs(20),
        }
    }

    #[test]
    fn only_what_a_program_needs_and_the_proxy_is_passed_on() {
        let pairs = |items: &[(&str, &str)]| -> Vec<(OsString, OsString)> {
            items.iter().map(|(key, value)| (OsString::from(key), OsString::from(value))).collect()
        };
        let environment = environment(
            &plain_extension(),
            &pairs(&[
                ("HOME", "/Users/me"),
                ("LANG", "en_US.UTF-8"),
                ("HTTPS_PROXY", "http://proxy:8080"),
                ("OPENAI_API_KEY", "sk-secret"),
                ("GITHUB_TOKEN", "ghp_secret"),
                ("PATH", "/somewhere/odd"),
            ]),
        );
        let get = |key: &str| {
            environment
                .iter()
                .find(|(name, _)| name.to_string_lossy() == key)
                .map(|(_, value)| value.to_string_lossy().into_owned())
        };

        assert_eq!(get("HOME").as_deref(), Some("/Users/me"));
        assert_eq!(get("LANG").as_deref(), Some("en_US.UTF-8"));
        assert_eq!(get("HTTPS_PROXY").as_deref(), Some("http://proxy:8080"), "a proxy in any case is passed on");
        assert_eq!(get("OPENAI_API_KEY"), None, "a token exported for another tool is not the extension's");
        assert_eq!(get("GITHUB_TOKEN"), None);
        assert_eq!(get("PULSE_EXTENSION_ID").as_deref(), Some("acme"));
        assert_eq!(get("PULSE_EXTENSION_SCHEMA").as_deref(), Some("1"));

        // `PATH` is set, not inherited.
        let path = get("PATH").unwrap();
        assert_ne!(path, "/somewhere/odd");
        #[cfg(unix)]
        assert!(path.starts_with("/opt/homebrew/bin:"), "{path}");
        #[cfg(windows)]
        assert!(path.to_lowercase().contains("system32"), "{path}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_report_becomes_a_reading() {
        let (extension, root) = service(
            "runner-success",
            "#!/bin/sh\nprintf '%s' '{\"schemaVersion\":1,\"limits\":[{\"label\":\"Quota\",\"usedPercent\":64}]}'\n",
            None,
        );
        let usage = fetch(&extension).await;
        assert!(usage.error.is_none(), "{:?}", usage.error);
        assert_eq!(usage.id, account_id("acme"));
        assert_eq!(usage.name, "Acme");
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].percent_used, Some(64.0));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn it_runs_in_its_own_folder_with_the_two_variables_it_is_promised() {
        // Prints the good report only when everything the contract promises
        // holds; anything else is a reply PulseWin cannot read.
        let (extension, root) = service(
            "runner-environment",
            "#!/bin/sh\nif [ \"$PULSE_EXTENSION_ID\" = acme ] && [ \"$PULSE_EXTENSION_SCHEMA\" = 1 ] && [ -f pulse-extension.json ]; then\n  printf '%s' '{\"schemaVersion\":1,\"balance\":{\"amount\":3,\"currency\":\"USD\"}}'\nelse\n  echo wrong\nfi\n",
            None,
        );
        let usage = fetch(&extension).await;
        assert!(usage.error.is_none(), "{:?}", usage.error);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 3.0);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_program_that_stops_with_an_error_says_so() {
        let (extension, root) = service("runner-failed", "#!/bin/sh\necho oops >&2\nexit 3\n", None);
        let usage = fetch(&extension).await;
        assert_eq!(usage.error.as_deref(), Some(STOPPED));
        assert!(usage.windows.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_program_that_runs_past_its_limit_is_stopped() {
        let (extension, root) = service(
            "runner-timeout",
            "#!/bin/sh\necho $$ > pidfile\nexec sleep 300\n",
            Some(1.0),
        );
        let started = Instant::now();
        let usage = fetch(&extension).await;
        assert_eq!(usage.error.as_deref(), Some(TIMED_OUT));
        assert!(started.elapsed() < std::time::Duration::from_secs(120), "the deadline has to hold");

        // The program is gone, not merely ignored: it was killed with the run.
        let pid = fs::read_to_string(root.join("acme").join("pidfile")).unwrap().trim().to_owned();
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while Instant::now() < deadline {
            let alive = std::process::Command::new("/bin/ps")
                .args(["-p", &pid])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if !alive {
                let _ = fs::remove_dir_all(root);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("the program was still running ten seconds after its deadline");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_program_deleted_since_the_scan_is_reported_missing() {
        let (extension, root) = service(
            "runner-deleted",
            "#!/bin/sh\nprintf '%s' '{\"schemaVersion\":1,\"limits\":[{\"label\":\"Quota\",\"usedPercent\":1}]}'\n",
            None,
        );
        fs::remove_dir_all(&root).unwrap();
        let usage = fetch(&extension).await;
        assert_eq!(usage.error.as_deref(), Some(MISSING));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_past_the_ceiling_is_dropped_and_the_pipe_is_still_drained() {
        // More than the ceiling, then a well-formed report past it: the run
        // finishes (so nothing deadlocked) and what was kept cannot parse.
        let (extension, root) = service(
            "runner-ceiling",
            "#!/bin/sh\nhead -c 400000 /dev/zero | tr '\\0' 'x'\nprintf '%s' '{\"schemaVersion\":1}'\n",
            None,
        );
        let usage = fetch(&extension).await;
        assert!(usage.error.is_some(), "a truncated report cannot be read as one");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_batch_file_is_wrapped_in_cmd() {
        // Not run here: `cmd.exe` is Windows'. The wrap is what matters, and it
        // is decided by the extension alone.
        let mut extension = plain_extension();
        extension.executable = Path::new("C:\\Users\\me\\Extensions\\acme\\run.cmd").to_path_buf();
        let command = command(&extension);
        let debug = format!("{command:?}");
        assert!(debug.contains("cmd.exe") || cfg!(not(windows)), "{debug}");
        let _ = Path::new("C:\\Users\\me\\Extensions\\acme\\run.exe");
    }
}
