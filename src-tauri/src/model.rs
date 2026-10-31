use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Tunnel summary as returned by `devtunnel list -j`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TunnelSummary {
    pub tunnel_id: String,
    pub name: String,
    pub host_connections: u32,
    pub labels: Vec<String>,
    pub port_count: u32,
    pub tunnel_expiration: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TunnelPort {
    pub port_number: u16,
    pub protocol: String,
    pub port_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AccessEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub subjects: Vec<String>,
    pub scopes: Vec<String>,
    #[serde(alias = "deny")]
    pub is_deny: bool,
}

impl AccessEntry {
    pub fn allows_anonymous(&self) -> bool {
        self.kind.eq_ignore_ascii_case("anonymous")
            && !self.is_deny
            && (self.scopes.is_empty() || self.scopes.iter().any(|s| s == "connect"))
    }
}

/// Tunnel details as returned by `devtunnel show <id> -j`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TunnelDetails {
    pub tunnel_id: String,
    pub host_connections: u32,
    pub client_connections: u32,
    pub labels: Vec<String>,
    pub description: String,
    pub ports: Vec<TunnelPort>,
    pub access_control: Vec<AccessEntry>,
    pub tunnel_expiration: String,
}

/// A local listener forwarding to a remote tunnel port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct Forward {
    pub local_port: u16,
    pub remote_port: u16,
}

/// Which `devtunnel` subcommand a supervised process runs.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Connect,
    Host,
}

impl Role {
    pub fn subcommand(self) -> &'static str {
        match self {
            Role::Connect => "connect",
            Role::Host => "host",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    /// Process exited unexpectedly; waiting before the next attempt.
    Backoff { attempt: u32, retry_in_secs: u64, message: String },
    AuthRequired { message: String },
    /// A `devtunnel connect`/`host` started outside this app.
    External { pid: u32 },
}

impl ConnState {
    pub fn is_up(&self) -> bool {
        matches!(self, ConnState::Connected | ConnState::External { .. })
    }
    pub fn is_problem(&self) -> bool {
        matches!(
            self,
            ConnState::Backoff { .. } | ConnState::AuthRequired { .. }
        )
    }
    pub fn is_transitional(&self) -> bool {
        matches!(self, ConnState::Connecting | ConnState::Reconnecting)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TunnelSettings {
    pub nickname: Option<String>,
    pub auto_connect: bool,
    /// Host this tunnel when Tunnel Bar starts.
    pub auto_host: bool,
    /// ~/.ssh/config Host alias to use for "Open SSH" (port is overridden with -p).
    pub ssh_host: Option<String>,
    /// Stable local ports: remote port -> local port served by Tunnel Bar's proxy while connected.
    pub pinned_ports: BTreeMap<u16, u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub cli_path: Option<String>,
    pub launch_at_login: bool,
    pub notifications: bool,
    pub first_run_done: bool,
    pub tunnels: BTreeMap<String, TunnelSettings>,
    /// Global shortcut (Tauri accelerator syntax) that toggles the popover; None disables it.
    pub hotkey: Option<String>,
    /// Show only tunnels carrying any of these labels (empty = all).
    pub label_filter: Vec<String>,
}

pub const DEFAULT_HOTKEY: &str = "CmdOrCtrl+Alt+Shift+T";

impl Default for Settings {
    fn default() -> Self {
        Self {
            cli_path: None,
            launch_at_login: true,
            notifications: true,
            first_run_done: false,
            tunnels: BTreeMap::new(),
            hotkey: Some(DEFAULT_HOTKEY.into()),
            label_filter: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub logged_in: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelView {
    pub id: String,
    pub display_name: String,
    pub summary: TunnelSummary,
    pub ports: Vec<TunnelPort>,
    pub state: ConnState,
    pub forwards: Vec<Forward>,
    pub settings: TunnelSettings,
    /// Tunnel is running but no longer listed on the account.
    pub orphaned: bool,
    pub host_state: ConnState,
    /// Ports published by our `devtunnel host`, with public URLs.
    pub hosted_ports: Vec<HostedPort>,
    pub anonymous: bool,
    /// Created by Tunnel Bar's "Expose local port" (label `tunnelbar`).
    pub created_by_app: bool,
    /// A host other than this app's is connected to the relay.
    pub hosted_elsewhere: bool,
    /// Tunnel lifetime from the service (e.g. "30 days" -> 720); None if unknown.
    pub expiration_hours: Option<u32>,
    /// Matches `Settings.label_filter` (always true when the filter is empty).
    pub matches_filter: bool,
    /// Stable-port proxies configured for this tunnel.
    pub pins: Vec<PinView>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PinView {
    pub remote_port: u16,
    pub local_port: u16,
    /// The proxy is listening on `local_port`.
    pub listening: bool,
    /// Current `devtunnel connect` local port the proxy forwards to.
    pub target_port: Option<u16>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PortSpec {
    pub port: u16,
    /// "http", "https" or "auto" (default).
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateTunnelRequest {
    /// Custom tunnel id; the CLI generates one when None.
    pub tunnel_id: Option<String>,
    pub description: String,
    /// Extra labels; `tunnelbar` is always added.
    pub labels: Vec<String>,
    /// e.g. "8h" or "30d" (1h..30d); service default (30 days) when None.
    pub expiration: Option<String>,
    pub ports: Vec<PortSpec>,
    pub anonymous: bool,
    /// Start hosting from this Mac after creating.
    pub host: bool,
    pub nickname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateTunnelRequest {
    pub description: Option<String>,
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub expiration: Option<String>,
}

/// What deleting a tunnel would require; lets the UI ask before calling `delete_tunnel`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeletePreflight {
    pub created_by_app: bool,
    /// Pass `confirm: true` (tunnel isn't labelled `tunnelbar`).
    pub needs_confirm: bool,
    /// Pass `allowHostedElsewhere: true` (another machine is hosting it).
    pub hosted_elsewhere: bool,
    pub hosting_here: bool,
    pub connected_here: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum LoginProvider {
    #[default]
    Microsoft,
    Github,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LoginState {
    /// `devtunnel user login` is running (browser flow, or waiting for the device code).
    Running { provider: LoginProvider },
    DeviceCode { provider: LoginProvider, url: String, code: String },
    Succeeded { message: String },
    Failed { message: String },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostedPort {
    pub port: u16,
    pub protocol: String,
    pub uri: String,
    pub inspect_uri: String,
    /// Something is accepting connections on localhost:port.
    pub listening: bool,
}

pub const APP_LABEL: &str = "tunnelbar";

/// "30 days" -> 720, "1 hours" -> 1, "12h" -> 12, "2d" -> 48.
pub fn parse_expiration_hours(s: &str) -> Option<u32> {
    let s = s.trim().to_ascii_lowercase();
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    let n: u32 = s[..split].parse().ok()?;
    match s[split..].trim() {
        "h" | "hour" | "hours" => Some(n),
        "d" | "day" | "days" => n.checked_mul(24),
        _ => None,
    }
}

/// Normalizes a user expiration ("8h", "30 d", "2 days") to the CLI form, enforcing 1h..30d.
pub fn normalize_expiration(s: &str) -> Result<String, String> {
    let hours = parse_expiration_hours(s).ok_or_else(|| format!("Invalid expiration “{s}”. Use hours or days, e.g. 8h or 30d."))?;
    if !(1..=720).contains(&hours) {
        return Err("Expiration must be between 1 hour and 30 days.".into());
    }
    Ok(if hours % 24 == 0 { format!("{}d", hours / 24) } else { format!("{hours}h") })
}

/// Service rule: `[\w-=]{1,50}`.
pub fn validate_label(l: &str) -> Result<(), String> {
    let ok = (1..=50).contains(&l.chars().count()) && l.chars().all(|c| c.is_alphanumeric() || "_-=".contains(c));
    if ok { Ok(()) } else { Err(format!("Invalid label “{l}”: use 1–50 letters, digits, _, - or =.")) }
}

pub fn validate_protocol(p: &str) -> Result<&'static str, String> {
    match p.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => Ok("auto"),
        "http" => Ok("http"),
        "https" => Ok("https"),
        other => Err(format!("Unsupported protocol {other}; use http, https or auto.")),
    }
}

/// True when `filter` is empty or the tunnel carries any of its labels.
pub fn matches_label_filter(labels: &[String], filter: &[String]) -> bool {
    filter.is_empty() || filter.iter().any(|f| labels.iter().any(|l| l.eq_ignore_ascii_case(f)))
}

/// "https://abc-3000.usw3.devtunnels.ms/" -> "https://abc-3000-inspect.usw3.devtunnels.ms/"
pub fn inspect_uri(port_uri: &str) -> String {
    let Some(rest) = port_uri.strip_prefix("https://") else { return String::new() };
    match rest.split_once('.') {
        Some((host, domain)) if !host.is_empty() => format!("https://{host}-inspect.{domain}"),
        _ => String::new(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub cli_path: Option<String>,
    pub cli_version: Option<String>,
    pub auth: Option<AuthStatus>,
    pub tunnels: Vec<TunnelView>,
    pub settings: Settings,
    pub last_refresh: Option<String>,
    pub refresh_error: Option<String>,
    pub refreshing: bool,
    pub log_dir: String,
    /// Sorted, de-duplicated labels across all tunnels (for the label filter).
    pub all_labels: Vec<String>,
    /// In-app sign-in progress; None when no login has been started this session.
    pub login: Option<LoginState>,
    /// Why the global hotkey couldn't be registered.
    pub hotkey_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub ts: String,
    pub stream: String,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_inspect_uri() {
        assert_eq!(inspect_uri("https://abc-18765.usw3.devtunnels.ms/"), "https://abc-18765-inspect.usw3.devtunnels.ms/");
        assert_eq!(inspect_uri(""), "");
    }

    #[test]
    fn parses_and_normalizes_expiration() {
        assert_eq!(parse_expiration_hours("30 days"), Some(720));
        assert_eq!(parse_expiration_hours("1 hours"), Some(1));
        assert_eq!(parse_expiration_hours("2d"), Some(48));
        assert_eq!(parse_expiration_hours(""), None);
        assert_eq!(parse_expiration_hours("soon"), None);
        assert_eq!(normalize_expiration("48h").unwrap(), "2d");
        assert_eq!(normalize_expiration(" 8 H ").unwrap(), "8h");
        assert!(normalize_expiration("31d").is_err());
        assert!(normalize_expiration("0h").is_err());
    }

    #[test]
    fn validates_inputs() {
        assert!(validate_label("team-a=1_x").is_ok());
        assert!(validate_label("bad label").is_err());
        assert!(validate_label("").is_err());


        assert_eq!(validate_protocol("HTTP").unwrap(), "http");
        assert_eq!(validate_protocol("").unwrap(), "auto");
        assert!(validate_protocol("tcp").is_err());
    }

    #[test]
    fn filters_by_label() {
        let l = vec!["tunnelbar".to_string(), "Work".to_string()];
        assert!(matches_label_filter(&l, &[]));
        assert!(matches_label_filter(&l, &["work".into()]));
        assert!(!matches_label_filter(&l, &["home".into()]));
        assert!(!matches_label_filter(&[], &["home".into()]));
    }

    #[test]
    fn settings_default_and_backcompat() {
        let s: Settings = serde_json::from_str(r#"{"launchAtLogin":false,"tunnels":{"a":{"autoConnect":true}}}"#).unwrap();
        assert_eq!(s.hotkey.as_deref(), Some(DEFAULT_HOTKEY));
        assert!(s.label_filter.is_empty());
        assert!(s.tunnels["a"].pinned_ports.is_empty());
        let s: Settings = serde_json::from_str(r#"{"hotkey":null,"tunnels":{"a":{"pinnedPorts":{"2222":2222}}}}"#).unwrap();
        assert_eq!(s.hotkey, None);
        assert_eq!(s.tunnels["a"].pinned_ports[&2222], 2222);
    }

    #[test]
    fn detects_anonymous_access() {
        let e: AccessEntry = serde_json::from_str(r#"{"type":"Anonymous","subjects":[],"scopes":["connect"]}"#).unwrap();
        assert!(e.allows_anonymous());
        let d: AccessEntry =
            serde_json::from_str(r#"{"type":"Anonymous","subjects":[],"scopes":["connect"],"isDeny":true}"#).unwrap();
        assert!(!d.allows_anonymous());
    }
}
