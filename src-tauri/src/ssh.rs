//! "Open SSH" support: pick a ~/.ssh/config alias for a forwarded port and launch a terminal.

#[cfg(any(target_os = "macos", windows))]
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
                for alias in val
                    .split_whitespace()
                    .filter(|a| !a.contains('*') && !a.contains('?') && !a.starts_with('!'))
                {
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

pub fn ssh_command(alias: Option<&str>, local_port: u16) -> Result<Vec<String>, String> {
    managed_command(&crate::settings::config_dir().join("scripts"), alias, local_port)
}

fn managed_command(
    dir: &std::path::Path,
    alias: Option<&str>,
    local_port: u16,
) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    let command = {
        let path = dir.join("ssh-clean");
        write_unix_helper(&path)?;
        vec![path.to_string_lossy().into_owned()]
    };
    #[cfg(windows)]
    {
        let path = dir.join("ssh-clean.ps1");
        let source = include_str!("ssh-clean.ps1");
        if std::fs::read_to_string(&path).ok().as_deref() != Some(source) {
            std::fs::write(&path, source).map_err(|e| e.to_string())?;
        }
        return Ok(managed_powershell_command(&path, alias, local_port));
    };
    // -p on the command line overrides the alias's Port, so the alias still supplies User, keys, etc.
    #[cfg(unix)]
    Ok([command, vec!["-p".into(), local_port.to_string(), alias.unwrap_or("localhost").to_string()]]
        .concat())
}

#[cfg(any(windows, test))]
fn managed_powershell_command(path: &std::path::Path, alias: Option<&str>, local_port: u16) -> Vec<String> {
    powershell_command(&format!(
        "& {} -SshArgs @('-p', {}, {}); exit $LASTEXITCODE",
        powershell_quote(&path.to_string_lossy()),
        powershell_quote(&local_port.to_string()),
        powershell_quote(alias.unwrap_or("localhost")),
    ))
}

#[cfg(unix)]
fn write_unix_helper(path: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let source = include_str!("ssh-clean.sh");
    if std::fs::read_to_string(path).ok().as_deref() == Some(source) {
        return Ok(());
    }
    // Replacing the inode avoids truncating a script another SSH session is still reading.
    let staging =
        path.with_extension(format!("{}.{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o700).open(&staging)?;
        file.write_all(source.as_bytes())?;
        std::fs::rename(&staging, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(staging);
    }
    result.map_err(|e| e.to_string())
}

#[cfg_attr(windows, allow(dead_code))]
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/@".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(any(windows, test))]
fn powershell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

// EncodedCommand is UTF-16LE/base64. No shell interprets the embedded path or SSH arguments.
#[cfg(any(windows, test))]
fn powershell_command(script: &str) -> Vec<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as usize;
        let b = chunk.get(1).copied().unwrap_or(0) as usize;
        let c = chunk.get(2).copied().unwrap_or(0) as usize;
        encoded.push(ALPHABET[a >> 2] as char);
        encoded.push(ALPHABET[((a & 3) << 4) | (b >> 4)] as char);
        encoded.push(if chunk.len() > 1 { ALPHABET[((b & 15) << 2) | (c >> 6)] as char } else { '=' });
        encoded.push(if chunk.len() > 2 { ALPHABET[c & 63] as char } else { '=' });
    }
    vec![
        "powershell.exe".into(),
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-EncodedCommand".into(),
        encoded,
    ]
}

pub fn command_line(cmd: &[String]) -> String {
    #[cfg(unix)]
    return cmd.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
    #[cfg(windows)]
    return cmd.join(" "); // Managed commands contain only shell-safe launcher flags and base64.
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
        let line = command_line(cmd);
        let dir: PathBuf = crate::settings::config_dir().join("scripts");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("open-ssh.command");
        std::fs::write(
            &path,
            format!("#!/bin/zsh -l\nclear\nprintf '%s\\n' {}\n{}\n", shell_quote(&format!("$ {line}")), line),
        )
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
        let line = if cmd.first().is_some_and(|s| s == "powershell.exe") {
            command_line(cmd)
        } else {
            let script = cmd.iter().map(|a| powershell_quote(a)).collect::<Vec<_>>().join(" ");
            powershell_command(&format!("& {script}; exit $LASTEXITCODE")).join(" ")
        };
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
        let line = command_line(cmd);
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
    User example\developer
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
    fn encodes_windows_invocations_without_shell_expansion() {
        assert_eq!(powershell_quote("devbox ' & %PATH%! $x"), "'devbox '' & %PATH%! $x'");
        assert_eq!(powershell_command("a")[6], "YQA=");
        assert_eq!(powershell_command("ab")[6], "YQBiAA==");
        assert_eq!(powershell_command("abc")[6], "YQBiAGMA");
    }

    #[test]
    fn managed_powershell_launcher_preserves_helper_exit_status() {
        let executable = if cfg!(windows) { "powershell.exe" } else { "pwsh" };
        // Windows PowerShell treats `-Version` as "start version X", so probe with a no-op command.
        match std::process::Command::new(executable).args(["-NoProfile", "-Command", "exit"]).output() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !cfg!(windows) => {
                eprintln!("Skipping PowerShell launcher test: pwsh is not installed");
                return;
            }
            result => assert!(result.unwrap().status.success()),
        }
        let dir = tempfile::Builder::new().prefix("ssh test ' & ").tempdir_in(".").unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let helper = dir.join("ssh-clean.ps1");
        std::fs::write(
            &helper,
            "param([string[]]$SshArgs)\n[IO.File]::WriteAllLines($env:RECORD, $SshArgs)\nexit ([int]$env:SSH_STATUS)\n",
        )
        .unwrap();
        let alias = "devbox ' \" ; $HOME & %PATH%! \\";
        let cmd = managed_powershell_command(&helper, Some(alias), 2223);
        for status in [37, 255] {
            let output = std::process::Command::new(executable)
                .args(&cmd[1..])
                .env("RECORD", dir.join("args"))
                .env("SSH_STATUS", status.to_string())
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(status), "{}", String::from_utf8_lossy(&output.stderr));
            assert_eq!(
                std::fs::read_to_string(dir.join("args")).unwrap().lines().collect::<Vec<_>>(),
                vec!["-p", "2223", alias]
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_helper_forwards_native_arguments_and_preserves_failure() {
        let dir = tempfile::Builder::new().prefix("ssh test ' & % ").tempdir_in(".").unwrap();
        let dir = std::env::current_dir().unwrap().join(dir.path());
        let alias = "devbox ' \" ; $HOME & %PATH%! \\";
        let cmd = managed_command(&dir, Some(alias), 2223).unwrap();
        let compiler = format!(
            r#"
Add-Type -OutputAssembly {} -OutputType ConsoleApplication -TypeDefinition @'
using System;
using System.IO;
public class FakeSsh {{
    public static void Main(string[] args) {{
        File.WriteAllText(Environment.GetEnvironmentVariable("RECORD"), String.Join("\n", args));
        Environment.Exit(37);
    }}
}}
'@
{}; exit $LASTEXITCODE
"#,
            powershell_quote(&dir.join("ssh.exe").to_string_lossy()),
            command_line(&cmd)
        );
        let output = std::process::Command::new("powershell.exe")
            .args(&powershell_command(&compiler)[1..])
            .env("PATH", format!("{};{}", dir.display(), std::env::var("PATH").unwrap()))
            .env("RECORD", dir.join("args"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(37), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(
            std::fs::read_to_string(dir.join("args")).unwrap(),
            format!("-o\nServerAliveInterval=15\n-o\nServerAliveCountMax=3\n-p\n2223\n{alias}")
        );
    }

    #[test]
    fn builds_windows_script() {
        let s = windows_script("ssh -p 2222 \"100%\"");
        assert!(s.starts_with("@echo off\r\n"));
        assert!(s.contains("\r\nssh -p 2222 \"100%%\"\r\n"));
        assert!(s.contains("echo $ ssh -p 2222 100%%\r\n"));
    }

    #[cfg(unix)]
    fn fake_ssh(dir: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("ssh");
        std::fs::write(
            &path,
            r#"#!/usr/bin/env python3
import os, signal, sys, termios, time
with open(os.environ['RECORD'], 'w') as f:
    f.write(''.join(arg + '\n' for arg in sys.argv[1:]))
if os.isatty(0):
    attrs = termios.tcgetattr(0)
    attrs[3] &= ~(termios.ECHO | termios.ICANON)
    termios.tcsetattr(0, termios.TCSANOW, attrs)
def interrupted(*_):
    sys.exit(42)
signal.signal(signal.SIGINT, interrupted)
if os.environ.get('INTERRUPT') == '1':
    print('READY', flush=True)
    while True:
        time.sleep(60)
sys.exit(int(os.environ['SSH_STATUS']))
"#,
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copied_helper_forwards_arguments_and_preserves_failure_without_a_tty() {
        let dir = tempfile::Builder::new().prefix("ssh test ' ").tempdir_in(".").unwrap();
        let dir = dir.path().canonicalize().unwrap();
        fake_ssh(&dir);
        let alias = "devbox ' \" ; $HOME & \\";
        let mut cmd = managed_command(&dir, Some(alias), 2223).unwrap();
        cmd.push(String::new());
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &command_line(&cmd)])
            .env("PATH", format!("{}:{}", dir.display(), std::env::var("PATH").unwrap()))
            .env("RECORD", dir.join("args"))
            .env("SSH_STATUS", "37")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(37));
        assert!(output.stdout.is_empty(), "Redirected output must not contain terminal escapes");
        assert_eq!(
            std::fs::read_to_string(dir.join("args")).unwrap(),
            format!("-o\nServerAliveInterval=15\n-o\nServerAliveCountMax=3\n-p\n2223\n{alias}\n\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn helper_cleans_a_real_tty_after_success_failure_and_ctrl_c() {
        let dir = tempfile::Builder::new().prefix("ssh-pty-").tempdir_in(".").unwrap();
        let dir = dir.path().canonicalize().unwrap();
        fake_ssh(&dir);
        let cmd = managed_command(&dir, None, 2222).unwrap();
        // Headless PTY: verify the emitted escapes AND actual restored terminal attributes.
        let python = r#"
import errno, os, pty, re, select, subprocess, sys, termios, time
master, slave = pty.openpty()
def setup():
    os.setsid()
    import fcntl
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)
supervisor = '''
import signal, subprocess, sys, termios
signal.signal(signal.SIGINT, lambda *_: None)
status = subprocess.call(sys.argv[1:])
flags = termios.tcgetattr(0)[3]
print("TTY_FLAGS=%d" % flags, flush=True)
sys.exit(status)
'''
p = subprocess.Popen([sys.executable, '-c', supervisor] + sys.argv[1:],
                     stdin=slave, stdout=slave, stderr=slave, preexec_fn=setup)
data = b''
deadline = time.monotonic() + 10
sent = False
try:
    while p.poll() is None:
        if time.monotonic() > deadline:
            raise RuntimeError('SSH helper timed out')
        if select.select([master], [], [], .1)[0]:
            data += os.read(master, 4096)
        if os.environ.get('INTERRUPT') == '1' and b'READY' in data and not sent:
            os.write(master, b'\x03')
            sent = True
    while select.select([master], [], [], .1)[0]:
        try:
            chunk = os.read(master, 4096)
        except OSError as e:
            if e.errno == errno.EIO:
                break
            raise
        if not chunk:
            break
        data += chunk
    match = re.search(rb'TTY_FLAGS=(\d+)', data)
    assert match, data
    flags = int(match[1])
    assert flags & termios.ECHO and flags & termios.ICANON, 'stty sane did not restore tty'
    for mode in (1000, 1002, 1003, 1006, 1004, 2004):
        assert ('\x1b[?%dl' % mode).encode() in data, (mode, data)
    assert b'\x1b[0m' in data and b'\x1b[?25h' in data, data
    expected = 42 if sent else int(os.environ['SSH_STATUS'])
    assert p.returncode == expected, (p.returncode, expected, data)
finally:
    if p.poll() is None:
        import signal
        os.killpg(p.pid, signal.SIGKILL)
        p.wait()
    os.close(master)
    os.close(slave)
"#;
        for (status, interrupt) in [("0", "0"), ("37", "0"), ("0", "1")] {
            let output = std::process::Command::new("python3")
                .args(["-c", python])
                .args(&cmd)
                .env("PATH", format!("{}:{}", dir.display(), std::env::var("PATH").unwrap()))
                .env("RECORD", dir.join("args"))
                .env("SSH_STATUS", status)
                .env("INTERRUPT", interrupt)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "PTY case {status}/{interrupt}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
