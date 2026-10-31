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
        #[cfg(windows)]
        position_flyout(app, &w);
        #[cfg(not(windows))]
        if w.as_ref().window().move_window(Position::TrayCenter).is_err() {
            let _ = w.move_window(Position::TopRight);
        }
        *SHOWN_AT.lock().unwrap() = Some(std::time::Instant::now());
        let _r = (w.show(), w.set_focus());
        #[cfg(debug_assertions)]
        eprintln!("[tray] shown {_r:?} visible={:?} pos={:?}", w.is_visible(), w.outer_position());
    }
}

/// Places the flyout against the taskbar next to the tray icon, like the built-in Windows flyouts.
#[cfg(windows)]
fn position_flyout(app: &AppHandle, w: &tauri::WebviewWindow) {
    let tray_rect = app.tray_by_id(TRAY_ID).and_then(|t| t.rect().ok().flatten());
    let (Ok(size), Some((tray, monitor))) = (
        w.outer_size(),
        tray_rect.and_then(|r| {
            let scale = w.scale_factor().unwrap_or(1.0);
            let p = r.position.to_physical::<i32>(scale);
            let s = r.size.to_physical::<u32>(scale);
            let m = w.monitor_from_point(p.x as f64 + 1.0, p.y as f64 + 1.0).ok().flatten()?;
            Some((Rect { x: p.x, y: p.y, w: s.width as i32, h: s.height as i32 }, m))
        }),
    ) else {
        let _ = w.move_window(Position::BottomRight);
        return;
    };
    let mr = Rect {
        x: monitor.position().x,
        y: monitor.position().y,
        w: monitor.size().width as i32,
        h: monitor.size().height as i32,
    };
    let wa = monitor.work_area();
    let work = Rect { x: wa.position.x, y: wa.position.y, w: wa.size.width as i32, h: wa.size.height as i32 };
    let margin = (12.0 * monitor.scale_factor()) as i32;
    let (x, y) = flyout_position(tray, (size.width as i32, size.height as i32), mr, work, margin);
    let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// Top-left for a `win`-sized flyout anchored to the tray icon on whichever monitor edge holds the taskbar.
/// The edge is inferred from where the work area is inset; with an auto-hiding taskbar, from the nearest edge.
#[cfg_attr(not(windows), allow(dead_code))]
fn flyout_position(tray: Rect, win: (i32, i32), monitor: Rect, work: Rect, margin: i32) -> (i32, i32) {
    let (ww, wh) = win;
    let (cx, cy) = (tray.x + tray.w / 2, tray.y + tray.h / 2);
    let clamp = |v: i32, lo: i32, hi: i32| v.max(lo).min(hi.max(lo));
    let insets = [
        (work.y + work.h < monitor.y + monitor.h, monitor.y + monitor.h - cy), // bottom
        (work.y > monitor.y, cy - monitor.y),                                  // top
        (work.x > monitor.x, cx - monitor.x),                                  // left
        (work.x + work.w < monitor.x + monitor.w, monitor.x + monitor.w - cx), // right
    ];
    let edge = insets
        .iter()
        .position(|(inset, _)| *inset)
        .unwrap_or_else(|| (0..4).min_by_key(|&i| insets[i].1).unwrap_or(0));
    let hx = clamp(cx - ww / 2, work.x + margin, work.x + work.w - ww - margin);
    let vy = clamp(cy - wh / 2, work.y + margin, work.y + work.h - wh - margin);
    match edge {
        0 => (hx, work.y + work.h - wh - margin),
        1 => (hx, work.y + margin),
        2 => (work.x + margin, vy),
        _ => (work.x + work.w - ww - margin, vy),
    }
}

/// Hides the popover after it loses focus, remembering when so a tray click that caused the blur
/// doesn't immediately re-open it.
pub fn hide_window(w: &tauri::WebviewWindow) {
    *HIDDEN_AT.lock().unwrap() = Some(std::time::Instant::now());
    let _ = w.hide();
}

static HIDDEN_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

pub fn toggle_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            // On Windows, clicking the tray icon blurs (and hides) the flyout before the click arrives.
            #[cfg(windows)]
            if HIDDEN_AT.lock().unwrap().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(300)) {
                return;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };

    #[test]
    fn flyout_sits_against_the_taskbar() {
        let win = (400, 580);
        // Bottom taskbar (48px), tray near the right.
        let work = Rect { x: 0, y: 0, w: 1920, h: 1032 };
        let tray = Rect { x: 1700, y: 1040, w: 32, h: 32 };
        assert_eq!(flyout_position(tray, win, MON, work, 12), (1508, 1032 - 580 - 12));
        // Tray at the far right clamps inside the work area.
        let tray = Rect { x: 1890, y: 1040, w: 24, h: 24 };
        assert_eq!(flyout_position(tray, win, MON, work, 12).0, 1920 - 400 - 12);
        // Top taskbar.
        let work = Rect { x: 0, y: 48, w: 1920, h: 1032 };
        let tray = Rect { x: 1700, y: 8, w: 32, h: 32 };
        assert_eq!(flyout_position(tray, win, MON, work, 12), (1508, 60));
        // Left taskbar.
        let work = Rect { x: 60, y: 0, w: 1860, h: 1080 };
        let tray = Rect { x: 10, y: 900, w: 32, h: 32 };
        assert_eq!(flyout_position(tray, win, MON, work, 12), (72, 1080 - 580 - 12));
        // Right taskbar, second monitor at negative x.
        let mon = Rect { x: -1920, y: 0, w: 1920, h: 1080 };
        let work = Rect { x: -1920, y: 0, w: 1860, h: 1080 };
        let tray = Rect { x: -40, y: 300, w: 32, h: 32 };
        assert_eq!(flyout_position(tray, win, mon, work, 12), (-60 - 400 - 12, 26));
        // Auto-hide bottom taskbar: work area == monitor, infer the edge from the tray.
        let tray = Rect { x: 1700, y: 1060, w: 20, h: 20 };
        assert_eq!(flyout_position(tray, win, MON, MON, 12).1, 1080 - 580 - 12);
    }
}
