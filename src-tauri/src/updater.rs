//! Self-update from GitHub releases: periodic checks, background download, restart to install.

use crate::app::Shared;
use crate::model::{UpdateState, UpdateStatus};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::{Update, UpdaterExt};

/// Background checks run at most this often (wall clock, so sleep doesn't stretch it).
const CHECK_INTERVAL_HOURS: i64 = 6;
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_secs(15 * 60);
/// Build-time endpoint override for local update tests; `scripts/package-macos.sh` refuses to build with it set.
const ENDPOINT_OVERRIDE: Option<&str> = option_env!("TUNNELBAR_UPDATE_ENDPOINT");

/// Development builds (`tauri dev`) aren't installed bundles, so they never update unless pointed at a test endpoint.
pub fn enabled() -> bool {
    !cfg!(debug_assertions) || ENDPOINT_OVERRIDE.is_some()
}

#[derive(Default)]
pub struct Updates {
    state: Mutex<UpdateState>,
    ready: Mutex<Option<(Update, Vec<u8>)>>,
    busy: tokio::sync::Mutex<()>,
    last_attempt: Mutex<Option<chrono::DateTime<chrono::Utc>>>,
}

impl Updates {
    pub fn view(&self, current_version: &str) -> UpdateState {
        let mut s = self.state.lock().unwrap().clone();
        s.current_version = current_version.to_string();
        s.enabled = enabled();
        s
    }

    fn set(&self, f: impl FnOnce(&mut UpdateState)) {
        f(&mut self.state.lock().unwrap());
    }

    fn status(&self) -> UpdateStatus {
        self.state.lock().unwrap().status
    }
}

/// Checks on launch and then every few hours.
pub fn start(state: Shared) {
    if !enabled() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            let due = state
                .updates
                .last_attempt
                .lock()
                .unwrap()
                .is_none_or(|t| chrono::Utc::now() - t >= chrono::Duration::hours(CHECK_INTERVAL_HOURS));
            if due {
                check(&state, false).await;
            }
            tokio::time::sleep(POLL).await;
        }
    });
}

fn log(state: &Shared, text: &str) {
    state.logs.push("_app", "app", text);
}

fn updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    let mut b = app.updater_builder().timeout(Duration::from_secs(60));
    if let Some(url) = ENDPOINT_OVERRIDE {
        b = b.endpoints(vec![url.parse().map_err(|e| format!("bad update endpoint: {e}"))?]).map_err(|e| e.to_string())?;
    }
    let handle = app.clone();
    // Windows only: the installer is launched and the process exits without running RunEvent::Exit.
    b = b.on_before_exit(move || {
        if let Some(s) = handle.try_state::<Shared>() {
            s.save_resume();
            s.shutdown();
        }
    });
    b.build().map_err(|e| e.to_string())
}

/// Turns transport errors into something readable; the full error is logged.
pub fn friendly_error(e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("error sending request") || l.contains("dns") || l.contains("connect") || l.contains("timed out") {
        "Couldn’t reach the update server. Check your internet connection.".into()
    } else if l.contains("signature") {
        "The downloaded update failed signature verification and was discarded.".into()
    } else if l.contains("valid release json") {
        "No update information was found for the latest release.".into()
    } else {
        e.to_string()
    }
}

/// Checks for an update and downloads it in the background. Background failures are only logged;
/// a manual check reports them in the update state.
pub async fn check(state: &Shared, manual: bool) {
    if !enabled() {
        if manual {
            state.updates.set(|s| {
                s.status = UpdateStatus::Error;
                s.error = Some("Updates are disabled in development builds.".into());
            });
            state.publish();
        }
        return;
    }
    let Ok(_guard) = state.updates.busy.try_lock() else { return };
    let before = state.updates.status();
    if matches!(before, UpdateStatus::Ready | UpdateStatus::Installing) {
        return;
    }
    *state.updates.last_attempt.lock().unwrap() = Some(chrono::Utc::now());
    state.updates.set(|s| {
        s.status = UpdateStatus::Checking;
        s.error = None;
    });
    state.publish();
    let fail = |e: String| {
        log(state, &format!("Update check failed: {e}"));
        state.updates.set(|s| {
            s.progress = None;
            if manual {
                s.status = UpdateStatus::Error;
                s.error = Some(friendly_error(&e));
            } else {
                s.status = if before == UpdateStatus::Error { UpdateStatus::Idle } else { before };
                s.version = None;
            }
        });
        state.publish();
    };
    let found = match updater(&state.app) {
        Ok(u) => u.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    let update = match found {
        Ok(None) => {
            state.updates.set(|s| {
                s.status = UpdateStatus::UpToDate;
                s.version = None;
                s.notes = None;
                s.last_checked = Some(chrono::Utc::now().to_rfc3339());
            });
            state.publish();
            return;
        }
        Ok(Some(u)) => u,
        Err(e) => return fail(e),
    };
    log(state, &format!("Downloading Tunnel Bar {}", update.version));
    state.updates.set(|s| {
        s.status = UpdateStatus::Downloading;
        s.version = Some(update.version.clone());
        s.notes = update.body.clone().filter(|n| !n.trim().is_empty());
        s.progress = Some(0.0);
        s.last_checked = Some(chrono::Utc::now().to_rfc3339());
    });
    state.publish();
    let mut received: u64 = 0;
    let mut published = Instant::now();
    let bytes = update
        .download(
            |chunk, total| {
                received += chunk as u64;
                if let Some(total) = total.filter(|t| *t > 0) {
                    state.updates.set(|s| s.progress = Some((received as f64 / total as f64).min(1.0)));
                    if published.elapsed() > Duration::from_millis(500) {
                        published = Instant::now();
                        state.publish();
                    }
                }
            },
            || {},
        )
        .await;
    match bytes {
        Ok(bytes) => {
            log(state, &format!("Tunnel Bar {} downloaded and verified; restart to install", update.version));
            let version = update.version.clone();
            *state.updates.ready.lock().unwrap() = Some((update, bytes));
            state.updates.set(|s| {
                s.status = UpdateStatus::Ready;
                s.progress = None;
            });
            state.publish();
            // Lets the local end-to-end update test exercise install + relaunch without UI automation.
            if crate::settings::TEST_PROFILE.is_some() && std::env::var_os("TUNNELBAR_TEST_AUTO_INSTALL").is_some() {
                let st = state.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = install(&st).await;
                });
                return;
            }
            if state.settings.lock().unwrap().notifications {
                let _ = state
                    .app
                    .notification()
                    .builder()
                    .title(format!("Tunnel Bar {version} is ready"))
                    .body("Restart Tunnel Bar to update when you’re ready.")
                    .show();
            }
        }
        Err(e) => fail(e.to_string()),
    }
}

/// Installs the downloaded update and restarts. Active connections and hosts stop; the ones that
/// were running are restored by the new version.
pub async fn install(state: &Shared) -> Result<(), String> {
    let (update, bytes) = state.updates.ready.lock().unwrap().clone().ok_or("No update is ready to install.")?;
    state.updates.set(|s| {
        s.status = UpdateStatus::Installing;
        s.error = None;
    });
    state.publish();
    log(state, &format!("Installing Tunnel Bar {}", update.version));
    let r = tauri::async_runtime::spawn_blocking(move || update.install(&bytes).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    if let Err(e) = r {
        log(state, &format!("Update install failed: {e}"));
        state.updates.set(|s| {
            s.status = UpdateStatus::Ready;
            s.error = Some(format!("Couldn’t install the update: {e}"));
        });
        state.publish();
        return Err(format!("Couldn’t install the update: {e}"));
    }
    state.save_resume();
    // Runs RunEvent::Exit (which stops every child) before relaunching the new bundle.
    state.app.request_restart();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_errors() {
        assert!(friendly_error("error sending request for url (https://example.invalid/latest.json)").contains("internet"));
        assert!(friendly_error("signature verification failed").contains("signature"));
        assert!(friendly_error("Could not fetch a valid release JSON from the remote").contains("No update information"));
        assert_eq!(friendly_error("something else"), "something else");
    }

    #[test]
    fn view_reports_version_and_availability() {
        let u = Updates::default();
        let v = u.view("1.2.3");
        assert_eq!(v.current_version, "1.2.3");
        assert_eq!(v.status, UpdateStatus::Idle);
        assert_eq!(v.enabled, enabled());
    }
}
