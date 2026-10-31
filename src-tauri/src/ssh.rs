//! "Open SSH" support: pick a ~/.ssh/config alias for a forwarded port and launch a terminal.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub port: Option<u16>,
}

/// Minimal ssh_config parser: top-level `Host` blocks with HostName/Port (Include and Match are ignored).
pub fn parse_ssh_config(s: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for raw in s.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (key, val) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.trim().to_lowercase(), v.trim().trim_start_matches('=').trim()),
            None => continue,
        };
        match key.as_str() {
            "host" => {
                current.clear();
                for alias in val.split_whitespace().filter(|a| !a.contains('*') && !a.contains('?') && !a.starts_with('!')) {
                    hosts.push(SshHost { alias: alias.to_string(), hostname: None, port: None });
                    current.push(hosts.len() - 1);
                }
            }
            "match" => current.clear(),
            "hostname" => current.iter().for_each(|&i| {
                hosts[i].hostname.get_or_insert_with(|| val.to_string());
            }),
            "port" => {
                if let Ok(p) = val.parse() {
                    current.iter().for_each(|&i| {
                        hosts[i].port.get_or_insert(p);
                    });
                }
            }
            _ => {}
        }
    }
    hosts
}

fn is_loopback(h: &str) -> bool {
    matches!(h.to_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

/// Finds a loopback alias whose port matches the local port first, then the remote port.
pub fn find_alias(hosts: &[SshHost], local_port: u16, remote_port: u16) -> Option<String> {
    let loop_hosts: Vec<&SshHost> =
        hosts.iter().filter(|h| h.hostname.as_deref().is_some_and(is_loopback)).collect();
    loop_hosts
        .iter()
        .find(|h| h.port == Some(local_port))
        .or_else(|| loop_hosts.iter().find(|h| h.port == Some(remote_port)))
        .map(|h| h.alias.clone())
}

pub fn read_user_config() -> Vec<SshHost> {
    dirs::home_dir()
        .and_then(|h| std::fs::read_to_string(h.join(".ssh/config")).ok())
        .map(|s| parse_ssh_config(&s))
        .unwrap_or_default()
}

pub fn ssh_command(alias: Option<&str>, local_port: u16) -> Vec<String> {
    // -p on the command line overrides the alias's Port, so the alias still supplies User, keys, etc.
    vec!["ssh".into(), "-p".into(), local_port.to_string(), alias.unwrap_or("localhost").to_string()]
}

#[cfg_attr(windows, allow(dead_code))]
fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/@".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Quotes an argument for a cmd.exe command line. `"` can't be escaped reliably, so it's dropped.
#[cfg_attr(not(windows), allow(dead_code))]
fn cmd_quote(s: &str) -> String {
    let s = s.replace('"', "");
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/@\\".contains(c)) {
        s
    } else {
        format!("\"{s}\"")
    }
}

/// Batch file that echoes and runs `line` (CRLF, `%` doubled so it isn't expanded).
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_script(line: &str) -> String {
    let line = line.replace('%', "%%");
    let shown: String = line.chars().filter(|c| !"&|<>^()\"".contains(*c)).collect();
    format!("@echo off\r\ntitle Tunnel Bar\r\necho $ {shown}\r\n{line}\r\n")
}

/// Opens the default terminal running the given command.
pub fn open_terminal(cmd: &[String]) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let line = cmd.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
        let dir: PathBuf = crate::settings::config_dir().join("scripts");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("open-ssh.command");
        std::fs::write(&path, format!("#!/bin/zsh -l\nclear\necho '$ {}'\n{}\n", line.replace('\'', ""), line))
            .map_err(|e| e.to_string())?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        std::process::Command::new("/usr/bin/open")
            .args(["-a", "Terminal"])
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let line = cmd.iter().map(|a| cmd_quote(a)).collect::<Vec<_>>().join(" ");
        let dir: PathBuf = crate::settings::config_dir().join("scripts");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("open-ssh.cmd"), windows_script(&line)).map_err(|e| e.to_string())?;
        // Windows Terminal first; `-d` sets the tab's directory so the script path needs no quoting.
        let wt = std::process::Command::new("wt.exe")
            .arg("-w")
            .arg("new")
            .arg("new-tab")
            .args(["--title", "Tunnel Bar", "-d"])
            .arg(&dir)
            .args(["cmd.exe", "/K", "open-ssh.cmd"])
            .creation_flags(crate::win::CREATE_NO_WINDOW)
            .spawn();
        if wt.is_ok() {
            return Ok(());
        }
        std::process::Command::new("cmd.exe")
            .args(["/K", "open-ssh.cmd"])
            .current_dir(&dir)
            .creation_flags(crate::win::CREATE_NEW_CONSOLE)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let line = cmd.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
        std::process::Command::new("x-terminal-emulator")
            .args(["-e", &line])
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: &str = r#"
# Anywhere: through the dev tunnel
Host devbox-tunnel
    HostName localhost
    Port 2222
    User redmond\svanvliet
    HostKeyAlias devbox

Host devbox
    HostName 10.0.0.5

Host *
    ServerAliveInterval 30
"#;

    #[test]
    fn finds_alias_by_port() {
        let hosts = parse_ssh_config(CFG);
        assert_eq!(hosts.len(), 2);
        assert_eq!(find_alias(&hosts, 2222, 2222).as_deref(), Some("devbox-tunnel"));
        // Local port differs (2223) but remote matches the alias port.
        assert_eq!(find_alias(&hosts, 2223, 2222).as_deref(), Some("devbox-tunnel"));
        assert_eq!(find_alias(&hosts, 3000, 3000), None);
    }

    #[test]
    fn builds_command() {
        assert_eq!(ssh_command(Some("a"), 2223), vec!["ssh", "-p", "2223", "a"]);
        assert_eq!(shell_quote("redmond\\x"), "'redmond\\x'");
    }

    #[test]
    fn builds_windows_script() {
        assert_eq!(cmd_quote("redmond\\x"), "redmond\\x");
        assert_eq!(cmd_quote("a b"), "\"a b\"");
        assert_eq!(cmd_quote("x&y\""), "\"x&y\"");
        assert_eq!(cmd_quote(""), "\"\"");
        let s = windows_script("ssh -p 2222 \"100%\"");
        assert!(s.starts_with("@echo off\r\n"));
        assert!(s.contains("\r\nssh -p 2222 \"100%%\"\r\n"));
        assert!(s.contains("echo $ ssh -p 2222 100%%\r\n"));
    }
}
