use crate::model::Settings;
use std::path::PathBuf;

pub const APP_ID: &str = "me.svv.tunnelbar";

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join(APP_ID)
}

pub fn log_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        return home.join("Library/Logs/TunnelBar");
    }
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("TunnelBar").join("logs")
}

pub fn pidfile_path() -> PathBuf {
    config_dir().join("children.json")
}

pub fn hosts_pidfile_path() -> PathBuf {
    config_dir().join("hosts.json")
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn load() -> Settings {
    load_from(&settings_path())
}

pub fn load_from(path: &std::path::Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(s: &Settings) -> Result<(), String> {
    save_to(&settings_path(), s)
}

pub fn save_to(path: &std::path::Path, s: &Settings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TunnelSettings;

    #[test]
    fn round_trips_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        let d = load_from(&p);
        assert!(d.launch_at_login && d.notifications && !d.first_run_done);
        let mut s = Settings::default();
        s.tunnels.insert(
            "a.usw3".into(),
            TunnelSettings { nickname: Some("Dev".into()), auto_connect: true, auto_host: true, ssh_host: None, pinned_ports: Default::default() },
        );
        save_to(&p, &s).unwrap();
        assert_eq!(load_from(&p), s);
        // Unknown/missing fields tolerated.
        std::fs::write(&p, r#"{"notifications":false}"#).unwrap();
        let l = load_from(&p);
        assert!(!l.notifications && l.launch_at_login);
    }
}
