//! Supervises one `devtunnel connect|host <id>` child process per tunnel (one Supervisor per role).

use crate::logs::LogStore;
use crate::model::{ConnState, Forward, Role};
use crate::parser::{parse_line, CliEvent};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Notify;

const MAX_BACKOFF_SECS: u64 = 60;
const STOP_GRACE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub enum SupEvent {
    Changed { role: Role, id: String, prev: ConnState, next: ConnState },
    Log { id: String, line: crate::model::LogLine },
}

pub type Listener = Arc<dyn Fn(SupEvent) + Send + Sync>;

struct Managed {
    state: ConnState,
    forwards: BTreeSet<Forward>,
    desired: bool,
    generation: u64,
    running: bool,
    pid: Option<u32>,
    wake: Arc<Notify>,
}

impl Managed {
    fn new() -> Self {
        Self {
            state: ConnState::Disconnected,
            forwards: BTreeSet::new(),
            desired: false,
            generation: 0,
            running: false,
            pid: None,
            wake: Arc::new(Notify::new()),
        }
    }
}

#[derive(Clone)]
pub struct Supervisor {
    role: Role,
    inner: Arc<Mutex<HashMap<String, Managed>>>,
    logs: Arc<LogStore>,
    listener: Listener,
    /// Records managed child PIDs so orphans can be reclaimed if the app dies without cleanup.
    pidfile: Option<Arc<PathBuf>>,
}

pub fn backoff_secs(attempt: u32) -> u64 {
    2u64.saturating_pow(attempt.saturating_sub(1).min(16)).min(MAX_BACKOFF_SECS)
}

enum RunEnd {
    Stopped,
    Exited { message: String, auth: bool, was_connected: bool },
    SpawnFailed(String),
}

impl Supervisor {
    pub fn new(logs: Arc<LogStore>, listener: Listener) -> Self {
        Self::with_role(Role::Connect, logs, listener)
    }

    pub fn with_role(role: Role, logs: Arc<LogStore>, listener: Listener) -> Self {
        Self { role, inner: Arc::new(Mutex::new(HashMap::new())), logs, listener, pidfile: None }
    }

    pub fn with_pidfile(mut self, path: PathBuf) -> Self {
        self.pidfile = Some(Arc::new(path));
        self
    }

    fn persist_pids(&self) {
        let Some(path) = &self.pidfile else { return };
        let map: BTreeMap<String, u32> =
            self.inner.lock().unwrap().iter().filter_map(|(k, m)| m.pid.map(|p| (k.clone(), p))).collect();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path.as_ref(), serde_json::to_vec(&map).unwrap_or_default());
    }

    pub fn state(&self, id: &str) -> (ConnState, Vec<Forward>) {
        let m = self.inner.lock().unwrap();
        m.get(id)
            .map(|x| (x.state.clone(), x.forwards.iter().cloned().collect()))
            .unwrap_or((ConnState::Disconnected, vec![]))
    }

    pub fn managed_ids(&self) -> Vec<String> {
        self.inner.lock().unwrap().iter().filter(|(_, m)| m.desired || m.running).map(|(k, _)| k.clone()).collect()
    }

    pub fn managed_pids(&self) -> Vec<u32> {
        self.inner.lock().unwrap().values().filter_map(|m| m.pid).collect()
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.inner.lock().unwrap().get(id).map(|m| m.desired || m.running).unwrap_or(false)
    }

    fn set_state(&self, id: &str, generation: Option<u64>, next: ConnState) {
        let prev = {
            let mut m = self.inner.lock().unwrap();
            let e = m.entry(id.to_string()).or_insert_with(Managed::new);
            if generation.is_some_and(|g| g != e.generation) {
                return;
            }
            if !matches!(next, ConnState::Connected | ConnState::Reconnecting) {
                e.forwards.clear();
            }
            if e.state == next {
                return;
            }
            std::mem::replace(&mut e.state, next.clone())
        };
        (self.listener)(SupEvent::Changed { role: self.role, id: id.to_string(), prev, next });
    }

    fn log(&self, id: &str, stream: &str, text: &str) {
        let stream = match self.role {
            Role::Connect => stream.to_string(),
            Role::Host => format!("host-{stream}"),
        };
        let line = self.logs.push(id, &stream, text);
        (self.listener)(SupEvent::Log { id: id.to_string(), line });
    }

    fn still_wanted(&self, id: &str, generation: u64) -> bool {
        self.inner.lock().unwrap().get(id).is_some_and(|m| m.desired && m.generation == generation)
    }

    pub fn connect(&self, id: &str, cli: PathBuf) {
        let (generation, wake) = {
            let mut m = self.inner.lock().unwrap();
            let e = m.entry(id.to_string()).or_insert_with(Managed::new);
            if e.desired && e.running {
                // Already running; a kick skips any pending backoff delay.
                e.wake.notify_one();
                return;
            }
            e.desired = true;
            e.generation += 1;
            e.running = true;
            // Fresh notifier per generation so a lingering previous run can't steal wakeups.
            e.wake = Arc::new(Notify::new());
            (e.generation, e.wake.clone())
        };
        let this = self.clone();
        let id = id.to_string();
        tauri::async_runtime::spawn(async move { this.run_loop(id, cli, generation, wake).await });
    }

    pub fn disconnect(&self, id: &str) {
        let (wake, running) = {
            let mut m = self.inner.lock().unwrap();
            let Some(e) = m.get_mut(id) else { return };
            e.desired = false;
            (e.wake.clone(), e.running)
        };
        if running {
            wake.notify_one();
        } else {
            self.set_state(id, None, ConnState::Disconnected);
        }
    }

    /// Skip backoff waits for all tunnels (e.g. after wake from sleep / network change).
    pub fn kick_all(&self) {
        let m = self.inner.lock().unwrap();
        for e in m.values().filter(|e| e.desired && matches!(e.state, ConnState::Backoff { .. })) {
            e.wake.notify_one();
        }
    }

    async fn run_loop(self, id: String, cli: PathBuf, generation: u64, wake: Arc<Notify>) {
        let mut attempt: u32 = 0;
        loop {
            if !self.still_wanted(&id, generation) {
                break;
            }
            self.set_state(&id, Some(generation), ConnState::Connecting);
            self.log(&id, "app", &format!("Starting: {} connect {id}", cli.display()));
            let message = match self.run_child(&id, &cli, generation, &wake).await {
                RunEnd::Stopped => break,
                RunEnd::Exited { auth: true, message, .. } => {
                    if let Some(e) = self.inner.lock().unwrap().get_mut(&id).filter(|e| e.generation == generation) {
                        e.desired = false;
                    }
                    self.log(&id, "app", &format!("Authentication required: {message}"));
                    self.set_state(&id, Some(generation), ConnState::AuthRequired { message });
                    self.finish(&id, generation, false);
                    return;
                }
                RunEnd::Exited { message, was_connected, .. } => {
                    if was_connected {
                        attempt = 0;
                    }
                    message
                }
                RunEnd::SpawnFailed(message) => message,
            };
            if !self.still_wanted(&id, generation) {
                break;
            }
            attempt += 1;
            let delay = backoff_secs(attempt);
            self.log(&id, "app", &format!("Exited ({message}); retrying in {delay}s (attempt {attempt})"));
            self.set_state(&id, Some(generation), ConnState::Backoff { attempt, retry_in_secs: delay, message });
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(delay)) => {}
                _ = wake.notified() => {}
            }
        }
        self.finish(&id, generation, true);
    }

    fn finish(&self, id: &str, generation: u64, set_disconnected: bool) {
        let current = {
            let mut m = self.inner.lock().unwrap();
            match m.get_mut(id) {
                Some(e) if e.generation == generation => {
                    e.running = false;
                    e.pid = None;
                    true
                }
                _ => false,
            }
        };
        if current {
            self.persist_pids();
        }
        if current && set_disconnected {
            self.log(id, "app", "Disconnected");
            self.set_state(id, Some(generation), ConnState::Disconnected);
        }
    }

    async fn run_child(&self, id: &str, cli: &PathBuf, generation: u64, wake: &Notify) -> RunEnd {
        let mut cmd = Command::new(cli);
        cmd.args([self.role.subcommand(), id]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(crate::win::CREATE_NO_WINDOW);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("failed to start devtunnel: {e}");
                self.log(id, "app", &msg);
                return RunEnd::SpawnFailed(msg);
            }
        };
        let pid = child.id();
        // Put the child in a kill-on-close Job Object so its whole tree dies with it, or with us if we crash.
        #[cfg(windows)]
        let _job = match pid.zip(child.raw_handle()) {
            Some((p, h)) => crate::win::JobGuard::attach(p, h as _).map_err(|e| self.log(id, "app", &e)).ok(),
            None => None,
        };
        {
            let mut m = self.inner.lock().unwrap();
            if let Some(e) = m.get_mut(id) {
                e.pid = pid;
            }
        }
        self.persist_pids();
        let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut err = BufReader::new(child.stderr.take().unwrap()).lines();
        let (mut out_done, mut err_done) = (false, false);
        let mut last_msg = String::new();
        let mut auth = false;
        let mut was_connected = false;

        let status = loop {
            tokio::select! {
                l = out.next_line(), if !out_done => match l {
                    Ok(Some(l)) => self.handle_line(id, generation, "out", &l, &mut last_msg, &mut auth, &mut was_connected),
                    _ => out_done = true,
                },
                l = err.next_line(), if !err_done => match l {
                    Ok(Some(l)) => self.handle_line(id, generation, "err", &l, &mut last_msg, &mut auth, &mut was_connected),
                    _ => err_done = true,
                },
                s = child.wait() => break s.ok(),
                _ = wake.notified() => {
                    if !self.still_wanted(id, generation) {
                        terminate(&mut child, pid).await;
                        return RunEnd::Stopped;
                    }
                }
            }
            if auth {
                terminate(&mut child, pid).await;
                break None;
            }
        };
        // Drain whatever is left after exit.
        let drain = async {
            while !out_done || !err_done {
                tokio::select! {
                    l = out.next_line(), if !out_done => match l { Ok(Some(l)) => self.handle_line(id, generation, "out", &l, &mut last_msg, &mut auth, &mut was_connected), _ => out_done = true },
                    l = err.next_line(), if !err_done => match l { Ok(Some(l)) => self.handle_line(id, generation, "err", &l, &mut last_msg, &mut auth, &mut was_connected), _ => err_done = true },
                }
            }
        };
        let _ = tokio::time::timeout(Duration::from_millis(500), drain).await;
        if !self.still_wanted(id, generation) {
            return RunEnd::Stopped;
        }
        let code = status.and_then(|s| s.code()).map(|c| format!("exit code {c}")).unwrap_or_else(|| "terminated".into());
        let message = if last_msg.is_empty() { code } else { format!("{last_msg} ({code})") };
        RunEnd::Exited { message, auth, was_connected }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_line(
        &self,
        id: &str,
        generation: u64,
        stream: &str,
        line: &str,
        last_msg: &mut String,
        auth: &mut bool,
        was_connected: &mut bool,
    ) {
        self.log(id, stream, line);
        let Some(ev) = parse_line(line) else { return };
        match ev {
            CliEvent::Connected { .. } | CliEvent::Restored | CliEvent::Ready => {
                *was_connected = true;
                self.set_state(id, Some(generation), ConnState::Connected);
            }
            CliEvent::Forward { .. } | CliEvent::HostingPort { .. } => {
                let (local_port, remote_port) = match ev {
                    CliEvent::Forward { local_port, remote_port, .. } => (local_port, remote_port),
                    CliEvent::HostingPort { port } => (port, port),
                    _ => unreachable!(),
                };
                *was_connected = true;
                let added = {
                    let mut m = self.inner.lock().unwrap();
                    m.get_mut(id)
                        .filter(|e| e.generation == generation)
                        .map(|e| e.forwards.insert(Forward { local_port, remote_port }))
                        .unwrap_or(false)
                };
                if self.role == Role::Connect {
                    self.set_state(id, Some(generation), ConnState::Connected);
                }
                if added {
                    let s = self.state(id).0;
                    (self.listener)(SupEvent::Changed { role: self.role, id: id.to_string(), prev: s.clone(), next: s });
                }
            }
            CliEvent::Reconnecting => self.set_state(id, Some(generation), ConnState::Reconnecting),
            CliEvent::Closed { message } | CliEvent::Error { message } | CliEvent::Warning { message } => {
                *last_msg = message
            }
            CliEvent::AuthRequired { message } => {
                *last_msg = message;
                *auth = true;
            }
        }
    }

    /// Synchronously stops every managed child (used on app exit).
    pub fn shutdown(&self) {
        let pids: Vec<u32> = {
            let mut m = self.inner.lock().unwrap();
            m.values_mut().for_each(|e| e.desired = false);
            m.values().filter_map(|e| e.pid).collect()
        };
        for &pid in &pids {
            signal_group(pid, false);
        }
        let deadline = std::time::Instant::now() + STOP_GRACE;
        while std::time::Instant::now() < deadline && pids.iter().any(|&p| alive(p)) {
            std::thread::sleep(Duration::from_millis(100));
        }
        for &pid in pids.iter().filter(|&&p| alive(p)) {
            signal_group(pid, true);
        }
        self.inner.lock().unwrap().values_mut().for_each(|e| e.pid = None);
        self.persist_pids();
    }
}

/// Stops `devtunnel connect|host` children left behind by a previous instance that died without cleanup
/// (SIGKILL, crash). Returns the tunnel ids so they can be restarted under management.
pub fn reclaim_orphans(pidfile: &std::path::Path, role: Role) -> Vec<String> {
    let mut running: Option<Vec<crate::external::ExternalConn>> = None;
    reclaim_orphans_with(pidfile, |pid, id| {
        // Only signal PIDs that are still a devtunnel of this role for the recorded tunnel (PIDs get reused).
        running
            .get_or_insert_with(|| crate::external::scan(&[]))
            .iter()
            .any(|c| c.pid == pid && c.role == role && crate::external::matches_tunnel(&c.tunnel_arg, id))
    })
}

fn reclaim_orphans_with(pidfile: &std::path::Path, mut is_ours: impl FnMut(u32, &str) -> bool) -> Vec<String> {
    let Some(map) = std::fs::read(pidfile).ok().and_then(|b| serde_json::from_slice::<BTreeMap<String, u32>>(&b).ok())
    else {
        return vec![];
    };
    let _ = std::fs::remove_file(pidfile);
    if map.is_empty() {
        return vec![];
    }
    let mut reclaimed = Vec::new();
    for (id, pid) in map {
        if alive(pid) && is_ours(pid, &id) {
            signal_group(pid, false);
            let deadline = std::time::Instant::now() + STOP_GRACE;
            while std::time::Instant::now() < deadline && alive(pid) {
                std::thread::sleep(Duration::from_millis(100));
            }
            if alive(pid) {
                signal_group(pid, true);
            }
            reclaimed.push(id);
        }
    }
    reclaimed
}

async fn terminate(child: &mut Child, pid: Option<u32>) {
    if let Some(pid) = pid {
        signal_group(pid, false);
        if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_ok() {
            return;
        }
        signal_group(pid, true);
    }
    let _ = child.kill().await;
}

/// Signals the child's process group (the CLI may be a wrapper that spawns the real binary).
#[cfg(unix)]
pub fn signal_group(pid: u32, force: bool) {
    let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
    unsafe {
        if libc::kill(-(pid as i32), sig) != 0 {
            libc::kill(pid as i32, sig);
        }
    }
}

/// Windows has no SIGTERM for console-less children: terminate the child's Job Object (whole tree),
/// or walk the process tree for PIDs we don't own a job for.
#[cfg(windows)]
pub fn signal_group(pid: u32, _force: bool) {
    if !crate::win::terminate_job(pid) {
        crate::win::kill_tree(pid);
    }
}

#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    crate::win::alive(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_secs(1), 1);
        assert_eq!(backoff_secs(2), 2);
        assert_eq!(backoff_secs(4), 8);
        assert_eq!(backoff_secs(7), 60);
        assert_eq!(backoff_secs(100), 60);
    }

    /// Writes a fake `devtunnel` batch file that runs `body` (cmd syntax).
    #[cfg(windows)]
    fn fake_cli(dir: &std::path::Path, body: &str) -> PathBuf {
        let p = dir.join("devtunnel.cmd");
        std::fs::write(&p, format!("@echo off\r\n{}\r\n", body.replace('\n', "\r\n"))).unwrap();
        p
    }

    /// Picks the fake CLI body for the current platform.
    fn script(unix: &str, windows: &str) -> String {
        if cfg!(windows) { windows.into() } else { unix.into() }
    }

    fn fixture(name: &str) -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(name).display().to_string()
    }

    /// Windows sleep for batch files (`timeout` needs a console).
    const WIN_SLEEP: &str = "ping -n 31 127.0.0.1 >nul";

    /// Writes a fake `devtunnel` script that runs `body` for `connect`.
    #[cfg(unix)]
    fn fake_cli(dir: &std::path::Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("devtunnel");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn sup(dir: &std::path::Path) -> (Supervisor, Arc<Mutex<Vec<ConnState>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let listener: Listener = Arc::new(move |ev| {
            if let SupEvent::Changed { next, .. } = ev {
                s2.lock().unwrap().push(next);
            }
        });
        (Supervisor::new(Arc::new(LogStore::new(dir.join("logs"))), listener), seen)
    }

    fn wait_for(s: &Supervisor, id: &str, f: impl Fn(&ConnState) -> bool) -> ConnState {
        for _ in 0..100 {
            let st = s.state(id).0;
            if f(&st) {
                return st;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out; state = {:?}", s.state(id).0);
    }

    #[test]
    fn connects_forwards_and_disconnects_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let f = fixture("connect_ok.txt");
        let cli = fake_cli(dir.path(), &script(&format!("cat '{f}'\nexec sleep 30"), &format!("type \"{f}\"\n{WIN_SLEEP}")));
        let (s, _) = sup(dir.path());
        s.connect("t.usw3", cli);
        wait_for(&s, "t.usw3", |st| *st == ConnState::Connected);
        for _ in 0..40 {
            if !s.state("t.usw3").1.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(s.state("t.usw3").1, vec![Forward { local_port: 2223, remote_port: 2222 }]);
        let pid = s.managed_pids()[0];
        assert!(alive(pid));
        s.disconnect("t.usw3");
        wait_for(&s, "t.usw3", |st| *st == ConnState::Disconnected);
        std::thread::sleep(Duration::from_millis(200));
        assert!(!alive(pid), "child should be terminated");
        assert!(!s.is_active("t.usw3"));
    }

    #[test]
    fn hosts_with_host_subcommand_and_tracks_hosted_ports() {
        let dir = tempfile::tempdir().unwrap();
        let f = fixture("host_ok.txt");
        // Only succeed when invoked as `host <id>`.
        let cli = fake_cli(
            dir.path(),
            &script(
                &format!("[ \"$1\" = host ] || exit 9\ncat '{f}'\nexec sleep 30"),
                &format!("if not \"%1\"==\"host\" exit /b 9\ntype \"{f}\"\n{WIN_SLEEP}"),
            ),
        );
        let logs = Arc::new(LogStore::new(dir.path().join("logs")));
        let roles = Arc::new(Mutex::new(Vec::new()));
        let r2 = roles.clone();
        let listener: Listener = Arc::new(move |ev| {
            if let SupEvent::Changed { role, .. } = ev {
                r2.lock().unwrap().push(role);
            }
        });
        let s = Supervisor::with_role(Role::Host, logs.clone(), listener);
        s.connect("t.usw3", cli);
        wait_for(&s, "t.usw3", |st| *st == ConnState::Connected);
        for _ in 0..40 {
            if !s.state("t.usw3").1.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(s.state("t.usw3").1, vec![Forward { local_port: 18765, remote_port: 18765 }]);
        assert!(roles.lock().unwrap().iter().all(|r| *r == Role::Host));
        assert!(logs.get("t.usw3").iter().any(|l| l.stream == "host-out" && l.text.starts_with("Ready")));
        s.disconnect("t.usw3");
        wait_for(&s, "t.usw3", |st| *st == ConnState::Disconnected);
    }

    #[test]
    fn crashing_child_backs_off_then_stops_on_disconnect() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(dir.path(), &script("echo 'Error: host not available'\nexit 3", "echo Error: host not available\nexit /b 3"));
        let (s, _) = sup(dir.path());
        s.connect("t.usw3", cli);
        let st = wait_for(&s, "t.usw3", |st| matches!(st, ConnState::Backoff { attempt: 2.., .. }));
        if let ConnState::Backoff { message, .. } = st {
            assert!(message.contains("host not available") && message.contains("exit code 3"), "{message}");
        }
        s.disconnect("t.usw3");
        wait_for(&s, "t.usw3", |st| *st == ConnState::Disconnected);
    }

    #[test]
    fn auth_failure_stops_retrying() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(
            dir.path(),
            &script(
                "echo 'Login required. Run devtunnel user login.' >&2\nexec sleep 30",
                &format!("echo Login required. Run devtunnel user login. 1>&2\n{WIN_SLEEP}"),
            ),
        );
        let (s, _) = sup(dir.path());
        s.connect("t.usw3", cli);
        wait_for(&s, "t.usw3", |st| matches!(st, ConnState::AuthRequired { .. }));
        std::thread::sleep(Duration::from_millis(300));
        assert!(!s.is_active("t.usw3"));
        assert!(s.managed_pids().is_empty());
    }

    /// Live: serve "tunnelbar-live-ok" on the tunnel port, then `TUNNELBAR_LIVE_HOST=<id> cargo test live_host -- --ignored`.
    #[cfg(unix)]
    #[test]
    #[ignore]
    fn live_host_and_connect_through_it() {
        // TUNNELBAR_LIVE_HOST=<tunnel with an http port served by a local server on that port>
        let id = std::env::var("TUNNELBAR_LIVE_HOST").expect("set TUNNELBAR_LIVE_HOST");
        let cli = tauri::async_runtime::block_on(crate::cli::find_cli(None)).expect("devtunnel CLI");
        let rt = tokio::runtime::Runtime::new().unwrap();
        // Anonymous access round-trip while nothing is hosted.
        rt.block_on(crate::cli::set_anonymous(&cli, &id, true)).unwrap();
        let d = rt.block_on(crate::cli::show(&cli, &id)).unwrap();
        assert!(d.access_control.iter().any(|a| a.allows_anonymous()), "anonymous added: {:?}", d.access_control);
        rt.block_on(crate::cli::set_anonymous(&cli, &id, false)).unwrap();
        let d = rt.block_on(crate::cli::show(&cli, &id)).unwrap();
        assert!(!d.access_control.iter().any(|a| a.allows_anonymous()), "anonymous removed");

        let dir = tempfile::tempdir().unwrap();
        let logs = Arc::new(LogStore::new(dir.path().join("logs")));
        let host = Supervisor::with_role(Role::Host, logs.clone(), Arc::new(|_| {}));
        host.connect(&id, cli.clone());
        for _ in 0..600 {
            if host.state(&id).0 == ConnState::Connected && !host.state(&id).1.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let (st, hosted) = host.state(&id);
        eprintln!("host state={st:?} hosted={hosted:?}");
        assert_eq!(st, ConnState::Connected);
        let port = hosted[0].remote_port;
        std::thread::sleep(Duration::from_secs(2));

        let client = Supervisor::new(logs.clone(), Arc::new(|_| {}));
        client.connect(&id, cli);
        for _ in 0..600 {
            if !client.state(&id).1.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let local = client.state(&id).1[0].local_port;
        assert_ne!(local, port, "client must pick another local port");
        let body = {
            use std::io::{Read, Write};
            let mut c = std::net::TcpStream::connect(("127.0.0.1", local)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
            c.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n").unwrap();
            let mut s = String::new();
            let _ = c.read_to_string(&mut s);
            s
        };
        eprintln!("response head={:?}", body.lines().next());
        assert!(body.starts_with("HTTP/1."), "HTTP through hosted tunnel");
        assert!(body.contains("tunnelbar-live-ok"), "served by the local test server");
        let pids = [host.managed_pids(), client.managed_pids()].concat();
        client.disconnect(&id);
        host.disconnect(&id);
        wait_for(&host, &id, |st| *st == ConnState::Disconnected);
        std::thread::sleep(Duration::from_millis(500));
        assert!(pids.iter().all(|&p| !alive(p)));
    }

    /// Live test against the real CLI and account: `TUNNELBAR_LIVE_TUNNEL=<id> cargo test live_ -- --ignored`.
    #[cfg(unix)]
    #[test]
    #[ignore]
    fn live_connect_disconnect() {
        let id = std::env::var("TUNNELBAR_LIVE_TUNNEL").expect("set TUNNELBAR_LIVE_TUNNEL");
        let cli = tauri::async_runtime::block_on(crate::cli::find_cli(None)).expect("devtunnel CLI");
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = sup(dir.path());
        s.connect(&id, cli);
        for _ in 0..600 {
            if s.state(&id).0 == ConnState::Connected && !s.state(&id).1.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let (st, fwd) = s.state(&id);
        eprintln!("state={st:?} forwards={fwd:?}");
        assert_eq!(st, ConnState::Connected);
        let port = fwd[0].local_port;
        {
            use std::io::Read;
            let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).expect("local listener");
            c.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
            let mut buf = [0u8; 64];
            let n = c.read(&mut buf).unwrap_or(0);
            let banner = String::from_utf8_lossy(&buf[..n]).to_string();
            eprintln!("banner={banner:?}");
            if fwd[0].remote_port == 22 || fwd[0].remote_port == 2222 {
                assert!(banner.starts_with("SSH-"), "expected SSH banner through tunnel");
            }
        }
        let pid = s.managed_pids()[0];
        s.disconnect(&id);
        wait_for(&s, &id, |st| *st == ConnState::Disconnected);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!alive(pid));
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "listener closed");
    }

    #[test]
    fn pidfile_tracks_children_and_reclaims_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(dir.path(), &script("echo 'Connected to tunnel: t1'; sleep 30", &format!("echo Connected to tunnel: t1\n{WIN_SLEEP}")));
        let pidfile = dir.path().join("children.json");
        let (s, _) = sup(dir.path());
        let s = s.with_pidfile(pidfile.clone());
        s.connect("t1", cli);
        wait_for(&s, "t1", |st| *st == ConnState::Connected);
        let map: BTreeMap<String, u32> = serde_json::from_slice(&std::fs::read(&pidfile).unwrap()).unwrap();
        let pid = map["t1"];
        assert!(alive(pid));

        // Simulate a hard crash: the supervisor never cleans up, so the child is orphaned.
        let pre = std::fs::read(&pidfile).unwrap();
        let ignored = reclaim_orphans_with(&pidfile, |_, _| false);
        assert!(ignored.is_empty() && alive(pid), "a reused/foreign pid must not be signalled");
        std::fs::write(&pidfile, &pre).unwrap();
        let got = reclaim_orphans_with(&pidfile, |p, id| p == pid && id == "t1");
        assert_eq!(got, vec!["t1".to_string()]);
        for _ in 0..50 {
            if !alive(pid) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!alive(pid));
        assert!(!pidfile.exists());
        s.shutdown();
    }

    #[test]
    fn shutdown_clears_pidfile() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(dir.path(), &script("echo 'Connected to tunnel: t1'; sleep 30", &format!("echo Connected to tunnel: t1\n{WIN_SLEEP}")));
        let pidfile = dir.path().join("children.json");
        let (s, _) = sup(dir.path());
        let s = s.with_pidfile(pidfile.clone());
        s.connect("t1", cli);
        wait_for(&s, "t1", |st| *st == ConnState::Connected);
        s.shutdown();
        let map: BTreeMap<String, u32> = serde_json::from_slice(&std::fs::read(&pidfile).unwrap()).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn shutdown_kills_children() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(dir.path(), &script("echo 'Connected to tunnel: x'\nexec sleep 30", &format!("echo Connected to tunnel: x\n{WIN_SLEEP}")));
        let (s, _) = sup(dir.path());
        s.connect("a.usw3", cli.clone());
        s.connect("b.usw3", cli);
        wait_for(&s, "a.usw3", |st| *st == ConnState::Connected);
        wait_for(&s, "b.usw3", |st| *st == ConnState::Connected);
        let pids = s.managed_pids();
        assert_eq!(pids.len(), 2);
        s.shutdown();
        assert!(pids.iter().all(|&p| !alive(p)));
    }

    /// The Job Object must take down grandchildren (cmd.exe -> ping.exe) when the child is stopped.
    #[cfg(windows)]
    #[test]
    fn job_object_kills_grandchildren() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(dir.path(), &format!("echo Connected to tunnel: x\n{WIN_SLEEP}"));
        let (s, _) = sup(dir.path());
        s.connect("g.usw3", cli);
        wait_for(&s, "g.usw3", |st| *st == ConnState::Connected);
        let pid = s.managed_pids()[0];
        let kids = || {
            let mut sys = sysinfo::System::new().unwrap();
            sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
            let parents: Vec<(u32, Option<u32>)> =
                sys.processes().iter().map(|(p, pr)| (p.as_u32(), pr.parent().map(|x| x.as_u32()))).collect();
            crate::external::descendants(pid, &parents).into_iter().skip(1).collect::<Vec<_>>()
        };
        let mut grandkids = vec![];
        for _ in 0..40 {
            grandkids = kids();
            if !grandkids.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!grandkids.is_empty(), "fake CLI should have spawned ping");
        s.disconnect("g.usw3");
        wait_for(&s, "g.usw3", |st| *st == ConnState::Disconnected);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!alive(pid));
        assert!(grandkids.iter().all(|&p| !alive(p)), "grandchildren must die with the job");
    }
}
