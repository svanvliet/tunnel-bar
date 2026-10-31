//! Application state, snapshot assembly, refresh and background ticks.

use crate::external::{self, ExternalConn};
use crate::icon::TrayState;
use crate::logs::LogStore;
use crate::model::*;
use crate::supervisor::{SupEvent, Supervisor};
use crate::pin::Pins;
use crate::{cli, login, settings};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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
    /// Stable-port proxies for connected tunnels.
    pub pins: Pins,
    /// Hosting port-map proxies: 127.0.0.1:<tunnel port> -> localhost:<local port> while this Mac hosts.
    pub host_maps: Pins,
    host_map_targets: Arc<Mutex<HashMap<(String, u16), u16>>>,
    pub login: Mutex<Option<LoginState>>,
    login_cancel: Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// PID (= process group) of the running `devtunnel user login`, so quitting can stop it synchronously.
    login_pid: Arc<Mutex<Option<u32>>>,
    pub hotkey_error: Mutex<Option<String>>,
    /// A devtunnel process we can't inspect (elevated) couldn't be matched to a tunnel.
    pub admin_devtunnel: Mutex<bool>,
    /// When a host on this device was last seen per tunnel (feeds `Settings.hosted_here`).
    local_host_seen: Mutex<HashMap<String, Instant>>,
    started: Instant,
}

/// Why a delete must be refused, given what the caller acknowledged.
pub fn check_delete(p: &DeletePreflight, confirm: bool, allow_hosted_elsewhere: bool) -> Result<(), String> {
    if p.hosted_elsewhere && !allow_hosted_elsewhere {
        return Err(format!(
            "{HOSTED_ELSEWHERE}This tunnel is hosted by another machine (e.g. a dev box); deleting it would cut that machine off."
        ));
    }
    if p.needs_confirm && !confirm {
        return Err(format!("{CONFIRM_REQUIRED}This tunnel wasn't created by Tunnel Bar. Confirm to delete it permanently."));
    }
    Ok(())
}

/// More hosts than the one this Mac runs means another machine hosts the tunnel too.
pub fn hosted_elsewhere_count(host_connections: u32, hosting_here: bool) -> bool {
    host_connections > hosting_here as u32
}

/// Label changes that would forge or drop Tunnel Bar ownership on a tunnel it didn't create are refused
/// outright (ownership gates every confirm guard); expiration changes there need `confirm`.
pub fn check_update(req: &UpdateTunnelRequest, created_by_app: bool, confirm: bool) -> Result<(), String> {
    if created_by_app {
        return Ok(());
    }
    if req.add_labels.iter().chain(&req.remove_labels).any(|l| l.eq_ignore_ascii_case(APP_LABEL)) {
        return Err(format!("The “{APP_LABEL}” label can't be changed on a tunnel Tunnel Bar didn't create."));
    }
    if req.expiration.is_some() && !confirm {
        return Err(format!(
            "{CONFIRM_REQUIRED}This tunnel wasn't created by Tunnel Bar. Confirm to change its expiration; the service deletes it when it expires."
        ));
    }
    Ok(())
}

/// Error prefixes the UI can match to offer a confirmation and retry with the matching flag.
pub const CONFIRM_REQUIRED: &str = "CONFIRM_REQUIRED: ";
pub const HOSTED_ELSEWHERE: &str = "HOSTED_ELSEWHERE: ";

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
            let supervisor = Supervisor::new(logs.clone(), listener.clone()).with_pidfile(settings::pidfile_path());
            let sup2 = supervisor.clone();
            let resolve: crate::pin::Resolver = Arc::new(move |id, remote| {
                sup2.state(id).1.iter().find(|f| f.remote_port == remote).map(|f| f.local_port)
            });
            let host_map_targets: Arc<Mutex<HashMap<(String, u16), u16>>> = Default::default();
            let targets = host_map_targets.clone();
            let map_resolve: crate::pin::Resolver =
                Arc::new(move |id, remote| targets.lock().unwrap().get(&(id.to_string(), remote)).copied());
            AppState {
                app: app.clone(),
                pins: Pins::new(resolve),
                host_maps: Pins::new(map_resolve),
                host_map_targets,
                login: Mutex::new(None),
                login_cancel: Mutex::new(None),
                login_pid: Default::default(),
                hotkey_error: Mutex::new(None),
                admin_devtunnel: Mutex::new(false),
                local_host_seen: Mutex::new(HashMap::new()),
                started: Instant::now(),
                supervisor,
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
                let host_map = settings.tunnels.get(id).map(|t| t.host_port_map.clone()).unwrap_or_default();
                let mut ports = details.map(|d| d.ports.clone()).unwrap_or_default();
                for p in &mut ports {
                    p.local_port = host_map.get(&p.port_number).copied();
                }
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
                    let map = if hosting_here { host_map.clone() } else { Default::default() };
                    hosted_ports(&ports, &host_fwd, &cache.listening, &map)
                } else {
                    vec![]
                };
                let hosted_elsewhere = !hosting_here
                    && !matches!(host_state, ConnState::External { .. })
                    && summary.as_ref().is_some_and(|s| s.host_connections > 0);
                let hosted_here = if hosting_here || matches!(host_state, ConnState::External { .. }) {
                    HostedHere::Live
                } else if settings.hosted_here.contains(id) {
                    HostedHere::Last
                } else {
                    HostedHere::No
                };
                let tunnel_settings = settings.tunnels.get(id).cloned().unwrap_or_default();
                let expiration = details
                    .map(|d| d.tunnel_expiration.as_str())
                    .filter(|e| !e.is_empty())
                    .or(summary.as_ref().map(|s| s.tunnel_expiration.as_str()));
                TunnelView {
                    expiration_hours: expiration.and_then(parse_expiration_hours),
                    matches_filter: matches_label_filter(
                        summary.as_ref().map(|s| s.labels.as_slice()).unwrap_or_default(),
                        &settings.label_filter,
                    ),
                    pins: tunnel_settings.pinned_ports.iter().map(|(&r, &l)| self.pins.view(id, r, l)).collect(),
                    port_maps: host_map
                        .iter()
                        .map(|(&r, &l)| {
                            let v = self.host_maps.view(id, r, r);
                            PortMapView { remote_port: r, local_port: l, listening: v.listening, error: v.error }
                        })
                        .collect(),
                    access_rules: details.map(|d| d.access_control.clone()).unwrap_or_default(),
                    host_state,
                    hosted_here,
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
                    settings: tunnel_settings,
                }
            })
            .collect();
        let mut all_labels: Vec<String> = cache.summaries.iter().flat_map(|s| s.labels.iter().cloned()).collect();
        all_labels.sort_by_key(|l| l.to_lowercase());
        all_labels.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        Snapshot {
            all_labels,
            login: self.login.lock().unwrap().clone(),
            hotkey_error: self.hotkey_error.lock().unwrap().clone(),
            admin_devtunnel: *self.admin_devtunnel.lock().unwrap(),
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
                self.sync_pins(None);
                self.handle_transition(&id, &prev, &next);
                self.publish();
            }
            SupEvent::Changed { role: Role::Host, id, prev, next } => {
                self.sync_host_maps(None);
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
            let install = if cfg!(windows) { "winget install Microsoft.devtunnel" } else { "brew install --cask devtunnel" };
            return Err(format!("devtunnel CLI not found. Install it ({install}) or set its path in Settings."));
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
        let scan = external::scan_all(&ours);
        let mut found = scan.found;
        let mut unexplained = false;
        if !scan.opaque.is_empty() {
            let opaque: Vec<(u32, bool)> = scan.opaque.iter().map(|&p| (p, !external::listen_ports(p).is_empty())).collect();
            let candidates: Vec<String> = self
                .cache
                .lock()
                .unwrap()
                .summaries
                .iter()
                .filter(|s| s.host_connections > 0 && !self.hoster.is_active(&s.tunnel_id))
                .filter(|s| !found.iter().any(|e| e.role == Role::Host && external::matches_tunnel(&e.tunnel_arg, &s.tunnel_id)))
                .map(|s| s.tunnel_id.clone())
                .collect();
            let a = external::attribute_opaque(&opaque, &candidates);
            found.extend(a.host);
            unexplained = a.unexplained;
        }
        external::exclusive_roles(&mut found);
        let notice_changed = std::mem::replace(&mut *self.admin_devtunnel.lock().unwrap(), unexplained) != unexplained;
        let with_ports: Vec<(ExternalConn, Vec<u16>)> =
            found.into_iter().map(|e| { let p = external::listen_ports(e.pid); (e, p) }).collect();
        let remembered_changed = self.remember_hosted_here(&with_ports, unexplained);
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
            listening_changed || notice_changed || remembered_changed
        }
    }

    /// Updates `Settings.hosted_here` from what's hosting on this device right now.
    fn remember_hosted_here(&self, externals: &[(ExternalConn, Vec<u16>)], admin_notice: bool) -> bool {
        let now = Instant::now();
        let summaries = self.cache.lock().unwrap().summaries.clone();
        let changed = {
            let mut seen = self.local_host_seen.lock().unwrap();
            let mut settings = self.settings.lock().unwrap();
            let mut changed = false;
            for s in &summaries {
                let id = &s.tunnel_id;
                let live = self.hoster.is_active(id)
                    || externals.iter().any(|(e, _)| e.role == Role::Host && external::matches_tunnel(&e.tunnel_arg, id));
                if live {
                    seen.insert(id.clone(), now);
                }
                let since = now.duration_since(seen.get(id).copied().unwrap_or(self.started));
                let remembered = settings.hosted_here.contains(id);
                let next = next_hosted_here(remembered, live, s.host_connections > 0, admin_notice, since);
                if next != remembered {
                    changed = true;
                    if next {
                        settings.hosted_here.insert(id.clone());
                    } else {
                        settings.hosted_here.remove(id);
                    }
                }
            }
            changed
        };
        if changed && let Err(e) = self.save_settings() {
            self.logs.push("_app", "app", &format!("Couldn't save settings: {e}"));
        }
        changed
    }

    /// Checks which hosted ports have a local server, so the UI can warn about "nothing listening".
    fn probe_hosted_ports(&self) -> HashSet<u16> {
        let maps: HashMap<String, BTreeMap<u16, u16>> = {
            let s = self.settings.lock().unwrap();
            s.tunnels.iter().filter(|(_, t)| !t.host_port_map.is_empty()).map(|(id, t)| (id.clone(), t.host_port_map.clone())).collect()
        };
        let ports: HashSet<u16> = {
            let c = self.cache.lock().unwrap();
            self.hoster
                .managed_ids()
                .iter()
                .flat_map(|id| {
                    let mut p: Vec<u16> = self.hoster.state(id).1.iter().map(|f| f.remote_port).collect();
                    p.extend(c.details.get(id).map(|d| d.ports.iter().map(|x| x.port_number).collect::<Vec<_>>()).unwrap_or_default());
                    // Mapped ports: probe the real server, not our own proxy.
                    let map = maps.get(id);
                    p.into_iter().map(|n| map.and_then(|m| m.get(&n).copied()).unwrap_or(n)).collect::<Vec<_>>()
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
        // Claim pinned ports first so `devtunnel connect` picks other local ports for the proxy to target.
        self.sync_pins(Some(id));
        self.supervisor.connect(id, cli);
        Ok(())
    }

    /// Runs pin proxies for every tunnel with pinned ports whose connection is active (plus `starting`).
    pub fn sync_pins(&self, starting: Option<&str>) {
        let wanted: Vec<(String, u16, u16)> = {
            let s = self.settings.lock().unwrap();
            s.tunnels
                .iter()
                .filter(|(id, _)| starting == Some(id.as_str()) || self.supervisor.is_active(id))
                .flat_map(|(id, t)| t.pinned_ports.iter().map(move |(&r, &l)| (id.clone(), r, l)))
                .collect()
        };
        self.pins.sync(&wanted);
    }

    /// Runs hosting port-map proxies for every tunnel this Mac hosts (plus `starting`).
    pub fn sync_host_maps(&self, starting: Option<&str>) {
        let wanted: Vec<(String, u16, u16)> = {
            let s = self.settings.lock().unwrap();
            s.tunnels
                .iter()
                .filter(|(id, _)| {
                    starting == Some(id.as_str())
                        || (self.hoster.is_active(id) && !matches!(self.hoster.state(id).0, ConnState::AuthRequired { .. }))
                })
                .flat_map(|(id, t)| t.host_port_map.iter().map(move |(&r, &l)| (id.clone(), r, l)))
                .collect()
        };
        *self.host_map_targets.lock().unwrap() = wanted.iter().map(|(id, r, l)| ((id.clone(), *r), *l)).collect();
        let entries: Vec<(String, u16, u16)> = wanted.into_iter().map(|(id, r, _)| (id, r, r)).collect();
        self.host_maps.sync(&entries);
    }

    /// Refuses a mapped tunnel port that something else already listens on locally (our own map proxy excepted),
    /// since `devtunnel host` would reach that server instead of the mapped one.
    fn check_map_port_free(&self, id: &str, remote: u16) -> Result<(), String> {
        if self.host_maps.view(id, remote, remote).listening {
            return Ok(());
        }
        match crate::pin::port_in_use(remote) {
            Some(addr) => Err(format!(
                "Port {remote} is already in use on this Mac ({addr}), so it can't forward to another local port. Choose a different tunnel port."
            )),
            None => Ok(()),
        }
    }

    /// Maps tunnel port `remote` to local server port `local` for hosting (None or local == remote clears it).
    /// Restarts our host so the proxy and published ports agree.
    pub async fn set_port_map(self: &Arc<Self>, id: &str, remote: u16, local: Option<u16>) -> Result<(), String> {
        if remote == 0 || local == Some(0) {
            return Err("Ports must be between 1 and 65535.".into());
        }
        let local = local.filter(|&l| l != remote);
        if local.is_some() {
            self.check_map_port_free(id, remote)?;
        }
        {
            let mut s = self.settings.lock().unwrap();
            let t = s.tunnels.entry(id.to_string()).or_default();
            match local {
                Some(l) => t.host_port_map.insert(remote, l),
                None => t.host_port_map.remove(&remote),
            };
        }
        self.save_settings()?;
        self.sync_host_maps(None);
        self.logs.push(
            id,
            "app",
            &match local {
                Some(l) => format!("Tunnel port {remote} now forwards to localhost:{l} while hosted here"),
                None => format!("Tunnel port {remote} forwards to localhost:{remote} again"),
            },
        );
        self.publish();
        Ok(())
    }

    /// Sets (local Some) or clears (None) the stable local port for one remote port.
    pub fn set_pin(&self, id: &str, remote_port: u16, local_port: Option<u16>) -> Result<(), String> {
        if remote_port == 0 || local_port == Some(0) {
            return Err("Ports must be between 1 and 65535.".into());
        }
        {
            let mut s = self.settings.lock().unwrap();
            if let Some(l) = local_port
                && let Some((other, _)) = s
                    .tunnels
                    .iter()
                    .flat_map(|(tid, t)| t.pinned_ports.iter().map(move |(&r, &lp)| (tid, r, lp)))
                    .find(|&(tid, r, lp)| lp == l && !(tid == id && r == remote_port))
                    .map(|(tid, r, _)| (tid.clone(), r))
            {
                return Err(format!("localhost:{l} is already pinned for {other}."));
            }
            let t = s.tunnels.entry(id.to_string()).or_default();
            match local_port {
                Some(l) => t.pinned_ports.insert(remote_port, l),
                None => t.pinned_ports.remove(&remote_port),
            };
        }
        self.save_settings()?;
        self.sync_pins(None);
        self.publish();
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
        let here = self.hoster.is_active(id);
        self.cache.lock().unwrap().summaries.iter().any(|s| s.tunnel_id == id && hosted_elsewhere_count(s.host_connections, here))
    }

    /// Re-reads one tunnel with `devtunnel show` so guards don't act on a list that may be a minute old.
    async fn reload_summary(&self, cli: &Path, id: &str) -> Result<(), String> {
        let d = cli::show(cli, id).await?;
        let mut c = self.cache.lock().unwrap();
        if let Some(s) = c.summaries.iter_mut().find(|s| s.tunnel_id == id) {
            s.host_connections = d.host_connections;
            s.labels = d.labels.clone();
        }
        c.details.insert(id.to_string(), d);
        Ok(())
    }

    pub async fn host(self: &Arc<Self>, id: &str) -> Result<(), String> {
        if let Some(pid) = self.external_host_pid(id) {
            return Err(format!("Already hosted outside Tunnel Bar (PID {pid})."));
        }
        if self.hosted_elsewhere(id) {
            return Err("This tunnel is already hosted by another machine. Hosting it here too would split its traffic.".into());
        }
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        let mapped: Vec<u16> = self.settings.lock().unwrap().tunnels.get(id).map(|t| t.host_port_map.keys().copied().collect()).unwrap_or_default();
        for r in mapped {
            self.check_map_port_free(id, r)?;
        }
        // Start the map proxies before `devtunnel host` so its first connections reach the mapped servers.
        self.sync_host_maps(Some(id));
        self.hoster.connect(id, cli);
        Ok(())
    }

    pub fn stop_host(self: &Arc<Self>, id: &str) {
        self.hoster.disconnect(id);
        // The relay's host count lags; refresh so the card doesn't claim "hosted elsewhere".
        self.refresh_later(3);
    }

    /// The service is eventually consistent; re-read after it settles.
    pub fn refresh_later(self: &Arc<Self>, secs: u64) {
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(secs)).await;
            this.refresh().await;
        });
    }

    /// Waits until neither role runs a child for `id` (max ~4s).
    async fn wait_stopped(&self, id: &str) {
        for _ in 0..40 {
            if !self.hoster.is_active(id) && !self.supervisor.is_active(id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// `devtunnel host` publishes the ports that exist when it starts, so restart it after port changes.
    async fn restart_host_if_running(&self, id: &str, cli: PathBuf) {
        if !self.hoster.is_active(id) {
            return;
        }
        self.logs.push(id, "app", "Restarting host to publish the updated ports");
        self.hoster.disconnect(id);
        for _ in 0..40 {
            if !self.hoster.is_active(id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.hoster.connect(id, cli);
    }

    fn summary(&self, id: &str) -> Option<TunnelSummary> {
        self.cache.lock().unwrap().summaries.iter().find(|s| s.tunnel_id == id).cloned()
    }

    fn created_by_app(&self, id: &str) -> bool {
        self.summary(id).is_some_and(|s| s.labels.iter().any(|l| l == APP_LABEL))
    }

    /// Creates a tunnel (always labelled `tunnelbar`), adds ports/access, and optionally hosts it.
    pub async fn create_tunnel(self: &Arc<Self>, req: CreateTunnelRequest) -> Result<String, String> {
        if let Some(id) = req.tunnel_id.as_deref().filter(|i| !i.trim().is_empty()) {
            cli::validate_tunnel_id(id.trim())?;
        }
        let mut labels: Vec<String> = vec![APP_LABEL.into()];
        for l in req.labels.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
            validate_label(l)?;
            if !labels.iter().any(|x| x.eq_ignore_ascii_case(l)) {
                labels.push(l.to_string());
            }
        }
        let expiration = req.expiration.as_deref().filter(|e| !e.trim().is_empty()).map(normalize_expiration).transpose()?;
        // (tunnel port, protocol, headers) plus tunnel port -> local port maps.
        let mut ports: Vec<(u16, &'static str, HeaderOptions)> = Vec::new();
        let mut port_map: BTreeMap<u16, u16> = BTreeMap::new();
        for p in &req.ports {
            let remote = p.remote_port.unwrap_or(p.port);
            if p.port == 0 || remote == 0 {
                return Err("Ports must be between 1 and 65535.".into());
            }
            if ports.iter().any(|(n, _, _)| *n == remote) {
                return Err(format!("Port {remote} is listed twice."));
            }
            if remote != p.port {
                self.check_map_port_free("", remote)?;
                port_map.insert(remote, p.port);
            }
            ports.push((remote, validate_protocol(&p.protocol)?, p.headers.normalized()?));
        }
        if req.host && ports.is_empty() {
            return Err("Add at least one port to host this tunnel.".into());
        }
        let headers = req.headers.normalized()?;
        let rules = req.access_rules.iter().map(AccessRuleRequest::normalized).collect::<Result<Vec<_>, _>>()?;
        if let Some(p) = rules.iter().filter_map(|r| r.port).find(|p| !ports.iter().any(|(n, _, _)| n == p)) {
            return Err(format!("Access rule for port {p}, which isn't in the port list."));
        }
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let id = cli::create(
            &cli,
            req.tunnel_id.as_deref().map(str::trim).filter(|i| !i.is_empty()),
            &label_refs,
            req.description.trim(),
            expiration.as_deref(),
            &headers,
        )
        .await?;
        self.logs.push(&id, "app", &format!("Created tunnel ({} port(s))", ports.len()));
        let setup = async {
            for (port, protocol, h) in &ports {
                cli::port_create(&cli, &id, *port, protocol, h).await?;
            }
            if req.anonymous {
                cli::set_anonymous(&cli, &id, true).await?;
            }
            for r in &rules {
                cli::access_create(&cli, &id, r).await?;
            }
            Ok::<(), String>(())
        };
        if let Err(e) = setup.await {
            // We just created it, so rolling back can't touch anything else.
            let _ = cli::delete(&cli, &id).await;
            return Err(e);
        }
        let nickname = req.nickname.as_deref().map(str::trim).filter(|n| !n.is_empty());
        if nickname.is_some() || !port_map.is_empty() {
            {
                let mut s = self.settings.lock().unwrap();
                let t = s.tunnels.entry(id.clone()).or_default();
                if let Some(n) = nickname {
                    t.nickname = Some(n.to_string());
                }
                t.host_port_map = port_map;
            }
            self.save_settings()?;
        }
        self.refresh().await;
        if req.host {
            self.sync_host_maps(Some(&id));
            self.hoster.connect(&id, cli);
        }
        // The service can lag on new tunnels/access rules; re-read once it settles.
        self.refresh_later(4);
        Ok(id)
    }

    /// Edits description, labels, expiration and/or tunnel-level header rewriting. Header changes on
    /// a tunnel another machine hosts need `allow_hosted_elsewhere`.
    pub async fn update_tunnel(
        self: &Arc<Self>,
        id: &str,
        mut req: UpdateTunnelRequest,
        confirm: bool,
        allow_hosted_elsewhere: bool,
    ) -> Result<(), String> {
        req.headers = req.headers.normalized()?;
        if !req.headers.is_empty() {
            self.check_port_change(id, allow_hosted_elsewhere)?;
        }
        for l in req.add_labels.iter_mut().chain(req.remove_labels.iter_mut()) {
            *l = l.trim().to_string();
            validate_label(l)?;
        }
        if let Some(d) = req.description.as_mut() {
            *d = d.trim().to_string();
        }
        req.expiration = req.expiration.as_deref().filter(|e| !e.trim().is_empty()).map(normalize_expiration).transpose()?;
        check_update(&req, self.created_by_app(id), confirm)?;
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        cli::update(&cli, id, &req).await?;
        self.logs.push(id, "app", "Tunnel properties updated");
        let restart_host = !req.headers.is_empty();
        {
            let mut c = self.cache.lock().unwrap();
            let apply = |labels: &mut Vec<String>, description: &mut String, expiration: &mut String| {
                labels.retain(|l| !req.remove_labels.iter().any(|r| r.eq_ignore_ascii_case(l)));
                for a in &req.add_labels {
                    if !labels.iter().any(|l| l.eq_ignore_ascii_case(a)) {
                        labels.push(a.clone());
                    }
                }
                if let Some(d) = &req.description {
                    *description = d.clone();
                }
                if let Some(h) = req.expiration.as_deref().and_then(parse_expiration_hours) {
                    *expiration = if h % 24 == 0 { format!("{} days", h / 24) } else { format!("{h} hours") };
                }
            };
            if let Some(s) = c.summaries.iter_mut().find(|s| s.tunnel_id == id) {
                apply(&mut s.labels, &mut s.description, &mut s.tunnel_expiration);
            }
            if let Some(d) = c.details.get_mut(id) {
                apply(&mut d.labels, &mut d.description, &mut d.tunnel_expiration);
            }
        }
        self.publish();
        if restart_host {
            self.restart_host_if_running(id, cli).await;
        }
        self.refresh_later(4);
        Ok(())
    }

    /// Ports on a tunnel another machine hosts belong to that machine's setup; require an explicit override.
    fn check_port_change(&self, id: &str, allow_hosted_elsewhere: bool) -> Result<(), String> {
        if self.hosted_elsewhere(id) && !allow_hosted_elsewhere {
            return Err(format!(
                "{HOSTED_ELSEWHERE}This tunnel is hosted by another machine; changing its ports affects that machine's host."
            ));
        }
        Ok(())
    }

    /// Adds tunnel port `remote_port` (default: `port`). When it differs, hosting here forwards it to
    /// localhost:`port` through a loopback proxy (see `set_port_map`).
    #[allow(clippy::too_many_arguments)]
    pub async fn add_port(
        self: &Arc<Self>,
        id: &str,
        port: u16,
        remote_port: Option<u16>,
        protocol: &str,
        headers: &HeaderOptions,
        allow_hosted_elsewhere: bool,
    ) -> Result<(), String> {
        let local = port;
        let port = remote_port.unwrap_or(local);
        if port == 0 || local == 0 {
            return Err("Enter a port between 1 and 65535.".into());
        }
        let protocol = validate_protocol(protocol)?;
        let headers = headers.normalized()?;
        self.check_port_change(id, allow_hosted_elsewhere)?;
        if port != local {
            self.check_map_port_free(id, port)?;
        }
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        cli::port_create(&cli, id, port, protocol, &headers).await?;
        if port != local {
            self.settings.lock().unwrap().tunnels.entry(id.to_string()).or_default().host_port_map.insert(port, local);
            self.save_settings()?;
            self.sync_host_maps(None);
            self.logs.push(id, "app", &format!("Added port {port} ({protocol}) -> localhost:{local}"));
        } else {
            self.logs.push(id, "app", &format!("Added port {port} ({protocol})"));
        }
        {
            let mut c = self.cache.lock().unwrap();
            if let Some(d) = c.details.get_mut(id).filter(|d| !d.ports.iter().any(|p| p.port_number == port)) {
                d.ports.push(TunnelPort {
                    port_number: port,
                    protocol: protocol.into(),
                    host_header: headers.host_header.clone(),
                    origin_header: headers.origin_header.clone(),
                    local_port: (port != local).then_some(local),
                    ..Default::default()
                });
                d.ports.sort_by_key(|p| p.port_number);
            }
            if let Some(s) = c.summaries.iter_mut().find(|s| s.tunnel_id == id) {
                s.port_count += 1;
            }
        }
        self.publish();
        self.restart_host_if_running(id, cli).await;
        self.refresh_later(4);
        Ok(())
    }

    /// Removes a port. Needs `confirm` unless Tunnel Bar created the tunnel.
    pub async fn remove_port(
        self: &Arc<Self>,
        id: &str,
        port: u16,
        confirm: bool,
        allow_hosted_elsewhere: bool,
    ) -> Result<(), String> {
        self.check_port_change(id, allow_hosted_elsewhere)?;
        if !confirm && !self.created_by_app(id) {
            return Err(format!("{CONFIRM_REQUIRED}This tunnel wasn't created by Tunnel Bar. Confirm to remove port {port}."));
        }
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        cli::port_delete(&cli, id, port).await?;
        self.logs.push(id, "app", &format!("Removed port {port}"));
        let pinned = {
            let mut c = self.cache.lock().unwrap();
            if let Some(d) = c.details.get_mut(id) {
                d.ports.retain(|p| p.port_number != port);
            }
            if let Some(s) = c.summaries.iter_mut().find(|s| s.tunnel_id == id) {
                s.port_count = s.port_count.saturating_sub(1);
            }
            let mut s = self.settings.lock().unwrap();
            let t = s.tunnels.get_mut(id);
            t.map(|t| (t.pinned_ports.remove(&port).is_some(), t.host_port_map.remove(&port).is_some())).unwrap_or_default()
        };
        if pinned.0 || pinned.1 {
            self.save_settings()?;
            self.sync_pins(None);
            self.sync_host_maps(None);
        }
        self.publish();
        self.restart_host_if_running(id, cli).await;
        self.refresh_later(4);
        Ok(())
    }

    /// Changes one port's Host/Origin header rewriting and restarts our host so it takes effect.
    pub async fn set_port_headers(
        self: &Arc<Self>,
        id: &str,
        port: u16,
        headers: &HeaderOptions,
        allow_hosted_elsewhere: bool,
    ) -> Result<(), String> {
        let headers = headers.normalized()?;
        if headers.is_empty() {
            return Err("Choose a Host or Origin header setting.".into());
        }
        self.check_port_change(id, allow_hosted_elsewhere)?;
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        cli::port_update_headers(&cli, id, port, &headers).await?;
        self.logs.push(id, "app", &format!("Updated header rewriting for port {port}"));
        if let Some(p) = self.cache.lock().unwrap().details.get_mut(id).and_then(|d| d.ports.iter_mut().find(|p| p.port_number == port)) {
            if headers.host_header.is_some() {
                p.host_header = headers.host_header.clone();
            }
            if headers.origin_header.is_some() {
                p.origin_header = headers.origin_header.clone();
            }
        }
        self.publish();
        self.restart_host_if_running(id, cli).await;
        self.refresh_later(4);
        Ok(())
    }

    /// Access rules gate who can connect, so changing them on a tunnel Tunnel Bar didn't create
    /// (e.g. the user's dev-box SSH tunnel) needs `confirm`.
    fn check_access_change(&self, id: &str, confirm: bool) -> Result<(), String> {
        if !confirm && !self.created_by_app(id) {
            return Err(format!("{CONFIRM_REQUIRED}This tunnel wasn't created by Tunnel Bar. Confirm to change who can connect to it."));
        }
        Ok(())
    }

    /// Re-reads tunnel-level rules into the cache (`show` lags behind `access list`).
    async fn reload_access(&self, cli: &Path, id: &str) {
        if let Ok(rules) = cli::access_list(cli, id, None).await
            && let Some(d) = self.cache.lock().unwrap().details.get_mut(id)
        {
            d.access_control = rules;
        }
        self.publish();
    }

    pub async fn list_access_rules(&self, id: &str, port: Option<u16>) -> Result<Vec<AccessEntry>, String> {
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        cli::access_list(&cli, id, port).await
    }

    pub async fn add_access_rule(self: &Arc<Self>, id: &str, rule: &AccessRuleRequest, confirm: bool) -> Result<(), String> {
        let rule = rule.normalized()?;
        self.check_access_change(id, confirm)?;
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        cli::access_create(&cli, id, &rule).await?;
        let scope = rule.port.map(|p| format!(" on port {p}")).unwrap_or_default();
        let what = if rule.deny { "deny" } else { "allow" };
        let subject = rule.value.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
        self.logs.push(id, "app", &format!("Added {what} access rule: {:?}{subject}{scope}", rule.kind));
        self.reload_access(&cli, id).await;
        self.refresh_later(4);
        Ok(())
    }

    /// Removes the own rule at `index` (from `accessRules` / `list_access_rules`).
    pub async fn remove_access_rule(self: &Arc<Self>, id: &str, port: Option<u16>, index: usize, confirm: bool) -> Result<(), String> {
        self.check_access_change(id, confirm)?;
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        cli::access_delete(&cli, id, port, index).await?;
        let scope = port.map(|p| format!(" on port {p}")).unwrap_or_default();
        self.logs.push(id, "app", &format!("Removed access rule #{index}{scope}"));
        self.reload_access(&cli, id).await;
        self.refresh_later(4);
        Ok(())
    }

    /// Creates a Tunnel Bar-labelled tunnel for localhost:port and starts hosting it.
    #[allow(clippy::too_many_arguments)]
    pub async fn expose(
        self: &Arc<Self>,
        port: u16,
        remote_port: Option<u16>,
        protocol: &str,
        anonymous: bool,
        name: Option<&str>,
        headers: HeaderOptions,
        access_rules: Vec<AccessRuleRequest>,
    ) -> Result<String, String> {
        if !matches!(protocol, "http" | "https" | "auto") {
            return Err(format!("Unsupported protocol {protocol}"));
        }
        if port == 0 {
            return Err("Enter a port between 1 and 65535.".into());
        }
        self.create_tunnel(CreateTunnelRequest {
            tunnel_id: name.map(str::to_string),
            description: format!("localhost:{port} exposed by Tunnel Bar"),
            ports: vec![PortSpec { port, remote_port, protocol: protocol.into(), headers: Default::default() }],
            anonymous,
            headers,
            access_rules,
            host: true,
            nickname: name.is_none().then(|| format!("localhost:{port}")),
            ..Default::default()
        })
        .await
    }

    pub async fn set_anonymous(self: &Arc<Self>, id: &str, allow: bool, confirm: bool) -> Result<(), String> {
        self.check_access_change(id, confirm)?;
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
        self.refresh_later(4);
        Ok(())
    }

    pub fn delete_preflight(&self, id: &str) -> Result<DeletePreflight, String> {
        let summary = self.summary(id).ok_or("Tunnel not found on this account. Refresh and try again.")?;
        let created_by_app = summary.labels.iter().any(|l| l == APP_LABEL);
        Ok(DeletePreflight {
            created_by_app,
            needs_confirm: !created_by_app,
            hosted_elsewhere: self.hosted_elsewhere(id),
            hosting_here: self.hoster.is_active(id),
            connected_here: self.supervisor.is_active(id),
        })
    }

    /// Deletes a tunnel. Tunnels Tunnel Bar didn't create need `confirm`; tunnels hosted by another
    /// machine (e.g. a dev box) also need `allow_hosted_elsewhere`.
    pub async fn delete_tunnel(self: &Arc<Self>, id: &str, confirm: bool, allow_hosted_elsewhere: bool) -> Result<(), String> {
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        self.reload_summary(&cli, id).await?;
        check_delete(&self.delete_preflight(id)?, confirm, allow_hosted_elsewhere)?;
        self.hoster.disconnect(id);
        self.supervisor.disconnect(id);
        self.wait_stopped(id).await;
        cli::delete(&cli, id).await?;
        self.settings.lock().unwrap().tunnels.remove(id);
        self.save_settings()?;
        self.sync_pins(None);
        self.sync_host_maps(None);
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

    /// Starts `devtunnel user login` in the background; progress is published as `Snapshot.login`.
    pub async fn start_login(self: &Arc<Self>, provider: LoginProvider, device_code: bool) -> Result<(), String> {
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        let cancel = Arc::new(tokio::sync::Notify::new());
        {
            let mut cur = self.login_cancel.lock().unwrap();
            if cur.is_some() {
                return Err("A sign-in is already in progress.".into());
            }
            *cur = Some(cancel.clone());
        }
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let (a, b) = (this.clone(), this.clone());
            let fin = login::run(
                &cli,
                provider,
                device_code,
                cancel,
                this.login_pid.clone(),
                login::LOGIN_TIMEOUT,
                move |st| {
                    *a.login.lock().unwrap() = Some(st.clone());
                    a.publish();
                },
                move |line| {
                    b.logs.push("_app", "login", line);
                },
            )
            .await;
            *this.login_cancel.lock().unwrap() = None;
            if matches!(fin, LoginState::Succeeded { .. }) {
                this.refresh().await;
                this.auto_connect().await;
            }
        });
        Ok(())
    }

    pub fn cancel_login(&self) {
        if let Some(c) = self.login_cancel.lock().unwrap().as_ref() {
            c.notify_one();
        }
    }

    /// Signs the CLI out. Running connections/hosts are left alone (they keep their relay sessions).
    pub async fn logout(self: &Arc<Self>) -> Result<(), String> {
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        cli::logout(&cli).await?;
        self.logs.push("_app", "app", "Signed out of devtunnel");
        {
            let mut c = self.cache.lock().unwrap();
            c.auth = Some(AuthStatus { logged_in: false, message: "Not logged in.".into() });
            c.summaries.clear();
            c.details.clear();
        }
        *self.login.lock().unwrap() = None;
        self.publish();
        self.refresh().await;
        Ok(())
    }

    pub fn set_label_filter(&self, labels: Vec<String>) -> Result<(), String> {
        let mut clean: Vec<String> = labels.into_iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        clean.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        self.settings.lock().unwrap().label_filter = clean;
        self.save_settings()?;
        self.publish();
        Ok(())
    }

    pub fn shutdown(&self) {
        // Stop both roles concurrently so quitting stays within one grace period.
        let h = self.hoster.clone();
        let t = std::thread::spawn(move || h.shutdown());
        self.supervisor.shutdown();
        let _ = t.join();
        self.pins.stop_all();
        self.host_maps.stop_all();
        // The login task won't get to run before the process exits; stop its CLI now.
        login::stop(&self.login_pid);
        self.cancel_login();
    }

    /// Stops an external `devtunnel connect`, or failing that `devtunnel host`, for this tunnel
    /// (signals only that PID, not its terminal's process group). Returns the role that was stopped.
    pub async fn stop_external(self: &Arc<Self>, id: &str) -> Result<Role, String> {
        let (pid, role) = match self.external_pid(id) {
            Some(p) => (p, Role::Connect),
            None => (self.external_host_pid(id).ok_or("No external devtunnel process found")?, Role::Host),
        };
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        #[cfg(windows)]
        if crate::win::can_terminate(pid) {
            crate::supervisor::signal_group(pid, true);
        } else {
            // Started from an elevated terminal: only an elevated taskkill can stop it.
            let r = tauri::async_runtime::spawn_blocking(move || crate::win::kill_tree_elevated(pid))
                .await
                .map_err(|e| e.to_string())?;
            if let Err(declined) = r {
                return Err(if declined {
                    "Started from an admin terminal \u{2014} stop it there to let Tunnel Bar manage it.".into()
                } else {
                    format!("Couldn't stop devtunnel (PID {pid}).")
                });
            }
        }
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
        if role == Role::Host {
            // The relay's host count lags; until it catches up the tunnel would look hosted elsewhere.
            let _ = self.reload_summary_after_stop(id).await;
        }
        self.scan_externals();
        self.publish();
        Ok(role)
    }

    /// Waits briefly for the service to drop the stopped host, so taking over doesn't trip the "hosted elsewhere" guard.
    async fn reload_summary_after_stop(&self, id: &str) -> Result<(), String> {
        let cli = self.cli_path().await.ok_or("devtunnel CLI not found")?;
        for _ in 0..10 {
            self.reload_summary(&cli, id).await?;
            if !self.hosted_elsewhere(id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
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
            let hosted_here = self.hoster.is_active(&id) || self.external_host_pid(&id).is_some();
            if should_auto_connect(self.supervisor.is_active(&id), self.external_pid(&id).is_some(), hosted_here) {
                let _ = self.connect(&id).await;
            } else if hosted_here {
                self.logs.push(&id, "app", "Auto-connect skipped: hosted on this device");
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
            let resumed = tick().await;
            ticks += 1;
            let now = chrono::Local::now();
            let gap = (now - last_wall).num_seconds();
            last_wall = now;
            if resumed || gap > 30 {
                // Woke from sleep: retry immediately instead of waiting out backoff.
                if resumed {
                    state.logs.push("_app", "app", "Resumed from sleep");
                    // Give the network a moment to come back.
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    last_wall = chrono::Local::now();
                } else {
                    state.logs.push("_app", "app", &format!("Wake detected (gap {gap}s)"));
                }
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

/// Waits for the next 5s tick; returns true if the OS reported a resume from sleep instead.
/// The wall-clock gap check covers macOS, where the monotonic clock pauses during sleep.
async fn tick() -> bool {
    #[cfg(windows)]
    {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => false,
            _ = crate::win::resumed().notified() => true,
        }
    }
    #[cfg(not(windows))]
    {
        tokio::time::sleep(Duration::from_secs(5)).await;
        false
    }
}

pub fn state(app: &AppHandle) -> Shared {
    app.state::<Shared>().inner().clone()
}

fn hosted_ports(ports: &[TunnelPort], live: &[Forward], listening: &HashSet<u16>, map: &BTreeMap<u16, u16>) -> Vec<HostedPort> {
    let mut nums: Vec<u16> = ports.iter().map(|p| p.port_number).collect();
    nums.extend(live.iter().map(|f| f.remote_port));
    nums.sort();
    nums.dedup();
    nums.into_iter()
        .map(|n| {
            let p = ports.iter().find(|p| p.port_number == n);
            let uri = p.map(|p| p.port_uri.clone()).unwrap_or_default();
            let local = map.get(&n).copied().unwrap_or(n);
            HostedPort {
                port: n,
                local_port: local,
                protocol: p.map(|p| p.protocol.clone()).unwrap_or_default(),
                inspect_uri: inspect_uri(&uri),
                uri,
                listening: listening.contains(&local),
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

/// Auto-connect only when not already connected (by us or externally) and not hosted on this device,
/// where connecting would just loop back to our own host. The saved setting is left untouched.
fn should_auto_connect(connected_here: bool, external_client: bool, hosted_here: bool) -> bool {
    !connected_here && !external_client && !hosted_here
}

/// How long another device must be seen hosting, with no host here, before we forget hosting it.
/// Covers the service still reporting our own just-stopped host.
const HOSTED_HERE_GRACE: Duration = Duration::from_secs(60);

/// Whether to remember this device as the tunnel's host. `since_local` is the time since a host was
/// last seen here (or since launch). An unattributed admin `devtunnel` may be that host, so keep it.
fn next_hosted_here(remembered: bool, live: bool, host_online: bool, admin_notice: bool, since_local: Duration) -> bool {
    if live {
        return true;
    }
    if remembered && host_online && !admin_notice && since_local >= HOSTED_HERE_GRACE {
        return false;
    }
    remembered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_connect_skips_tunnels_hosted_on_this_device() {
        assert!(should_auto_connect(false, false, false));
        assert!(!should_auto_connect(false, false, true));
        assert!(!should_auto_connect(true, false, false));
        assert!(!should_auto_connect(false, true, false));
    }

    #[test]
    fn remembers_hosting_here_until_another_device_hosts() {
        let (s0, long) = (Duration::ZERO, HOSTED_HERE_GRACE);
        // Hosting here (by Tunnel Bar or an external host): remember.
        assert!(next_hosted_here(false, true, true, false, s0));
        // Host went down: keep it, so the card can say this device hosted it last.
        assert!(next_hosted_here(true, false, false, false, long));
        // Service still lists the host we just stopped: keep within the grace period.
        assert!(next_hosted_here(true, false, true, false, Duration::from_secs(5)));
        // Another device has been hosting past the grace period: forget.
        assert!(!next_hosted_here(true, false, true, false, long));
        // An unattributed admin devtunnel may be our host: keep.
        assert!(next_hosted_here(true, false, true, true, long));
        // Never-hosted tunnels stay unremembered.
        assert!(!next_hosted_here(false, false, true, false, long));
    }

    fn pf(created_by_app: bool, hosted_elsewhere: bool) -> DeletePreflight {
        DeletePreflight { created_by_app, needs_confirm: !created_by_app, hosted_elsewhere, hosting_here: false, connected_here: false }
    }

    #[test]
    fn hosted_ports_report_mapped_local_server() {
        let ports = vec![
            TunnelPort { port_number: 2222, protocol: "auto".into(), ..Default::default() },
            TunnelPort { port_number: 3000, protocol: "http".into(), ..Default::default() },
        ];
        let map = BTreeMap::from([(2222, 22)]);
        // Only localhost:22 is up; 2222 is merely our proxy and doesn't count.
        let h = hosted_ports(&ports, &[], &HashSet::from([22]), &map);
        assert_eq!((h[0].port, h[0].local_port, h[0].listening), (2222, 22, true));
        assert_eq!((h[1].port, h[1].local_port, h[1].listening), (3000, 3000, false));
        let h = hosted_ports(&ports, &[], &HashSet::from([2222]), &map);
        assert!(!h[0].listening);
    }

    #[test]
    fn update_guards_ownership_label_and_expiration() {
        let req = |add: &[&str], remove: &[&str], exp: Option<&str>| UpdateTunnelRequest {
            add_labels: add.iter().map(|s| s.to_string()).collect(),
            remove_labels: remove.iter().map(|s| s.to_string()).collect(),
            expiration: exp.map(Into::into),
            ..Default::default()
        };
        // Forging/dropping ownership on a foreign tunnel is refused even with confirm.
        for r in [req(&["tunnelbar"], &[], None), req(&["TunnelBar"], &[], None), req(&[], &["tunnelbar"], None)] {
            let e = check_update(&r, false, true).unwrap_err();
            assert!(!e.starts_with(CONFIRM_REQUIRED), "{e}");
        }
        // Expiration on a foreign tunnel (e.g. 1h on the dev-box SSH tunnel) needs confirm.
        let exp = req(&[], &[], Some("1h"));
        assert!(check_update(&exp, false, false).unwrap_err().starts_with(CONFIRM_REQUIRED));
        assert!(check_update(&exp, false, true).is_ok());
        // Other edits on foreign tunnels, and anything on our own tunnels, pass.
        assert!(check_update(&req(&["work"], &["old"], None), false, false).is_ok());
        assert!(check_update(&req(&[], &["tunnelbar"], Some("1h")), true, false).is_ok());
    }

    #[test]
    fn hosted_elsewhere_counts_our_host() {
        assert!(!hosted_elsewhere_count(0, false));
        assert!(hosted_elsewhere_count(1, false));
        assert!(!hosted_elsewhere_count(1, true));
        // A dev box hosting while this Mac also hosts.
        assert!(hosted_elsewhere_count(2, true));
    }

    #[test]
    fn delete_guards() {
        assert!(check_delete(&pf(true, false), false, false).is_ok());
        let e = check_delete(&pf(false, false), false, false).unwrap_err();
        assert!(e.starts_with(CONFIRM_REQUIRED), "{e}");
        assert!(check_delete(&pf(false, false), true, false).is_ok());
        // Hosted elsewhere (e.g. the dev box SSH tunnel): confirm alone is not enough.
        let e = check_delete(&pf(false, true), true, false).unwrap_err();
        assert!(e.starts_with(HOSTED_ELSEWHERE), "{e}");
        let e = check_delete(&pf(true, true), false, false).unwrap_err();
        assert!(e.starts_with(HOSTED_ELSEWHERE), "{e}");
        assert!(check_delete(&pf(false, true), false, true).unwrap_err().starts_with(CONFIRM_REQUIRED));
        assert!(check_delete(&pf(false, true), true, true).is_ok());
    }
}
