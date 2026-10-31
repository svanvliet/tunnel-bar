//! Supervises one `devtunnel connect <id>` child process per tunnel.

use crate::logs::LogStore;
use crate::model::{ConnState, Forward};
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
    Changed { id: String, prev: ConnState, next: ConnState },
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
        Self { inner: Arc::new(Mutex::new(HashMap::new())), logs, listener, pidfile: None }
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
        (self.listener)(SupEvent::Changed { id: id.to_string(), prev, next });
    }

    fn log(&self, id: &str, stream: &str, text: &str) {
        let line = self.logs.push(id, stream, text);
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
        cmd.args(["connect", id]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("failed to start devtunnel: {e}");
                self.log(id, "app", &msg);
                return RunEnd::SpawnFailed(msg);
            }
        };
        let pid = child.id();
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
            CliEvent::Connected { .. } | CliEvent::Restored => {
                *was_connected = true;
                self.set_state(id, Some(generation), ConnState::Connected);
            }
            CliEvent::Forward { local_port, remote_port, .. } => {
                *was_connected = true;
                let added = {
                    let mut m = self.inner.lock().unwrap();
                    m.get_mut(id)
                        .filter(|e| e.generation == generation)
                        .map(|e| e.forwards.insert(Forward { local_port, remote_port }))
                        .unwrap_or(false)
                };
                self.set_state(id, Some(generation), ConnState::Connected);
                if added {
                    let s = self.state(id).0;
                    (self.listener)(SupEvent::Changed { id: id.to_string(), prev: s.clone(), next: s });
                }
            }
            CliEvent::Reconnecting => self.set_state(id, Some(generation), ConnState::Reconnecting),
            CliEvent::Closed { message } | CliEvent::Error { message } => *last_msg = message,
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

/// Stops `devtunnel connect` children left behind by a previous instance that died without cleanup
/// (SIGKILL, crash). Returns the tunnel ids so they can be reconnected under management.
pub fn reclaim_orphans(pidfile: &std::path::Path) -> Vec<String> {
    let mut running: Option<Vec<crate::external::ExternalConn>> = None;
    reclaim_orphans_with(pidfile, |pid, id| {
        // Only signal PIDs that are still a `devtunnel connect` for the recorded tunnel (PIDs get reused).
        running
            .get_or_insert_with(|| crate::external::scan(&[]))
            .iter()
            .any(|c| c.pid == pid && crate::external::matches_tunnel(&c.tunnel_arg, id))
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

#[cfg(windows)]
pub fn signal_group(pid: u32, _force: bool) {
    let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
}

#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    let Ok(mut s) = sysinfo::System::new() else { return false };
    s.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]), true);
    s.process(sysinfo::Pid::from_u32(pid)).is_some()
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

    /// Writes a fake `devtunnel` script that runs `body` for `connect`.
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

    #[cfg(unix)]
    #[test]
    fn connects_forwards_and_disconnects_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/connect_ok.txt");
        let cli = fake_cli(dir.path(), &format!("cat '{fixture}'\nexec sleep 30"));
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

    #[cfg(unix)]
    #[test]
    fn crashing_child_backs_off_then_stops_on_disconnect() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(dir.path(), "echo 'Error: host not available'\nexit 3");
        let (s, _) = sup(dir.path());
        s.connect("t.usw3", cli);
        let st = wait_for(&s, "t.usw3", |st| matches!(st, ConnState::Backoff { attempt: 2.., .. }));
        if let ConnState::Backoff { message, .. } = st {
            assert!(message.contains("host not available") && message.contains("exit code 3"), "{message}");
        }
        s.disconnect("t.usw3");
        wait_for(&s, "t.usw3", |st| *st == ConnState::Disconnected);
    }

    #[cfg(unix)]
    #[test]
    fn auth_failure_stops_retrying() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(dir.path(), "echo 'Login required. Run devtunnel user login.' >&2\nexec sleep 30");
        let (s, _) = sup(dir.path());
        s.connect("t.usw3", cli);
        wait_for(&s, "t.usw3", |st| matches!(st, ConnState::AuthRequired { .. }));
        std::thread::sleep(Duration::from_millis(300));
        assert!(!s.is_active("t.usw3"));
        assert!(s.managed_pids().is_empty());
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

    #[cfg(unix)]
    #[test]
    fn pidfile_tracks_children_and_reclaims_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(dir.path(), "echo 'Connected to tunnel: t1'; sleep 30");
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
        let cli = fake_cli(dir.path(), "echo 'Connected to tunnel: t1'; sleep 30");
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
        let cli = fake_cli(dir.path(), "echo 'Connected to tunnel: x'\nexec sleep 30");
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
}
