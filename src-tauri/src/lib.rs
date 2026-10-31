mod app;
mod cli;
mod commands;
mod external;
mod icon;
mod logs;
mod model;
mod parser;
mod settings;
mod ssh;
mod supervisor;
mod tray;

use tauri::{Manager, RunEvent, WindowEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_window(app)))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let state = app::AppState::new(app.handle().clone());
            app.manage(state.clone());
            tray::create(app.handle())?;

            let first_run = {
                let mut s = state.settings.lock().unwrap();
                let first = !s.first_run_done;
                s.first_run_done = true;
                first
            };
            if first_run {
                let _ = state.save_settings();
            }
            let launch = state.settings.lock().unwrap().launch_at_login;
            // Only reconcile autostart in release builds so `tauri dev` doesn't register the debug binary.
            if !cfg!(debug_assertions)
                && let Err(e) = commands::apply_autostart(app.handle(), launch) {
                    state.logs.push("_app", "app", &format!("autostart: {e}"));
                }

            if let Some(w) = app.get_webview_window("main") {
                let w2 = w.clone();
                w.on_window_event(move |e| match e {
                    WindowEvent::Focused(false) if tray::should_hide_on_blur() => {
                        let _ = w2.hide();
                    }
                    WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        let _ = w2.hide();
                    }
                    _ => {}
                });
            }
            #[cfg(unix)]
            {
                // Treat SIGTERM/SIGINT/SIGHUP like Quit so children are stopped instead of orphaned.
                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    use tokio::signal::unix::{signal, SignalKind};
                    let (Ok(mut term), Ok(mut int), Ok(mut hup)) =
                        (signal(SignalKind::terminate()), signal(SignalKind::interrupt()), signal(SignalKind::hangup()))
                    else {
                        return;
                    };
                    tokio::select! {
                        _ = term.recv() => {}
                        _ = int.recv() => {}
                        _ = hup.recv() => {}
                    }
                    h.exit(0);
                });
            }
            app::start_background(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::refresh,
            commands::connect,
            commands::disconnect,
            commands::stop_external,
            commands::adopt_external,
            commands::update_tunnel_settings,
            commands::update_app_settings,
            commands::get_logs,
            commands::clear_logs,
            commands::open_log_dir,
            commands::ssh_command,
            commands::open_ssh,
            commands::open_login,
            commands::open_url,
            commands::quit,
            commands::host,
            commands::stop_host,
            commands::expose_port,
            commands::set_anonymous,
            commands::delete_tunnel,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| match event {
        // Keep running in the tray when the popover closes.
        RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => tray::show_window(handle),
        RunEvent::Exit => {
            if let Some(s) = handle.try_state::<app::Shared>() {
                s.shutdown();
            }
        }
        _ => {}
    });
}
