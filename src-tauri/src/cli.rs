//! Thin async wrapper around the `devtunnel` CLI.

use crate::model::{AccessEntry, AccessRuleKind, AccessRuleRequest, AuthStatus, HeaderOptions, TunnelDetails, TunnelSummary};
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
    candidates.extend(windows_candidates(
        dirs::data_local_dir().as_deref(),
        std::env::var_os("ProgramFiles").map(PathBuf::from).as_deref(),
    ));
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

/// Install locations used by winget (user and machine scope, links and unpacked packages) and the MSI/zip.
#[cfg(any(windows, test))]
fn windows_candidates(local_appdata: Option<&Path>, program_files: Option<&Path>) -> Vec<PathBuf> {
    const EXE: &str = "devtunnel.exe";
    let mut out = vec![];
    for root in [local_appdata, program_files].into_iter().flatten() {
        out.push(root.join("Microsoft").join("WinGet").join("Links").join(EXE));
        out.push(root.join("WinGet").join("Links").join(EXE));
    }
    for root in [local_appdata, program_files].into_iter().flatten() {
        for pkgs in [root.join("Microsoft").join("WinGet").join("Packages"), root.join("WinGet").join("Packages")] {
            let Ok(rd) = std::fs::read_dir(&pkgs) else { continue };
            let mut dirs: Vec<PathBuf> = rd
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().to_ascii_lowercase().starts_with("microsoft.devtunnel"))
                .map(|e| e.path())
                .collect();
            dirs.sort();
            out.extend(dirs.into_iter().rev().map(|d| d.join(EXE)));
        }
    }
    if let Some(l) = local_appdata {
        out.push(l.join("Programs").join("devtunnel").join(EXE));
    }
    if let Some(pf) = program_files {
        out.push(pf.join("devtunnel").join(EXE));
        out.push(pf.join("Microsoft").join("devtunnel").join(EXE));
    }
    out
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
        .map(|mut r| {
            r.tunnel.ports.iter_mut().for_each(crate::model::TunnelPort::parse_headers);
            crate::model::index_access_entries(&mut r.tunnel.access_control);
            r.tunnel
        })
        .map_err(|e| format!("could not parse `devtunnel show -j`: {e}"))
}

/// First few meaningful lines of a failure (service errors put the detail on line 2).
pub fn err_text(out: &CliOutput) -> String {
    let t = if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr };
    let lines: Vec<&str> = t
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Request ID:"))
        .take_while(|l| !l.starts_with("Description:") && !l.starts_with("Usage:"))
        .take(3)
        .map(|l| l.trim_start_matches(':').trim())
        .collect();
    if lines.is_empty() { "devtunnel failed".into() } else { lines.join(" ") }
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

fn push_headers<'a>(args: &mut Vec<&'a str>, h: &'a HeaderOptions) {
    if let Some(v) = &h.host_header {
        args.extend(["--host-header", v.as_str()]);
    }
    if let Some(v) = &h.origin_header {
        args.extend(["--origin-header", v.as_str()]);
    }
}

pub fn create_args<'a>(
    id: Option<&'a str>,
    labels: &[&'a str],
    description: &'a str,
    expiration: Option<&'a str>,
    headers: &'a HeaderOptions,
) -> Vec<&'a str> {
    let mut args = vec!["create"];
    args.extend(id);
    args.extend(["-j", "-d", description]);
    for l in labels {
        args.extend(["-l", l]);
    }
    if let Some(e) = expiration {
        args.extend(["-e", e]);
    }
    push_headers(&mut args, headers);
    args
}

/// Creates a tunnel (CLI-generated id when `id` is None); returns "name.cluster".
pub async fn create(
    cli: &Path,
    id: Option<&str>,
    labels: &[&str],
    description: &str,
    expiration: Option<&str>,
    headers: &HeaderOptions,
) -> Result<String, String> {
    parse_create(&ok(cli, &create_args(id, labels, description, expiration, headers)).await?)
}

/// Builds `devtunnel update` args; None if there's nothing to change.
pub fn update_args<'a>(id: &'a str, req: &'a crate::model::UpdateTunnelRequest) -> Option<Vec<&'a str>> {
    let mut args = vec!["update", id, "-j"];
    if let Some(d) = &req.description {
        args.extend(["-d", d.as_str()]);
    }
    for l in &req.add_labels {
        args.extend(["--add-labels", l.as_str()]);
    }
    for l in &req.remove_labels {
        args.extend(["--remove-labels", l.as_str()]);
    }
    if let Some(e) = &req.expiration {
        args.extend(["-e", e.as_str()]);
    }
    push_headers(&mut args, &req.headers);
    (args.len() > 3).then_some(args)
}

pub async fn update(cli: &Path, id: &str, req: &crate::model::UpdateTunnelRequest) -> Result<(), String> {
    match update_args(id, req) {
        Some(args) => ok(cli, &args).await.map(|_| ()),
        None => Ok(()),
    }
}

pub async fn port_delete(cli: &Path, id: &str, port: u16) -> Result<(), String> {
    let port = port.to_string();
    ok(cli, &["port", "delete", id, "-p", &port, "-j"]).await.map(|_| ())
}

pub async fn logout(cli: &Path) -> Result<(), String> {
    ok(cli, &["user", "logout"]).await.map(|_| ())
}

pub fn login_args(provider: crate::model::LoginProvider, device_code: bool) -> Vec<&'static str> {
    let mut args = vec!["user", "login"];
    args.push(match provider {
        crate::model::LoginProvider::Microsoft => "-e",
        crate::model::LoginProvider::Github => "-g",
    });
    if device_code {
        args.push("-d");
    }
    args
}

/// Extracts (url, code) from a device-code prompt such as
/// "To sign in, use a web browser to open the page https://microsoft.com/devicelogin and enter the code ABC123 to authenticate."
pub fn parse_device_code(line: &str) -> Option<(String, String)> {
    static URL: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| regex::Regex::new(r"https://[^\s'\x22]+").unwrap());
    static CODE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"\b(?i:enter|use|input)\s+(?i:the\s+)?(?i:one-time\s+)?(?i:code):?\s+([A-Z0-9]{4,}(?:-[A-Z0-9]{4,})?)\b")
            .unwrap()
    });
    let url = URL.find(line)?.as_str().trim_end_matches(['.', ',', ')']).to_string();
    let code = CODE.captures(line)?.get(1)?.as_str().to_string();
    Some((url, code))
}

/// Mirrors the service's rule for requested tunnel IDs (TunnelConstraints.NewTunnelIdPattern).
pub fn validate_tunnel_id(id: &str) -> Result<(), String> {
    let b = id.as_bytes();
    let ok = (3..=49).contains(&b.len())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && b[0] != b'-'
        && b[b.len() - 1] != b'-';
    if ok {
        Ok(())
    } else {
        Err("Names must be 3–49 lowercase letters, digits or hyphens, and can't start or end with a hyphen.".into())
    }
}

pub async fn port_create(cli: &Path, id: &str, port: u16, protocol: &str, headers: &HeaderOptions) -> Result<(), String> {
    let port = port.to_string();
    let mut args = vec!["port", "create", id, "-p", &port, "--protocol", protocol, "-j"];
    push_headers(&mut args, headers);
    ok(cli, &args).await.map(|_| ())
}

/// Changes a port's Host/Origin header rewriting; no-op when `headers` is empty.
pub async fn port_update_headers(cli: &Path, id: &str, port: u16, headers: &HeaderOptions) -> Result<(), String> {
    if headers.is_empty() {
        return Ok(());
    }
    let port = port.to_string();
    let mut args = vec!["port", "update", id, "-p", &port, "-j"];
    push_headers(&mut args, headers);
    ok(cli, &args).await.map(|_| ())
}

/// Builds `devtunnel access create` args for a (normalized) rule.
pub fn access_create_args(id: &str, rule: &AccessRuleRequest) -> Vec<String> {
    let mut args: Vec<String> = vec!["access".into(), "create".into(), id.into()];
    if let Some(p) = rule.port {
        args.extend(["-p".into(), p.to_string()]);
    }
    match rule.kind {
        AccessRuleKind::Anonymous => args.push("--anonymous".into()),
        AccessRuleKind::Tenant => args.push("--tenant".into()),
        AccessRuleKind::Org => args.extend(["--org".into(), rule.value.clone().unwrap_or_default()]),
        AccessRuleKind::Repo => args.extend(["--repo".into(), rule.value.clone().unwrap_or_default()]),
    }
    if rule.deny {
        args.push("--deny".into());
    }
    if let Some(e) = &rule.expiration {
        args.extend(["-e".into(), e.clone()]);
    }
    args.push("-j".into());
    args
}

/// Adds an access rule, translating the CLI's terse validation errors.
pub async fn access_create(cli: &Path, id: &str, rule: &AccessRuleRequest) -> Result<(), String> {
    let args = access_create_args(id, rule);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ok(cli, &refs).await.map(|_| ()).map_err(|e| access_error(rule.kind, e))
}

pub fn access_error(kind: AccessRuleKind, e: String) -> String {
    if e.contains("same identity provider") {
        return match kind {
            AccessRuleKind::Org => "GitHub organization rules need a GitHub sign-in (Sign in with GitHub, then try again).".into(),
            _ => format!("{e} Sign in with the matching provider and try again."),
        };
    }
    if kind == AccessRuleKind::Repo && e.contains("Specify access control parameters") {
        return "devtunnel rejected the repository rule. Repository rules may need a sign-in with the repo's provider (GitHub for owner/repo).".into();
    }
    e
}

/// Tunnel-level rules, or a port's rules (including inherited tunnel rules) when `port` is set.
pub async fn access_list(cli: &Path, id: &str, port: Option<u16>) -> Result<Vec<AccessEntry>, String> {
    let port = port.map(|p| p.to_string());
    let mut args = vec!["access", "list", id, "-j"];
    if let Some(p) = &port {
        args.extend(["-p", p.as_str()]);
    }
    parse_access_list(&ok(cli, &args).await?)
}

/// Deletes the own (non-inherited) rule at `index`, as numbered by `access_list`.
pub async fn access_delete(cli: &Path, id: &str, port: Option<u16>, index: usize) -> Result<(), String> {
    let (index, port) = (index.to_string(), port.map(|p| p.to_string()));
    let mut args = vec!["access", "delete", id, "-i", &index, "-j"];
    if let Some(p) = &port {
        args.extend(["-p", p.as_str()]);
    }
    ok(cli, &args).await.map(|_| ())
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
        .map(|mut r| {
            crate::model::index_access_entries(&mut r.access_control_entries);
            r.access_control_entries
        })
        .map_err(|e| format!("could not parse `devtunnel access list -j`: {e}"))
}

/// Allows or revokes anonymous (no sign-in) connect access at the tunnel level.
pub async fn set_anonymous(cli: &Path, id: &str, allow: bool) -> Result<(), String> {
    if allow {
        return ok(cli, &["access", "create", id, "--anonymous", "-j"]).await.map(|_| ());
    }
    // Delete from the highest index down so earlier indexes stay valid.
    let entries = parse_access_list(&ok(cli, &["access", "list", id, "-j"]).await?)?;
    for i in entries.iter().rev().filter(|e| e.allows_anonymous()).filter_map(|e| e.index) {
        access_delete(cli, id, None, i).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_tunnel_ids() {
        for ok in ["abc", "my-app-1", &"a".repeat(49)] {
            assert!(validate_tunnel_id(ok).is_ok(), "{ok}");
        }
        for bad in ["ab", "-abc", "abc-", "My-App", "a_b", "a.b", &"a".repeat(50)] {
            assert!(validate_tunnel_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn builds_access_args_and_errors() {
        let rule = |kind, value: Option<&str>| AccessRuleRequest { kind, value: value.map(Into::into), ..Default::default() };
        assert_eq!(access_create_args("t", &rule(AccessRuleKind::Anonymous, None)), ["access", "create", "t", "--anonymous", "-j"]);
        let mut tenant = rule(AccessRuleKind::Tenant, None);
        tenant.expiration = Some("2h".into());
        tenant.port = Some(5173);
        assert_eq!(access_create_args("t", &tenant), ["access", "create", "t", "-p", "5173", "--tenant", "-e", "2h", "-j"]);
        let mut org = rule(AccessRuleKind::Org, Some("contoso"));
        org.deny = true;
        assert_eq!(access_create_args("t", &org), ["access", "create", "t", "--org", "contoso", "--deny", "-j"]);
        assert_eq!(
            access_create_args("t", &rule(AccessRuleKind::Repo, Some("o/r"))),
            ["access", "create", "t", "--repo", "o/r", "-j"]
        );
        let e = "Access control entries must use the same identity provider as the logged-in user.".to_string();
        assert!(access_error(AccessRuleKind::Org, e).contains("GitHub sign-in"));
        assert!(access_error(AccessRuleKind::Repo, "Specify access control parameters.".into()).contains("repository rule"));
        assert_eq!(access_error(AccessRuleKind::Tenant, "boom".into()), "boom");
    }

    #[test]
    fn parses_access_entries_with_inherited_and_expiry() {
        // `devtunnel access list <id> -p 5173 -j` (CLI 1.0.2115).
        let acl = parse_access_list(
            r#"{"accessControlEntries":[
              {"type":"Organizations","provider":"microsoft","isInherited":true,"subjects":["t1"],"scopes":["connect"],"expiration":"2026-11-01T00:24:53.387279Z"},
              {"type":"Anonymous","isInherited":true,"isDeny":true,"subjects":[],"scopes":["connect"]},
              {"type":"Organizations","provider":"microsoft","subjects":["t1"],"scopes":["connect"]},
              {"type":"Anonymous","subjects":[],"scopes":["connect"]}]}"#,
        )
        .unwrap();
        assert_eq!(acl.iter().map(|e| e.index).collect::<Vec<_>>(), [None, None, Some(0), Some(1)]);
        assert_eq!(acl[0].expiration.as_deref(), Some("2026-11-01T00:24:53.387279Z"));
        assert_eq!(acl[0].provider, "microsoft");
        assert!(acl[1].is_deny && !acl[1].allows_anonymous());
        assert!(acl[3].allows_anonymous());
    }

    #[test]
    fn parses_port_header_options() {
        let d = parse_show(
            r#"{"tunnel":{"tunnelId":"x.usw3","ports":[
              {"portNumber":5173,"protocol":"http","headerOptionDescription":"  (Host:unchanged, Origin:unchanged))"},
              {"portNumber":5174,"protocol":"auto","headerOptionDescription":"  (Host:localhost, Origin:https://example.com))"},
              {"portNumber":22,"protocol":"auto"}],
              "accessControl":[{"type":"Anonymous","subjects":[],"scopes":["connect"]}]}}"#,
        )
        .unwrap();
        assert_eq!(d.ports[0].host_header.as_deref(), Some("unchanged"));
        assert_eq!(d.ports[0].origin_header.as_deref(), Some("unchanged"));
        assert_eq!(d.ports[1].host_header.as_deref(), Some("localhost"));
        assert_eq!(d.ports[1].origin_header.as_deref(), Some("https://example.com"));
        assert_eq!((d.ports[2].host_header.clone(), d.ports[2].origin_header.clone()), (None, None));
        assert_eq!(d.access_control[0].index, Some(0));
    }

    #[test]
    fn builds_create_update_and_login_args() {
        let vite = HeaderOptions { host_header: Some("unchanged".into()), origin_header: Some("unchanged".into()) };
        assert_eq!(
            create_args(Some("x"), &["tunnelbar", "a"], "d", Some("8h"), &vite),
            [
                "create", "x", "-j", "-d", "d", "-l", "tunnelbar", "-l", "a", "-e", "8h", "--host-header", "unchanged",
                "--origin-header", "unchanged"
            ]
        );
        assert_eq!(create_args(None, &[], "", None, &Default::default()), ["create", "-j", "-d", ""]);
        let req = crate::model::UpdateTunnelRequest {
            description: Some(String::new()),
            add_labels: vec!["a".into(), "b".into()],
            remove_labels: vec!["c".into()],
            expiration: Some("2d".into()),
            headers: Default::default(),
        };
        assert_eq!(
            update_args("t", &req).unwrap(),
            ["update", "t", "-j", "-d", "", "--add-labels", "a", "--add-labels", "b", "--remove-labels", "c", "-e", "2d"]
        );
        assert!(update_args("t", &Default::default()).is_none());
        let hdr = crate::model::UpdateTunnelRequest {
            headers: HeaderOptions { host_header: Some("localhost".into()), origin_header: None },
            ..Default::default()
        };
        assert_eq!(update_args("t", &hdr).unwrap(), ["update", "t", "-j", "--host-header", "localhost"]);
        use crate::model::LoginProvider::*;
        assert_eq!(login_args(Microsoft, false), ["user", "login", "-e"]);
        assert_eq!(login_args(Github, true), ["user", "login", "-g", "-d"]);
    }

    #[test]
    fn parses_device_code_prompts() {
        let ms = "To sign in, use a web browser to open the page https://microsoft.com/devicelogin and enter the code F7QRSTUVW to authenticate.";
        assert_eq!(parse_device_code(ms), Some(("https://microsoft.com/devicelogin".into(), "F7QRSTUVW".into())));
        let gh = "Copy your one-time code: 1A2B-3C4D. Then open https://github.com/login/device and use the code 1A2B-3C4D";
        assert_eq!(parse_device_code(gh), Some(("https://github.com/login/device".into(), "1A2B-3C4D".into())));
        assert_eq!(parse_device_code("Logged in as a@b.com using Microsoft."), None);
        assert_eq!(parse_device_code("Browse to https://example.com"), None);
    }

    #[test]
    fn error_text_keeps_service_detail() {
        let out = CliOutput {
            success: false,
            stdout: String::new(),
            stderr: "Tunnel service error: One or more validation errors occurred. \n: Invalid custom expiration. Value must be between 1 hour to 30 days\nRequest ID: abc\n".into(),
        };
        assert_eq!(
            err_text(&out),
            "Tunnel service error: One or more validation errors occurred. Invalid custom expiration. Value must be between 1 hour to 30 days"
        );
        let out = CliOutput { success: false, stdout: "'x' was not matched.\nDescription:\n  Update".into(), stderr: String::new() };
        assert_eq!(err_text(&out), "'x' was not matched.");
    }

    #[test]
    fn lists_winget_install_locations() {
        let tmp = tempfile::tempdir().unwrap();
        let (local, pf) = (tmp.path().join("Local"), tmp.path().join("PF"));
        let pkgs = local.join("Microsoft").join("WinGet").join("Packages");
        for d in ["Microsoft.devtunnel_Microsoft.Winget.Source_8wekyb3d8bbwe", "Other.app_x"] {
            std::fs::create_dir_all(pkgs.join(d)).unwrap();
        }
        let c = windows_candidates(Some(&local), Some(&pf));
        assert_eq!(c[0], local.join("Microsoft/WinGet/Links/devtunnel.exe"));
        assert!(c.contains(&pkgs.join("Microsoft.devtunnel_Microsoft.Winget.Source_8wekyb3d8bbwe/devtunnel.exe")));
        assert!(!c.iter().any(|p| p.to_string_lossy().contains("Other.app")));
        assert!(c.contains(&pf.join("WinGet").join("Links").join("devtunnel.exe")));
        assert!(windows_candidates(None, None).is_empty());
    }

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
