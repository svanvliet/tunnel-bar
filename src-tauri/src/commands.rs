use crate::app::Shared;
use crate::model::{LogLine, Snapshot, TunnelSettings};
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
pub async fn stop_external(state: State<'_, Shared>, id: String) -> Result<(), String> {
    state.inner().clone().stop_external(&id).await
}

/// Stops an external connection and reconnects it under TunnelBar's management.
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
    let alias = resolve_alias(&state, &id, local_port, remote_port);
    ssh::ssh_command(alias.as_deref(), local_port).join(" ")
}

fn resolve_alias(state: &Shared, id: &str, local_port: u16, remote_port: u16) -> Option<String> {
    let configured = state.settings.lock().unwrap().tunnels.get(id).and_then(|t| t.ssh_host.clone());
    configured
        .filter(|a| !a.trim().is_empty())
        .or_else(|| ssh::find_alias(&ssh::read_user_config(), local_port, remote_port))
}

#[tauri::command]
pub fn open_ssh(state: State<'_, Shared>, id: String, local_port: u16, remote_port: u16) -> Result<(), String> {
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
