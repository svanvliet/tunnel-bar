//! Application state, snapshot assembly, refresh and background ticks.

use crate::external::{self, ExternalConn};
use crate::icon::TrayState;
use crate::logs::LogStore;
use crate::model::*;
use crate::supervisor::{SupEvent, Supervisor};
use crate::{cli, settings};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

#[derive(Default)]
pub struct Cache {
    pub cli: Option<PathBuf>,
    pub cli_version: Option<String>,
    pub auth: Option<AuthStatus>,
    pub summaries: Vec<TunnelSummary>,
    pub details: HashMap<String, TunnelDetails>,
    pub last_refresh: Option<String>,
    pub refresh_error: Option<String>,
    pub refreshing: bool,
    /// Hosted ports with something listening on localhost.
    pub listening: HashSet<u16>,
    /// Tunnels deleted recently; hidden while `devtunnel list` still reports them.
    pub deleted: HashMap<String, std::time::Instant>,
}

pub struct AppState {
    pub app: AppHandle,
    pub supervisor: Supervisor,
    /// Runs `devtunnel host` for tunnels this Mac serves.
    pub hoster: Supervisor,
    pub logs: Arc<LogStore>,
    pub settings: Mutex<Settings>,
    pub cache: Mutex<Cache>,
    pub externals: Mutex<Vec<(ExternalConn, Vec<u16>)>>,
    /// Tunnels that dropped and haven't been restored yet (for "restored" notifications).
    pub dropped: Mutex<HashSet<String>>,
    refresh_lock: tokio::sync::Mutex<()>,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(app: AppHandle) -> Shared {
        let logs = Arc::new(LogStore::new(settings::log_dir()));
        Arc::new_cyclic(|weak: &std::sync::Weak<AppState>| {
            let weak = weak.clone();
            let listener: crate::supervisor::Listener = Arc::new(move |ev| {
                if let Some(s) = weak.upgrade() {
                    s.on_supervisor_event(ev);
                }
            });
            AppState {
                app: app.clone(),
                supervisor: Supervisor::new(logs.clone(), listener.clone()).with_pidfile(settings::pidfile_path()),
                hoster: Supervisor::with_role(Role::Host, logs.clone(), listener)
                    .with_pidfile(settings::hosts_pidfile_path()),
                logs,
                settings: Mutex::new(settings::load()),
                cache: Mutex::new(Cache::default()),
                externals: Mutex::new(Vec::new()),
                dropped: Mutex::new(HashSet::new()),
                refresh_lock: tokio::sync::Mutex::new(()),
            }
        })
    }

    fn display_name(&self, id: &str, summary: Option<&TunnelSummary>) -> String {
        let s = self.settings.lock().unwrap();
        if let Some(n) = s.tunnels.get(id).and_then(|t| t.nickname.clone()).filter(|n| !n.trim().is_empty()) {
            return n;
        }
        if let Some(n) = summary.map(|s| s.name.clone()).filter(|n| !n.is_empty()) {
            return n;
        }
        id.split('.').next().unwrap_or(id).to_string()
    }

    pub fn snapshot(&self) -> Snapshot {
        let cache = self.cache.lock().unwrap();
        let externals = self.externals.lock().unwrap().clone();
        let settings = self.settings.lock().unwrap().clone();
        let mut ids: Vec<String> = cache.summaries.iter().map(|s| s.tunnel_id.clone()).collect();
        for id in self.supervisor.managed_ids().into_iter().chain(self.hoster.managed_ids()) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        let tunnels = ids
            .iter()
            .map(|id| {
                let summary = cache.summaries.iter().find(|s| &s.tunnel_id == id).cloned();
                let details = cache.details.get(id);
                let ports = details.map(|d| d.ports.clone()).unwrap_or_default();
                let (mut state, mut forwards) = self.supervisor.state(id);
                if !self.supervisor.is_active(id) && matches!(state, ConnState::Disconnected)
                    && let Some((ext, lports)) = externals.iter().find(|(e, _)| e.role == Role::Connect && external::matches_tunnel(&e.tunnel_arg, id)) {
                        state = ConnState::External { pid: ext.pid };
                        let remote: Vec<u16> = ports.iter().map(|p| p.port_number).collect();
                        forwards = external::guess_forwards(lports, &remote);
                    }
                let (mut host_state, host_fwd) = self.hoster.state(id);
                let hosting_here = self.hoster.is_active(id);
                if !hosting_here
                    && matches!(host_state, ConnState::Disconnected)
                    && let Some((ext, _)) =
                        externals.iter().find(|(e, _)| e.role == Role::Host && external::matches_tunnel(&e.tunnel_arg, id))
                {
                    host_state = ConnState::External { pid: ext.pid };
                }
                let hosted_ports = if hosting_here || matches!(host_state, ConnState::External { .. }) {
                    hosted_ports(&ports, &host_fwd, &cache.listening)
                } else {
                    vec![]
                };
                let hosted_elsewhere = !hosting_here
                    && !matches!(host_state, ConnState::External { .. })
                    && summary.as_ref().is_some_and(|s| s.host_connections > 0);
                TunnelView {
                    host_state,
                    hosted_ports,
                    hosted_elsewhere,
                    anonymous: details.is_some_and(|d| d.access_control.iter().any(|a| a.allows_anonymous())),
                    created_by_app: summary.as_ref().is_some_and(|s| s.labels.iter().any(|l| l == APP_LABEL)),
                    id: id.clone(),
                    display_name: self.display_name(id, summary.as_ref()),
                    orphaned: summary.is_none(),
                    summary: summary.unwrap_or_else(|| TunnelSummary { tunnel_id: id.clone(), ..Default::default() }),
                    ports,
                    state,
                    forwards,
                    settings: settings.tunnels.get(id).cloned().unwrap_or_default(),
                }
            })
            .collect();
        Snapshot {
            cli_path: cache.cli.as_ref().map(|p| p.display().to_string()),
            cli_version: cache.cli_version.clone(),
            auth: cache.auth.clone(),
            tunnels,
            settings,
            last_refresh: cache.last_refresh.clone(),
            refresh_error: cache.refresh_error.clone(),
            refreshing: cache.refreshing,
            log_dir: self.logs.dir().display().to_string(),
        }
    }

    pub fn publish(&self) {
        let snap = self.snapshot();
        #[cfg(debug_assertions)]
        if std::env::var_os("TUNNELBAR_TRACE").is_some() {
            let ts: Vec<_> = snap.tunnels.iter().map(|t| format!("{}={:?} {:?}", t.id, t.state, t.forwards)).collect();
            eprintln!("[snapshot] auth={:?} refreshing={} err={:?} {}", snap.auth.as_ref().map(|a| a.logged_in), snap.refreshing, snap.refresh_error, ts.join(" | "));
        }
        crate::tray::update(&self.app, &snap);
        let _ = self.app.emit("snapshot", &snap);
    }

    fn notify(&self, title: &str, body: &str) {
        if !self.settings.lock().unwrap().notifications {
            return;
        }
        let _ = self.app.notification().builder().title(title).body(body).show();
    }

    fn on_supervisor_event(self: &Arc<Self>, ev: SupEvent) {
        match ev {
            SupEvent::Log { id, line } => {
                let _ = self.app.emit("log", serde_json::json!({ "id": id, "line": line }));
            }
            SupEvent::Changed { role: Role::Connect, id, prev, next } => {
                self.handle_transition(&id, &prev, &next);
                self.publish();
            }
            SupEvent::Changed { role: Role::Host, id, prev, next } => {
                self.handle_host_transition(&id, &prev, &next);
                self.publish();
            }
        }
    }

    fn handle_transition(self: &Arc<Self>, id: &str, prev: &ConnState, next: &ConnState) {
        let name = self.display_name(id, None);
        match next {
            ConnState::Connected if self.dropped.lock().unwrap().remove(id) => {
                self.notify("Tunnel restored", &format!("{name} is connected again."));
            }
            ConnState::Backoff { retry_in_secs, .. } if !self.dropped.lock().unwrap().contains(id) => {
                if matches!(prev, ConnState::Connected | ConnState::Reconnecting) {
                    self.dropped.lock().unwrap().insert(id.to_string());
                    self.notify("Tunnel disconnected", &format!("{name} dropped. Retrying in {retry_in_secs}s."));
                } else if matches!(prev, ConnState::Connecting) {
                    self.dropped.lock().unwrap().insert(id.to_string());
                    self.notify("Tunnel connection failed", &format!("{name} could not connect. Retrying."));
                }
            }
            ConnState::Reconnecting => {
                // The CLI often recovers from blips quickly; only notify if it persists.
                let this = self.clone();
                let id = id.to_string();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    if matches!(this.supervisor.state(&id).0, ConnState::Reconnecting)
                        && this.dropped.lock().unwrap().insert(id.clone())
                    {
                        let name = this.display_name(&id, None);
                        this.notify("Tunnel interrupted", &format!("{name} lost its relay connection. Reconnecting…"));
                    }
                });
            }
            ConnState::AuthRequired { .. } => {
                self.notify("Dev Tunnels sign-in required", "Run `devtunnel user login`, then reconnect.");
                self.cache.lock().unwrap().auth = Some(AuthStatus { logged_in: false, message: "Login required".into() });
            }
            ConnState::Disconnected => {
                self.dropped.lock().unwrap().remove(id);
            }
            _ => {}
        }
    }

    fn handle_host_transition(self: &Arc<Self>, id: &str, prev: &ConnState, next: &ConnState) {
        let key = format!("host:{id}");
        let name = self.display_name(id, None);
        match next {
            ConnState::Connected => {
                if self.dropped.lock().unwrap().remove(&key) {
                    self.notify("Hosting restored", &format!("{name} is reachable again."));
                }
                if !matches!(prev, ConnState::Connected | ConnState::Reconnecting) {
                    // Pick up public port URIs and the relay's host count.
                    let this = self.clone();
                    tauri::async_runtime::spawn(async move { this.refresh().await });
                }
            }
            ConnState::Backoff { retry_in_secs, .. }
                if matches!(prev, ConnState::Connected | ConnState::Reconnecting | ConnState::Connecting)
                    && self.dropped.lock().unwrap().insert(key.clone()) =>
            {
                self.notify("Hosting interrupted", &format!("{name} stopped hosting. Retrying in {retry_in_secs}s."));
            }
            ConnState::AuthRequired { .. } => {
                self.notify("Dev Tunnels sign-in required", "Run `devtunnel user login`, then host again.");
                self.cache.lock().unwrap().auth = Some(AuthStatus { logged_in: false, message: "Login required".into() });
            }
            ConnState::Disconnected => {
                self.dropped.lock().unwrap().remove(&key);
            }
            _ => {}
        }
    }

    pub async fn cli_path(&self) -> Option<PathBuf> {
        let override_path = self.settings.lock().unwrap().cli_path.clone();
        let found = cli::find_cli(override_path.as_deref()).await;
        self.cache.lock().unwrap().cli = found.clone();
        found
    }

    /// Re-reads auth + tunnels from the CLI.
    pub async fn refresh(self: &Arc<Self>) {
        let Ok(_guard) = self.refresh_lock.try_lock() else { return };
        self.cache.lock().unwrap().refreshing = true;
        self.publish();
        let result = self.refresh_inner().await;
        {
            let mut c = self.cache.lock().unwrap();
            c.refreshing = false;
            c.refresh_error = result.err();
            c.last_refresh = Some(chrono::Local::now().to_rfc3339());
        }
        self.scan_externals();
        self.publish();
    }

    async fn refresh_inner(self: &Arc<Self>) -> Result<(), String> {
        let Some(cli_path) = self.cli_path().await else {
            return Err("devtunnel CLI not found. Install it (brew install --cask devtunnel) or set its path in Settings.".into());
        };
        if self.cache.lock().unwrap().cli_version.is_none() {
            let v = cli::version(&cli_path).await;
            self.cache.lock().unwrap().cli_version = v;
        }
        let auth = cli::auth_status(&cli_path).await?;
        let logged_in = auth.logged_in;
        self.cache.lock().unwrap().auth = Some(auth);
        if !logged_in {
            return Ok(());
        }
        let mut summaries = cli::list(&cli_path).await?;
        {
            let mut c = self.cache.lock().unwrap();
            c.deleted.retain(|_, at| at.elapsed() < Duration::from_secs(120));
            summaries.retain(|s| !c.deleted.contains_key(&s.tunnel_id));
        }
        let mut set = tokio::task::JoinSet::new();
        for s in &summaries {
            let (p, id) = (cli_path.clone(), s.tunnel_id.clone());
            set.spawn(async move { (id.clone(), cli::show(&p, &id).await) });
        }
        let mut details = HashMap::new();
        while let Some(Ok((id, r))) = set.join_next().await {
            match r {
                Ok(d) => {
                    details.insert(id, d);
                }
                Err(e) => {
                    self.logs.push(&id, "app", &format!("show failed: {e}"));
                }
            }
        }
        let mut c = self.cache.lock().unwrap();
        // Keep previous details for tunnels whose `show` failed transiently.
        for s in &summaries {
            if let (false, Some(old)) = (details.contains_key(&s.tunnel_id), c.details.get(&s.tunnel_id)) {
                details.insert(s.tunnel_id.clone(), old.clone());
            }
        }
        c.summaries = summaries;
        c.details = details;
        Ok(())
    }

    /// Returns true if the set of external connections changed.
    pub fn scan_externals(&self) -> bool {
        let mut ours = self.supervisor.managed_pids();
        ours.extend(self.hoster.managed_pids());
        let found = external::scan(&ours);
        let with_ports: Vec<(ExternalConn, Vec<u16>)> =
            found.into_iter().map(|e| { let p = external::listen_ports(e.pid); (e, p) }).collect();
        let listening = self.probe_hosted_ports();
        let listening_changed = {
            let mut c = self.cache.lock().unwrap();
            std::mem::replace(&mut c.listening, listening) != c.listening
        };
        let mut cur = self.externals.lock().unwrap();
        if *cur != with_ports {
            *cur = with_ports;
            true
        } else {
            listening_changed
        }
    }

    /// Checks which hosted ports have a local server, so the UI can warn about "nothing listening".
    fn probe_hosted_ports(&self) -> HashSet<u16> {
        let ports: HashSet<u16> = {
            let c = self.cache.lock().unwrap();
            self.hoster
                .managed_ids()
                .iter()
                .flat_map(|id| {
                    let mut p: Vec<u16> = self.hoster.state(id).1.iter().map(|f| f.remote_port).collect();
                    p.extend(c.details.get(id).map(|d| d.ports.iter().map(|x| x.port_number).collect::<Vec<_>>()).unwrap_or_default());
                    p
                })
                .collect()
        };
        ports.into_iter().filter(|&p| port_listening(p)).collect()
    }

    pub fn tray_state(snap: &Snapshot) -> TrayState {
        let any = |f: fn(&ConnState) -> bool| snap.tunnels.iter().any(|t| f(&t.state) || f(&t.host_state));
        if any(ConnState::is_problem) || snap.auth.as_ref().is_some_and(|a| !a.logged_in) {
            TrayState::Problem
        } else if any(ConnState::is_transitional) {
            TrayState::Busy
        } else if any(ConnState::is_up) {
            TrayState::Connected
        } else {
            TrayState::Idle
        }
    }

    pub async fn connect(self: &Arc<Self>, id: &str) -> Result<(), String> {
        if let Some(pid) = self.external_pid(id) {
            return Err(format!("Already connected outside Tunnel Bar (PID {pid}). Use “Take over” to manage it."));
        }
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        self.supervisor.connect(id, cli);
        Ok(())
    }

    pub fn external_pid(&self, id: &str) -> Option<u32> {
        self.externals
            .lock()
            .unwrap()
            .iter()
            .find(|(e, _)| e.role == Role::Connect && external::matches_tunnel(&e.tunnel_arg, id))
            .map(|(e, _)| e.pid)
    }

    pub fn external_host_pid(&self, id: &str) -> Option<u32> {
        self.externals
            .lock()
            .unwrap()
            .iter()
            .find(|(e, _)| e.role == Role::Host && external::matches_tunnel(&e.tunnel_arg, id))
            .map(|(e, _)| e.pid)
    }

    fn hosted_elsewhere(&self, id: &str) -> bool {
        !self.hoster.is_active(id)
            && self.cache.lock().unwrap().summaries.iter().any(|s| s.tunnel_id == id && s.host_connections > 0)
    }

    pub async fn host(self: &Arc<Self>, id: &str) -> Result<(), String> {
        if let Some(pid) = self.external_host_pid(id) {
            return Err(format!("Already hosted outside Tunnel Bar (PID {pid})."));
        }
        if self.hosted_elsewhere(id) {
            return Err("This tunnel is already hosted by another machine. Hosting it here too would split its traffic.".into());
        }
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        self.hoster.connect(id, cli);
        Ok(())
    }

    pub fn stop_host(self: &Arc<Self>, id: &str) {
        self.hoster.disconnect(id);
        // The relay's host count lags; refresh so the card doesn't claim "hosted elsewhere".
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            this.refresh().await;
        });
    }

    /// Creates a Tunnel Bar-labelled tunnel for localhost:port and starts hosting it.
    pub async fn expose(self: &Arc<Self>, port: u16, protocol: &str, anonymous: bool) -> Result<String, String> {
        if port == 0 {
            return Err("Enter a port between 1 and 65535.".into());
        }
        if !matches!(protocol, "http" | "https" | "auto") {
            return Err(format!("Unsupported protocol {protocol}"));
        }
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        let id = cli::create(&cli, &[APP_LABEL], &format!("localhost:{port} exposed by Tunnel Bar")).await?;
        self.logs.push(&id, "app", &format!("Created tunnel for localhost:{port} ({protocol})"));
        let setup = async {
            cli::port_create(&cli, &id, port, protocol).await?;
            if anonymous {
                cli::set_anonymous(&cli, &id, true).await?;
            }
            Ok::<(), String>(())
        };
        if let Err(e) = setup.await {
            let _ = cli::delete(&cli, &id).await;
            return Err(e);
        }
        {
            let mut s = self.settings.lock().unwrap();
            let t = s.tunnels.entry(id.clone()).or_default();
            t.nickname = Some(format!("localhost:{port}"));
        }
        self.save_settings()?;
        self.refresh().await;
        self.hoster.connect(&id, cli);
        // The service can lag on new tunnels/access rules; re-read once it settles.
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(4)).await;
            this.refresh().await;
        });
        Ok(id)
    }

    pub async fn set_anonymous(self: &Arc<Self>, id: &str, allow: bool) -> Result<(), String> {
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        cli::set_anonymous(&cli, id, allow).await?;
        self.logs.push(
            id,
            "app",
            if allow { "Anonymous access enabled — anyone with the URL can connect" } else { "Anonymous access removed" },
        );
        // `show` is eventually consistent, so reflect the change immediately and re-read later.
        if let Some(d) = self.cache.lock().unwrap().details.get_mut(id) {
            d.access_control.retain(|a| !a.allows_anonymous());
            if allow {
                d.access_control.push(AccessEntry {
                    kind: "Anonymous".into(),
                    scopes: vec!["connect".into()],
                    ..Default::default()
                });
            }
        }
        self.publish();
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(4)).await;
            this.refresh().await;
        });
        Ok(())
    }

    /// Deletes a tunnel Tunnel Bar created (refuses others to avoid destroying dev box tunnels).
    pub async fn delete_tunnel(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let ours = self
            .cache
            .lock()
            .unwrap()
            .summaries
            .iter()
            .any(|s| s.tunnel_id == id && s.labels.iter().any(|l| l == APP_LABEL));
        if !ours {
            return Err("Only tunnels created by Tunnel Bar can be deleted here.".into());
        }
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        self.hoster.disconnect(id);
        self.supervisor.disconnect(id);
        for _ in 0..40 {
            if !self.hoster.is_active(id) && !self.supervisor.is_active(id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        cli::delete(&cli, id).await?;
        self.settings.lock().unwrap().tunnels.remove(id);
        self.save_settings()?;
        self.logs.push(id, "app", "Tunnel deleted");
        {
            let mut c = self.cache.lock().unwrap();
            c.deleted.insert(id.to_string(), std::time::Instant::now());
            c.summaries.retain(|s| s.tunnel_id != id);
            c.details.remove(id);
        }
        self.publish();
        self.refresh().await;
        Ok(())
    }

    pub fn shutdown(&self) {
        // Stop both roles concurrently so quitting stays within one grace period.
        let h = self.hoster.clone();
        let t = std::thread::spawn(move || h.shutdown());
        self.supervisor.shutdown();
        let _ = t.join();
    }

    /// Stops an external `devtunnel connect` (signals only that PID, not its terminal's process group).
    pub async fn stop_external(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let pid = self.external_pid(id).ok_or("No external connection found")?;
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        #[cfg(windows)]
        crate::supervisor::signal_group(pid, true);
        for _ in 0..30 {
            if !crate::supervisor::alive(pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        #[cfg(unix)]
        if crate::supervisor::alive(pid) {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        self.scan_externals();
        self.publish();
        Ok(())
    }

    pub async fn auto_connect(self: &Arc<Self>) {
        let ids: Vec<String> = {
            let tunnels = self.settings.lock().unwrap().tunnels.clone();
            let c = self.cache.lock().unwrap();
            c.summaries
                .iter()
                .filter(|t| tunnels.get(&t.tunnel_id).is_some_and(|x| x.auto_connect))
                .map(|t| t.tunnel_id.clone())
                .collect()
        };
        for id in ids {
            if !self.supervisor.is_active(&id) && self.external_pid(&id).is_none() {
                let _ = self.connect(&id).await;
            }
        }
        let host_ids: Vec<String> = {
            let tunnels = self.settings.lock().unwrap().tunnels.clone();
            let c = self.cache.lock().unwrap();
            c.summaries
                .iter()
                .filter(|t| tunnels.get(&t.tunnel_id).is_some_and(|x| x.auto_host))
                .map(|t| t.tunnel_id.clone())
                .collect()
        };
        for id in host_ids {
            if self.hoster.is_active(&id) {
                continue;
            }
            if let Err(e) = self.host(&id).await {
                self.logs.push(&id, "app", &format!("Auto-host skipped: {e}"));
            }
        }
    }

    pub fn save_settings(&self) -> Result<(), String> {
        settings::save(&self.settings.lock().unwrap())
    }
}

/// Periodic work: external scan (10s), account refresh (60s), sleep/wake detection.
pub fn start_background(state: Shared) {
    tauri::async_runtime::spawn(async move {
        let (reclaimed, reclaimed_hosts) = tokio::task::spawn_blocking(|| {
            (
                crate::supervisor::reclaim_orphans(&settings::pidfile_path(), Role::Connect),
                crate::supervisor::reclaim_orphans(&settings::hosts_pidfile_path(), Role::Host),
            )
        })
        .await
        .unwrap_or_default();
        for id in &reclaimed {
            state.logs.push(id, "app", "Reclaimed connection left running by a previous Tunnel Bar instance");
        }
        for id in &reclaimed_hosts {
            state.logs.push(id, "app", "Reclaimed host left running by a previous Tunnel Bar instance");
        }
        // Restart reclaimed hosts before refreshing so the stale host count isn't mistaken for another machine.
        if !reclaimed_hosts.is_empty()
            && let Some(cli) = state.cli_path().await
        {
            for id in &reclaimed_hosts {
                state.hoster.connect(id, cli.clone());
            }
        }
        state.refresh().await;
        for id in reclaimed {
            if !state.supervisor.is_active(&id) {
                let _ = state.connect(&id).await;
            }
        }
        state.auto_connect().await;
        let mut last_wall = chrono::Local::now();
        let mut ticks: u64 = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            ticks += 1;
            let now = chrono::Local::now();
            let gap = (now - last_wall).num_seconds();
            last_wall = now;
            if gap > 30 {
                // Woke from sleep: retry immediately instead of waiting out backoff.
                state.logs.push("_app", "app", &format!("Wake detected (gap {gap}s)"));
                state.supervisor.kick_all();
                state.hoster.kick_all();
                state.refresh().await;
                state.auto_connect().await;
                continue;
            }
            if ticks.is_multiple_of(2) && state.scan_externals() {
                state.publish();
            }
            if ticks.is_multiple_of(12) {
                state.refresh().await;
            }
        }
    });
}

pub fn state(app: &AppHandle) -> Shared {
    app.state::<Shared>().inner().clone()
}

fn hosted_ports(ports: &[TunnelPort], live: &[Forward], listening: &HashSet<u16>) -> Vec<HostedPort> {
    let mut nums: Vec<u16> = ports.iter().map(|p| p.port_number).collect();
    nums.extend(live.iter().map(|f| f.remote_port));
    nums.sort();
    nums.dedup();
    nums.into_iter()
        .map(|n| {
            let p = ports.iter().find(|p| p.port_number == n);
            let uri = p.map(|p| p.port_uri.clone()).unwrap_or_default();
            HostedPort {
                port: n,
                protocol: p.map(|p| p.protocol.clone()).unwrap_or_default(),
                inspect_uri: inspect_uri(&uri),
                uri,
                listening: listening.contains(&n),
            }
        })
        .collect()
}

fn port_listening(port: u16) -> bool {
    let timeout = Duration::from_millis(150);
    ["127.0.0.1", "[::1]"].iter().any(|h| {
        format!("{h}:{port}")
            .parse::<std::net::SocketAddr>()
            .is_ok_and(|a| std::net::TcpStream::connect_timeout(&a, timeout).is_ok())
    })
}
