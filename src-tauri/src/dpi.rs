use tauri::{LogicalSize, PhysicalSize};

fn reconciliation_size(last_scale: Option<f64>, scale: f64, logical: LogicalSize<f64>) -> Option<PhysicalSize<u32>> {
    if !scale.is_finite() || scale <= 0.0 || last_scale == Some(scale) {
        return None;
    }
    Some(logical.to_physical(scale))
}

#[cfg(windows)]
pub fn logical_size(app: &tauri::AppHandle) -> LogicalSize<f64> {
    let config = app.config().app.windows.iter().find(|w| w.label == "main").expect("main window configuration");
    LogicalSize::new(config.width, config.height)
}

#[cfg(windows)]
pub fn initialize(window: &tauri::WebviewWindow) {
    use std::sync::{Arc, Mutex};
    use tauri::{Manager, WindowEvent};

    #[derive(Default)]
    struct State {
        last_scale: Option<f64>,
        queued: bool,
    }

    fn schedule(window: tauri::WebviewWindow, state: Arc<Mutex<State>>) {
        {
            let mut state = state.lock().unwrap();
            if state.queued {
                return;
            }
            state.queued = true;
        }
        // run_on_main_thread executes inline when called on the event-loop thread.
        // Enqueue from a worker instead: Tao must finish WM_DPICHANGED (including its
        // suggested resize) before we query or reconcile the window.
        tauri::async_runtime::spawn_blocking(move || {
            let target = window.clone();
            let pending = state.clone();
            if let Err(e) = window.run_on_main_thread(move || {
                let scale = target.scale_factor();
                let logical = logical_size(target.app_handle());
                let size = {
                    let mut state = state.lock().unwrap();
                    state.queued = false;
                    let Ok(scale) = scale else { return };
                    let size = reconciliation_size(state.last_scale, scale, logical);
                    if size.is_some() {
                        state.last_scale = Some(scale);
                    }
                    size
                };
                let Some(size) = size else { return };
                // Never derive logical dimensions from an old monitor's physical size.
                if target.inner_size().ok() != Some(size)
                    && let Err(e) = target.set_size(logical)
                {
                    eprintln!("[dpi] window size: {e}");
                }
                // WebView2 handles monitor rasterization itself. Page zoom is independent:
                // 1.0 is neutral, not the monitor scale or its inverse.
                if let Err(e) = target.set_zoom(1.0) {
                    eprintln!("[dpi] page zoom: {e}");
                }
            }) {
                pending.lock().unwrap().queued = false;
                eprintln!("[dpi] scheduling: {e}");
            }
        });
    }

    let state = Arc::new(Mutex::new(State::default()));
    let target = window.clone();
    let pending = state.clone();
    window.on_window_event(move |event| {
        // Tao emits this on cross-monitor moves and display DPI changes. Ordinary
        // Moved/Resized events require no work, including those from our own resize.
        if matches!(event, WindowEvent::ScaleFactorChanged { .. }) {
            schedule(target.clone(), pending.clone());
        }
    });
    schedule(window.clone(), state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_and_monitor_round_trips_use_configured_logical_size() {
        let logical = LogicalSize::new(400.0, 580.0);
        let mut last = None;
        for (scale, expected) in [(1.0, (400, 580)), (1.5, (600, 870)), (3.0, (1200, 1740)), (1.0, (400, 580)), (3.0, (1200, 1740))] {
            assert_eq!(reconciliation_size(last, scale, logical), Some(PhysicalSize::new(expected.0, expected.1)));
            last = Some(scale);
            assert_eq!(reconciliation_size(last, scale, logical), None);
        }
    }

    #[test]
    fn invalid_scale_does_not_trigger_reconciliation() {
        let logical = LogicalSize::new(400.0, 580.0);
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(reconciliation_size(None, scale, logical), None);
        }
    }
}
