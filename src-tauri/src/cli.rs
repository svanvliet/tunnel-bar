//! Thin async wrapper around the `devtunnel` CLI.

use crate::model::{AuthStatus, TunnelDetails, TunnelSummary};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[cfg(windows)]
const EXE: &str = "devtunnel.exe";
#[cfg(not(windows))]
const EXE: &str = "devtunnel";

/// Locates the devtunnel binary. GUI apps on macOS don't inherit the shell PATH,
/// so we check an explicit override, PATH, well-known install dirs, then the login shell.
pub async fn find_cli(override_path: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = override_path.filter(|p| !p.trim().is_empty()) {
        let p = PathBuf::from(shellexpand_home(p.trim()));
        return p.is_file().then_some(p);
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join(EXE)).collect())
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join("bin").join(EXE));
        candidates.push(home.join(".local/bin").join(EXE));
    }
    #[cfg(not(windows))]
    {
        candidates.push("/opt/homebrew/bin/devtunnel".into());
        candidates.push("/usr/local/bin/devtunnel".into());
    }
    #[cfg(windows)]
    if let Some(local) = dirs::data_local_dir() {
        candidates.push(local.join("Microsoft/WinGet/Links").join(EXE));
    }
    if let Some(p) = candidates.into_iter().find(|p| p.is_file()) {
        return Some(p);
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let out = tokio::time::timeout(
            Duration::from_secs(5),
            Command::new(shell).args(["-lc", "command -v devtunnel"]).stdin(Stdio::null()).output(),
        )
        .await
        .ok()?
        .ok()?;
        let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn shellexpand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
        _ => p.to_string(),
    }
}

pub struct CliOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub async fn run(cli: &Path, args: &[&str], timeout: Duration) -> Result<CliOutput, String> {
    let mut cmd = Command::new(cli);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| format!("`devtunnel {}` timed out", args.join(" ")))?
        .map_err(|e| format!("failed to run devtunnel: {e}"))?;
    Ok(CliOutput {
        success: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

pub async fn version(cli: &Path) -> Option<String> {
    let out = run(cli, &["--version"], Duration::from_secs(15)).await.ok()?;
    out.stdout
        .lines()
        .find_map(|l| l.strip_prefix("Tunnel CLI version:").map(|v| v.trim().to_string()))
}

pub async fn auth_status(cli: &Path) -> Result<AuthStatus, String> {
    let out = run(cli, &["user", "show"], Duration::from_secs(30)).await?;
    Ok(parse_auth(&out.stdout, &out.stderr))
}

pub fn parse_auth(stdout: &str, stderr: &str) -> AuthStatus {
    let text = format!("{}\n{}", stdout.trim(), stderr.trim()).trim().to_string();
    let first = text.lines().next().unwrap_or("").trim().to_string();
    AuthStatus { logged_in: first.starts_with("Logged in as"), message: first }
}

#[derive(Deserialize)]
struct ListResponse {
    #[serde(default)]
    tunnels: Vec<TunnelSummary>,
}

#[derive(Deserialize)]
struct ShowResponse {
    tunnel: TunnelDetails,
}

/// The CLI may print non-JSON preamble lines (e.g. update notices); take from the first '{'.
fn json_slice(s: &str) -> &str {
    s.find('{').map(|i| &s[i..]).unwrap_or(s)
}

pub fn parse_list(stdout: &str) -> Result<Vec<TunnelSummary>, String> {
    serde_json::from_str::<ListResponse>(json_slice(stdout))
        .map(|r| r.tunnels)
        .map_err(|e| format!("could not parse `devtunnel list -j`: {e}"))
}

pub fn parse_show(stdout: &str) -> Result<TunnelDetails, String> {
    serde_json::from_str::<ShowResponse>(json_slice(stdout))
        .map(|r| r.tunnel)
        .map_err(|e| format!("could not parse `devtunnel show -j`: {e}"))
}

fn err_text(out: &CliOutput) -> String {
    let t = if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr };
    t.trim().lines().next().unwrap_or("devtunnel failed").to_string()
}

pub async fn list(cli: &Path) -> Result<Vec<TunnelSummary>, String> {
    let out = run(cli, &["list", "-j"], Duration::from_secs(45)).await?;
    if !out.success {
        return Err(err_text(&out));
    }
    parse_list(&out.stdout)
}

pub async fn show(cli: &Path, id: &str) -> Result<TunnelDetails, String> {
    let out = run(cli, &["show", id, "-j"], Duration::from_secs(45)).await?;
    if !out.success {
        return Err(err_text(&out));
    }
    parse_show(&out.stdout)
}

async fn ok(cli: &Path, args: &[&str]) -> Result<String, String> {
    let out = run(cli, args, Duration::from_secs(45)).await?;
    if !out.success {
        return Err(err_text(&out));
    }
    Ok(out.stdout)
}

#[derive(Deserialize)]
struct CreateResponse {
    tunnel: CreatedTunnel,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedTunnel {
    tunnel_id: String,
}

pub fn parse_create(stdout: &str) -> Result<String, String> {
    serde_json::from_str::<CreateResponse>(json_slice(stdout))
        .map(|r| r.tunnel.tunnel_id)
        .map_err(|e| format!("could not parse `devtunnel create -j`: {e}"))
}

/// Creates a tunnel with a CLI-generated id; returns "name.cluster".
pub async fn create(cli: &Path, labels: &[&str], description: &str) -> Result<String, String> {
    let mut args = vec!["create", "-j", "-d", description];
    for l in labels {
        args.extend(["-l", l]);
    }
    parse_create(&ok(cli, &args).await?)
}

pub async fn port_create(cli: &Path, id: &str, port: u16, protocol: &str) -> Result<(), String> {
    let port = port.to_string();
    ok(cli, &["port", "create", id, "-p", &port, "--protocol", protocol, "-j"]).await.map(|_| ())
}

pub async fn delete(cli: &Path, id: &str) -> Result<(), String> {
    ok(cli, &["delete", id, "-f"]).await.map(|_| ())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccessList {
    #[serde(default)]
    access_control_entries: Vec<crate::model::AccessEntry>,
}

pub fn parse_access_list(stdout: &str) -> Result<Vec<crate::model::AccessEntry>, String> {
    serde_json::from_str::<AccessList>(json_slice(stdout))
        .map(|r| r.access_control_entries)
        .map_err(|e| format!("could not parse `devtunnel access list -j`: {e}"))
}

/// Allows or revokes anonymous (no sign-in) connect access at the tunnel level.
pub async fn set_anonymous(cli: &Path, id: &str, allow: bool) -> Result<(), String> {
    if allow {
        return ok(cli, &["access", "create", id, "--anonymous", "-j"]).await.map(|_| ());
    }
    // Delete from the highest index down so earlier indexes stay valid.
    let entries = parse_access_list(&ok(cli, &["access", "list", id, "-j"]).await?)?;
    for i in (0..entries.len()).rev().filter(|&i| entries[i].kind.eq_ignore_ascii_case("anonymous")) {
        let idx = i.to_string();
        ok(cli, &["access", "delete", id, "-i", &idx, "-j"]).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_create_and_access() {
        let id = parse_create(r#"{"tunnel":{"tunnelId":"abc-123.usw3","accessControl":[]}}"#).unwrap();
        assert_eq!(id, "abc-123.usw3");
        let acl = parse_access_list(
            r#"{"accessControlEntries":[{"type":"Anonymous","subjects":[],"scopes":["connect"]}]}"#,
        )
        .unwrap();
        assert!(acl[0].allows_anonymous());
        assert!(parse_access_list(r#"{"accessControlEntries":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn parses_list_json() {
        let s = r#"{"tunnels":[{"tunnelId":"devbox-ssh.usw3","name":"","hostConnections":1,"labels":[],"portCount":1,"tunnelExpiration":"30 days","description":""}]}"#;
        let t = parse_list(s).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].tunnel_id, "devbox-ssh.usw3");
        assert_eq!(t[0].host_connections, 1);
    }

    #[test]
    fn parses_list_with_preamble_and_empty() {
        assert!(parse_list("A new version is available\n{\"tunnels\":[]}").unwrap().is_empty());
    }

    #[test]
    fn parses_show_json() {
        let s = r#"{"tunnel":{"tunnelId":"x.usw3","hostConnections":1,"clientConnections":1,"labels":[],"tunnelExpiration":"30 days","description":"","currentUploadRate":"0 MB/s","ports":[{"portNumber":2222,"protocol":"auto","portUri":"https://abc-2222.usw3.devtunnels.ms/"}],"accessControl":[]}}"#;
        let t = parse_show(s).unwrap();
        assert_eq!(t.ports[0].port_number, 2222);
        assert_eq!(t.client_connections, 1);
    }

    #[test]
    fn parses_auth() {
        assert!(parse_auth("Logged in as a@b.com using Microsoft.\n", "").logged_in);
        assert!(!parse_auth("Not logged in.\n", "").logged_in);
    }
}
