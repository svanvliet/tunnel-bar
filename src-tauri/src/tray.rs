use crate::app::{self, AppState};
use crate::icon::{self, TrayState};
use crate::model::{ConnState, Snapshot};
use std::sync::Mutex;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_positioner::{Position, WindowExt};

const TRAY_ID: &str = "main";
static LAST: Mutex<Option<(TrayState, String)>> = Mutex::new(None);

fn image(state: TrayState) -> Image<'static> {
    icon::image(state)
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let menu = Menu::with_items(app, &[&MenuItem::with_id(app, "open", "Open Tunnel Bar…", true, None::<&str>)?])?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(image(TrayState::Idle))
        .icon_as_template(TrayState::Idle.is_template())
        .tooltip("Tunnel Bar")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                toggle_window(tray.app_handle());
            }
        })
        .on_menu_event(|app, ev| handle_menu(app, ev.id().as_ref()))
        .build(app)?;
    Ok(())
}

fn state_label(s: &ConnState) -> String {
    match s {
        ConnState::Disconnected => "Disconnected".into(),
        ConnState::Connecting => "Connecting…".into(),
        ConnState::Connected => "Connected".into(),
        ConnState::Reconnecting => "Reconnecting…".into(),
        ConnState::Backoff { retry_in_secs, .. } => format!("Retrying in {retry_in_secs}s"),
        ConnState::AuthRequired { .. } => "Sign-in required".into(),
        ConnState::External { pid } => format!("Connected outside app (PID {pid})"),
    }
}

fn menu_signature(snap: &Snapshot) -> String {
    snap.tunnels
        .iter()
        .map(|t| format!("{}|{}|{}|{:?}|{}", t.id, t.display_name, state_label(&t.state), t.forwards, state_label(&t.host_state)))
        .collect::<Vec<_>>()
        .join(";")
}

pub fn update(app: &AppHandle, snap: &Snapshot) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let ts = AppState::tray_state(snap);
    let sig = menu_signature(snap);
    let mut last = LAST.lock().unwrap();
    if last.as_ref().map(|(s, _)| *s) != Some(ts) {
        let _ = tray.set_icon(Some(image(ts)));
        let _ = tray.set_icon_as_template(ts.is_template());
    }
    if last.as_ref().map(|(_, m)| m.as_str()) != Some(sig.as_str()) {
        let up = snap.tunnels.iter().filter(|t| t.state.is_up()).count();
        let _ = tray.set_tooltip(Some(format!("Tunnel Bar — {up} of {} connected", snap.tunnels.len())));
        if let Ok(menu) = build_menu(app, snap) {
            let _ = tray.set_menu(Some(menu));
        }
    }
    *last = Some((ts, sig));
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, snap: &Snapshot) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    if snap.tunnels.is_empty() {
        menu.append(&MenuItem::with_id(app, "none", "No tunnels", false, None::<&str>)?)?;
    }
    for t in &snap.tunnels {
        let dot = match (&t.state, &t.host_state) {
            (s, h) if s.is_problem() || h.is_problem() => "⚠︎",
            (s, h) if s.is_transitional() || h.is_transitional() => "◐",
            (s, h) if s.is_up() || h.is_up() => "●",
            _ => "○",
        };
        let ports = t.forwards.iter().map(|f| format!(":{}", f.local_port)).collect::<Vec<_>>().join(" ");
        let hosting = match &t.host_state {
            ConnState::Disconnected => String::new(),
            ConnState::Connected | ConnState::External { .. } => "  · hosting".into(),
            s => format!("  · host: {}", state_label(s)),
        };
        let conn = if matches!(t.state, ConnState::Disconnected) && !hosting.is_empty() { "" } else { &state_label(&t.state) };
        let label = format!(
            "{dot} {}{}{conn}{}{hosting}",
            t.display_name,
            if conn.is_empty() { "" } else { " — " },
            if ports.is_empty() { String::new() } else { format!("  {ports}") }
        );
        let enabled = !matches!(t.state, ConnState::External { .. });
        // Tunnels Tunnel Bar exposed are for hosting; clicking toggles the host instead of a client connection.
        let (action, enabled) = if t.created_by_app {
            ("hosttoggle", !matches!(t.host_state, ConnState::External { .. }))
        } else {
            ("toggle", enabled)
        };
        menu.append(&MenuItem::with_id(app, format!("{action}:{}", t.id), label, enabled, None::<&str>)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "open", "Open Tunnel Bar…", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "refresh", "Refresh", true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit Tunnel Bar", true, Some("CmdOrCtrl+Q"))?)?;
    Ok(menu)
}

fn handle_menu(app: &AppHandle, id: &str) {
    let st = app::state(app);
    match id {
        "open" => show_window(app),
        "refresh" => {
            tauri::async_runtime::spawn(async move { st.refresh().await });
        }
        "quit" => quit(app),
        _ => {
            if let Some(tid) = id.strip_prefix("hosttoggle:") {
                let tid = tid.to_string();
                tauri::async_runtime::spawn(async move {
                    if st.hoster.is_active(&tid) {
                        st.stop_host(&tid);
                    } else if let Err(e) = st.host(&tid).await {
                        st.logs.push(&tid, "app", &e);
                    }
                });
            } else if let Some(tid) = id.strip_prefix("toggle:") {
                let tid = tid.to_string();
                tauri::async_runtime::spawn(async move {
                    if st.supervisor.is_active(&tid) {
                        st.supervisor.disconnect(&tid);
                    } else if let Err(e) = st.connect(&tid).await {
                        st.logs.push(&tid, "app", &e);
                    }
                });
            }
        }
    }
}

pub fn quit(app: &AppHandle) {
    app::state(app).shutdown();
    app.exit(0);
}

pub fn show_window(app: &AppHandle) {
    #[cfg(debug_assertions)]
    eprintln!("[tray] show_window");
    if let Some(w) = app.get_webview_window("main") {
        #[cfg(target_os = "windows")]
        let pos = Position::TrayBottomCenter;
        #[cfg(not(target_os = "windows"))]
        let pos = Position::TrayCenter;
        if w.as_ref().window().move_window(pos).is_err() {
            let _ = w.move_window(Position::TopRight);
        }
        *SHOWN_AT.lock().unwrap() = Some(std::time::Instant::now());
        let _r = (w.show(), w.set_focus());
        #[cfg(debug_assertions)]
        eprintln!("[tray] shown {_r:?} visible={:?} pos={:?}", w.is_visible(), w.outer_position());
    }
}

pub fn toggle_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            show_window(app);
        }
    }
}

static SHOWN_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// Hide the popover when it loses focus, except right after showing (macOS can emit a
/// spurious blur while activating an accessory app) or when pinned via `TUNNELBAR_PIN`.
pub fn should_hide_on_blur() -> bool {
    if std::env::var_os("TUNNELBAR_PIN").is_some() {
        return false;
    }
    SHOWN_AT.lock().unwrap().is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(400))
}
