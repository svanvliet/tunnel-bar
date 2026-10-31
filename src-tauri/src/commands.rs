use crate::app::Shared;
use crate::model::{AccessEntry, AccessRuleRequest, CreateTunnelRequest, HeaderOptions, DeletePreflight, LogLine, LoginProvider, Snapshot, TunnelSettings, UpdateTunnelRequest};
use crate::ssh;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub fn get_snapshot(state: State<'_, Shared>) -> Snapshot {
    state.snapshot()
}

#[tauri::command]
pub async fn refresh(state: State<'_, Shared>) -> Result<(), String> {
    state.inner().clone().refresh().await;
    Ok(())
}

#[tauri::command]
pub async fn connect(state: State<'_, Shared>, id: String) -> Result<(), String> {
    state.inner().clone().connect(&id).await
}

#[tauri::command]
pub fn disconnect(state: State<'_, Shared>, id: String) {
    state.supervisor.disconnect(&id);
}

#[tauri::command]
pub async fn host(state: State<'_, Shared>, id: String) -> Result<(), String> {
    state.inner().clone().host(&id).await
}

#[tauri::command]
pub fn stop_host(state: State<'_, Shared>, id: String) {
    state.inner().stop_host(&id);
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn expose_port(
    state: State<'_, Shared>,
    port: u16,
    protocol: String,
    anonymous: bool,
    name: Option<String>,
    host_header: Option<String>,
    origin_header: Option<String>,
    access_rules: Option<Vec<AccessRuleRequest>>,
) -> Result<String, String> {
    let headers = HeaderOptions { host_header, origin_header };
    state.inner().clone().expose(port, &protocol, anonymous, name.as_deref(), headers, access_rules.unwrap_or_default()).await
}

#[tauri::command]
pub async fn set_anonymous(state: State<'_, Shared>, id: String, allow: bool) -> Result<(), String> {
    state.inner().clone().set_anonymous(&id, allow).await
}

/// Fails with a `CONFIRM_REQUIRED: ` / `HOSTED_ELSEWHERE: ` prefixed message until the matching flag is passed.
#[tauri::command]
pub async fn delete_tunnel(
    state: State<'_, Shared>,
    id: String,
    confirm: Option<bool>,
    allow_hosted_elsewhere: Option<bool>,
) -> Result<(), String> {
    state.inner().clone().delete_tunnel(&id, confirm.unwrap_or(false), allow_hosted_elsewhere.unwrap_or(false)).await
}

#[tauri::command]
pub fn delete_preflight(state: State<'_, Shared>, id: String) -> Result<DeletePreflight, String> {
    state.delete_preflight(&id)
}

#[tauri::command]
pub async fn create_tunnel(state: State<'_, Shared>, request: CreateTunnelRequest) -> Result<String, String> {
    state.inner().clone().create_tunnel(request).await
}

#[tauri::command]
pub async fn update_tunnel(
    state: State<'_, Shared>,
    id: String,
    update: UpdateTunnelRequest,
    allow_hosted_elsewhere: Option<bool>,
) -> Result<(), String> {
    state.inner().clone().update_tunnel(&id, update, allow_hosted_elsewhere.unwrap_or(false)).await
}

#[tauri::command]
pub async fn add_port(
    state: State<'_, Shared>,
    id: String,
    port: u16,
    protocol: Option<String>,
    host_header: Option<String>,
    origin_header: Option<String>,
    allow_hosted_elsewhere: Option<bool>,
) -> Result<(), String> {
    let protocol = protocol.unwrap_or_default();
    let headers = HeaderOptions { host_header, origin_header };
    state.inner().clone().add_port(&id, port, &protocol, &headers, allow_hosted_elsewhere.unwrap_or(false)).await
}

/// `headers` is `{ hostHeader?, originHeader? }`; `"unchanged"` passes the browser's value through.
#[tauri::command]
pub async fn set_port_headers(
    state: State<'_, Shared>,
    id: String,
    port: u16,
    headers: HeaderOptions,
    allow_hosted_elsewhere: Option<bool>,
) -> Result<(), String> {
    state.inner().clone().set_port_headers(&id, port, &headers, allow_hosted_elsewhere.unwrap_or(false)).await
}

#[tauri::command]
pub async fn list_access_rules(state: State<'_, Shared>, id: String, port: Option<u16>) -> Result<Vec<AccessEntry>, String> {
    state.list_access_rules(&id, port).await
}

/// Fails with `CONFIRM_REQUIRED: ` on tunnels Tunnel Bar didn't create until `confirm` is passed.
#[tauri::command]
pub async fn add_access_rule(state: State<'_, Shared>, id: String, rule: AccessRuleRequest, confirm: Option<bool>) -> Result<(), String> {
    state.inner().clone().add_access_rule(&id, &rule, confirm.unwrap_or(false)).await
}

/// `index` is the entry's `index` from `accessRules` / `list_access_rules` (inherited entries have none).
#[tauri::command]
pub async fn remove_access_rule(
    state: State<'_, Shared>,
    id: String,
    port: Option<u16>,
    index: usize,
    confirm: Option<bool>,
) -> Result<(), String> {
    state.inner().clone().remove_access_rule(&id, port, index, confirm.unwrap_or(false)).await
}

#[tauri::command]
pub async fn remove_port(
    state: State<'_, Shared>,
    id: String,
    port: u16,
    confirm: Option<bool>,
    allow_hosted_elsewhere: Option<bool>,
) -> Result<(), String> {
    state.inner().clone().remove_port(&id, port, confirm.unwrap_or(false), allow_hosted_elsewhere.unwrap_or(false)).await
}

/// `localPort: null` removes the pin.
#[tauri::command]
pub fn set_pinned_port(state: State<'_, Shared>, id: String, remote_port: u16, local_port: Option<u16>) -> Result<(), String> {
    state.set_pin(&id, remote_port, local_port)
}

#[tauri::command]
pub fn set_label_filter(state: State<'_, Shared>, labels: Vec<String>) -> Result<(), String> {
    state.set_label_filter(labels)
}

#[tauri::command]
pub async fn start_login(state: State<'_, Shared>, provider: Option<LoginProvider>, device_code: Option<bool>) -> Result<(), String> {
    state.inner().clone().start_login(provider.unwrap_or_default(), device_code.unwrap_or(false)).await
}

#[tauri::command]
pub fn cancel_login(state: State<'_, Shared>) {
    state.cancel_login();
}

#[tauri::command]
pub async fn logout(state: State<'_, Shared>) -> Result<(), String> {
    state.inner().clone().logout().await
}

/// `hotkey: null` disables the global shortcut.
#[tauri::command]
pub fn set_hotkey(app: AppHandle, state: State<'_, Shared>, hotkey: Option<String>) -> Result<(), String> {
    let hotkey = hotkey.map(|h| h.trim().to_string()).filter(|h| !h.is_empty());
    crate::hotkey::apply(&app, hotkey.as_deref())?;
    state.settings.lock().unwrap().hotkey = hotkey;
    *state.hotkey_error.lock().unwrap() = None;
    state.save_settings()?;
    state.publish();
    Ok(())
}

#[tauri::command]
pub async fn stop_external(state: State<'_, Shared>, id: String) -> Result<(), String> {
    state.inner().clone().stop_external(&id).await
}

/// Stops an external connection and reconnects it under Tunnel Bar's management.
#[tauri::command]
pub async fn adopt_external(state: State<'_, Shared>, id: String) -> Result<(), String> {
    let s = state.inner().clone();
    s.stop_external(&id).await?;
    s.connect(&id).await
}

#[tauri::command]
pub fn update_tunnel_settings(state: State<'_, Shared>, id: String, settings: TunnelSettings) -> Result<(), String> {
    {
        let mut s = state.settings.lock().unwrap();
        s.tunnels.insert(id, settings);
    }
    state.save_settings()?;
    state.sync_pins(None);
    state.publish();
    Ok(())
}

#[tauri::command]
pub async fn update_app_settings(
    app: AppHandle,
    state: State<'_, Shared>,
    launch_at_login: bool,
    notifications: bool,
    cli_path: Option<String>,
) -> Result<(), String> {
    let cli_changed = {
        let mut s = state.settings.lock().unwrap();
        s.launch_at_login = launch_at_login;
        s.notifications = notifications;
        let new = cli_path.filter(|p| !p.trim().is_empty());
        let changed = s.cli_path != new;
        s.cli_path = new;
        changed
    };
    state.save_settings()?;
    apply_autostart(&app, launch_at_login)?;
    if cli_changed {
        state.inner().clone().refresh().await;
    } else {
        state.publish();
    }
    Ok(())
}

pub fn apply_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let al = app.autolaunch();
    let is = al.is_enabled().unwrap_or(false);
    match (enabled, is) {
        (true, false) => al.enable().map_err(|e| e.to_string()),
        (false, true) => al.disable().map_err(|e| e.to_string()),
        _ => Ok(()),
    }
}

#[tauri::command]
pub fn get_logs(state: State<'_, Shared>, id: String) -> Vec<LogLine> {
    state.logs.get(&id)
}

#[tauri::command]
pub fn clear_logs(state: State<'_, Shared>, id: String) {
    state.logs.clear(&id);
}

#[tauri::command]
pub fn open_log_dir(app: AppHandle, state: State<'_, Shared>) -> Result<(), String> {
    app.opener().open_path(state.logs.dir().display().to_string(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ssh_command(state: State<'_, Shared>, id: String, local_port: u16, remote_port: u16) -> String {
    let local_port = pinned_or(&state, &id, local_port, remote_port);
    let alias = resolve_alias(&state, &id, local_port, remote_port);
    ssh::ssh_command(alias.as_deref(), local_port).join(" ")
}

/// A pinned local port is stable across reconnects, so SSH should use it over the connect port.
fn pinned_or(state: &Shared, id: &str, local_port: u16, remote_port: u16) -> u16 {
    let s = state.settings.lock().unwrap();
    s.tunnels.get(id).and_then(|t| t.pinned_ports.get(&remote_port).copied()).unwrap_or(local_port)
}

fn resolve_alias(state: &Shared, id: &str, local_port: u16, remote_port: u16) -> Option<String> {
    let configured = state.settings.lock().unwrap().tunnels.get(id).and_then(|t| t.ssh_host.clone());
    configured
        .filter(|a| !a.trim().is_empty())
        .or_else(|| ssh::find_alias(&ssh::read_user_config(), local_port, remote_port))
}

#[tauri::command]
pub fn open_ssh(state: State<'_, Shared>, id: String, local_port: u16, remote_port: u16) -> Result<(), String> {
    let local_port = pinned_or(&state, &id, local_port, remote_port);
    let alias = resolve_alias(&state, &id, local_port, remote_port);
    ssh::open_terminal(&ssh::ssh_command(alias.as_deref(), local_port))
}

#[tauri::command]
pub async fn open_login(state: State<'_, Shared>) -> Result<(), String> {
    let cli = state.cli_path().await.ok_or("devtunnel CLI not found")?;
    ssh::open_terminal(&[cli.display().to_string(), "user".into(), "login".into()])
}

#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("only https URLs are allowed".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    crate::tray::quit(&app);
}
