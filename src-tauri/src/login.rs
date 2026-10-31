//! In-app `devtunnel user login` (browser or device-code flow), reporting progress as `LoginState`.

use crate::cli;
use crate::model::{LoginProvider, LoginState};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Notify;

pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Runs the login to completion, calling `on_state` for each transition, and returns the final state.
/// `cancel` stops the CLI (the account is left unchanged).
pub async fn run(
    cli: &Path,
    provider: LoginProvider,
    device_code: bool,
    cancel: Arc<Notify>,
    timeout: Duration,
    on_state: impl Fn(&LoginState),
    on_line: impl Fn(&str),
) -> LoginState {
    on_state(&LoginState::Running { provider });
    let mut cmd = Command::new(cli);
    cmd.args(cli::login_args(provider, device_code))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return finish(&on_state, LoginState::Failed { message: format!("failed to start devtunnel: {e}") }),
    };
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut err = BufReader::new(child.stderr.take().unwrap()).lines();
    let (mut out_done, mut err_done) = (false, false);
    let mut lines: Vec<String> = Vec::new();
    let mut seen_code = false;
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    let mut handle = |l: String, lines: &mut Vec<String>| {
        on_line(&l);
        if !seen_code && let Some((url, code)) = cli::parse_device_code(&l) {
            seen_code = true;
            on_state(&LoginState::DeviceCode { provider, url, code });
        }
        lines.push(l);
    };

    let status = loop {
        tokio::select! {
            l = out.next_line(), if !out_done => match l {
                Ok(Some(l)) => handle(l, &mut lines),
                _ => out_done = true,
            },
            l = err.next_line(), if !err_done => match l {
                Ok(Some(l)) => handle(l, &mut lines),
                _ => err_done = true,
            },
            s = child.wait() => break s.ok(),
            _ = cancel.notified() => {
                let _ = child.kill().await;
                return finish(&on_state, LoginState::Failed { message: "Sign-in cancelled".into() });
            }
            _ = &mut deadline => {
                let _ = child.kill().await;
                return finish(&on_state, LoginState::Failed { message: "Sign-in timed out".into() });
            }
        }
    };
    // Drain anything printed right before exit.
    let drain = async {
        while !out_done || !err_done {
            tokio::select! {
                l = out.next_line(), if !out_done => match l { Ok(Some(l)) => handle(l, &mut lines), _ => out_done = true },
                l = err.next_line(), if !err_done => match l { Ok(Some(l)) => handle(l, &mut lines), _ => err_done = true },
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_millis(500), drain).await;
    finish(&on_state, outcome(status.is_some_and(|s| s.success()), &lines))
}

fn finish(on_state: &impl Fn(&LoginState), s: LoginState) -> LoginState {
    on_state(&s);
    s
}

/// Maps the CLI's exit status and output to the final state.
pub fn outcome(success: bool, lines: &[String]) -> LoginState {
    let logged_in = lines.iter().map(|l| l.trim()).find(|l| l.starts_with("Logged in as"));
    if success {
        return LoginState::Succeeded { message: logged_in.unwrap_or("Signed in").to_string() };
    }
    // stdout/stderr interleave nondeterministically, so prefer an explicit error line.
    let meaningful = || lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with("Request ID:"));
    let message = meaningful()
        .rfind(|l| l.to_ascii_lowercase().contains("error") || l.to_ascii_lowercase().contains("fail"))
        .or_else(|| meaningful().next_back())
        .unwrap_or("Sign-in failed")
        .to_string();
    LoginState::Failed { message }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn fake_cli(dir: &Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("devtunnel");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn run_fake(body: &str, device: bool, cancel_after: Option<Duration>) -> (LoginState, Vec<LoginState>) {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(dir.path(), body);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let cancel = Arc::new(Notify::new());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let c2 = cancel.clone();
        let fin = rt.block_on(async move {
            if let Some(d) = cancel_after {
                tokio::spawn(async move {
                    tokio::time::sleep(d).await;
                    c2.notify_one();
                });
            }
            run(&cli, LoginProvider::Microsoft, device, cancel, Duration::from_secs(20), move |s| s2.lock().unwrap().push(s.clone()), |_| {}).await
        });
        let seen = seen.lock().unwrap().clone();
        (fin, seen)
    }

    #[test]
    fn device_code_flow_succeeds() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/login_device_code.txt");
        // Fails unless invoked as `user login -e -d`.
        let body = format!("[ \"$*\" = 'user login -e -d' ] || {{ echo \"bad args: $*\" >&2; exit 9; }}\ncat '{fixture}'\nsleep 0.3\necho 'Logged in as a@b.com using Microsoft.'");
        let (fin, seen) = run_fake(&body, true, None);
        assert_eq!(fin, LoginState::Succeeded { message: "Logged in as a@b.com using Microsoft.".into() });
        assert_eq!(seen.first(), Some(&LoginState::Running { provider: LoginProvider::Microsoft }));
        assert!(seen.contains(&LoginState::DeviceCode {
            provider: LoginProvider::Microsoft,
            url: "https://microsoft.com/devicelogin".into(),
            code: "F7QRSTUVW".into()
        }));
        assert_eq!(seen.last(), Some(&fin));
    }

    #[test]
    fn failure_reports_last_error_line() {
        let (fin, _) = run_fake("echo 'Starting login' \necho 'Error: AADSTS70016 authorization_pending expired' >&2\nexit 1", false, None);
        assert_eq!(fin, LoginState::Failed { message: "Error: AADSTS70016 authorization_pending expired".into() });
    }

    #[test]
    fn cancel_kills_cli() {
        let pidfile = tempfile::NamedTempFile::new().unwrap();
        let pf = pidfile.path().display().to_string();
        let (fin, _) = run_fake(&format!("echo $$ > '{pf}'\nexec sleep 30"), false, Some(Duration::from_millis(400)));
        assert_eq!(fin, LoginState::Failed { message: "Sign-in cancelled".into() });
        let pid: u32 = std::fs::read_to_string(pidfile.path()).unwrap().trim().parse().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(!crate::supervisor::alive(pid), "login CLI should be killed");
    }

    #[test]
    fn outcome_without_logged_in_line() {
        assert_eq!(outcome(true, &[]), LoginState::Succeeded { message: "Signed in".into() });
        assert_eq!(outcome(false, &[]), LoginState::Failed { message: "Sign-in failed".into() });
    }
}
