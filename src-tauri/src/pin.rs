//! Stable local ports for `devtunnel connect`.
//!
//! The CLI picks each forward's local port itself (the remote port if free, otherwise the next
//! free one) and has no flag to choose it. A pin is a small loopback TCP proxy owned by Tunnel Bar:
//! it listens on a fixed local port and forwards each new connection to whatever local port the
//! current `devtunnel connect` child chose. The target is resolved per connection, so pins survive
//! reconnects that land on a different port, work for any TCP protocol (SSH, HTTP, databases), and
//! let a static `~/.ssh/config` `Port` keep working without Tunnel Bar editing the user's files.

use crate::model::PinView;
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};

/// Returns the local port `devtunnel connect` currently uses for (tunnel id, remote port).
pub type Resolver = Arc<dyn Fn(&str, u16) -> Option<u16> + Send + Sync>;

#[derive(Default, Clone)]
struct Status {
    listening: bool,
    error: Option<String>,
}

struct Entry {
    local: u16,
    task: tauri::async_runtime::JoinHandle<()>,
    status: Arc<Mutex<Status>>,
}

pub struct Pins {
    entries: Mutex<HashMap<(String, u16), Entry>>,
    resolve: Resolver,
}

impl Pins {
    pub fn new(resolve: Resolver) -> Self {
        Self { entries: Mutex::new(HashMap::new()), resolve }
    }

    /// Makes the running proxies match `wanted` (tunnel id, remote port, local port).
    pub fn sync(&self, wanted: &[(String, u16, u16)]) {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|(id, remote), e| {
            let keep = wanted.iter().any(|(wid, wr, wl)| wid == id && wr == remote && *wl == e.local);
            if !keep {
                e.task.abort();
            }
            keep
        });
        for (id, remote, local) in wanted {
            let key = (id.clone(), *remote);
            if entries.contains_key(&key) {
                continue;
            }
            let status = Arc::new(Mutex::new(Status::default()));
            let task = tauri::async_runtime::spawn(serve(id.clone(), *remote, *local, self.resolve.clone(), status.clone()));
            entries.insert(key, Entry { local: *local, task, status });
        }
    }

    pub fn stop_all(&self) {
        self.sync(&[]);
    }

    pub fn view(&self, id: &str, remote: u16, local: u16) -> PinView {
        let status = self
            .entries
            .lock()
            .unwrap()
            .get(&(id.to_string(), remote))
            .filter(|e| e.local == local)
            .map(|e| e.status.lock().unwrap().clone());
        let target = (self.resolve)(id, remote);
        let (listening, error) = match status {
            Some(s) => (s.listening, s.error),
            None => (false, None),
        };
        PinView { remote_port: remote, local_port: local, listening, target_port: target, error }
    }
}

impl Drop for Pins {
    fn drop(&mut self) {
        for e in self.entries.get_mut().unwrap().values() {
            e.task.abort();
        }
    }
}

async fn bind(local: u16) -> std::io::Result<(TcpListener, Option<TcpListener>)> {
    let v4 = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, local))).await?;
    // Also hold ::1 so "localhost" can't resolve to a different listener; IPv6 may be unavailable.
    let v6 = TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, local))).await.ok();
    Ok((v4, v6))
}

async fn serve(id: String, remote: u16, local: u16, resolve: Resolver, status: Arc<Mutex<Status>>) {
    loop {
        match bind(local).await {
            Ok((v4, v6)) => {
                *status.lock().unwrap() = Status { listening: true, error: None };
                loop {
                    let accepted = match &v6 {
                        Some(v6) => tokio::select! { a = v4.accept() => a, a = v6.accept() => a },
                        None => v4.accept().await,
                    };
                    let Ok((inbound, _)) = accepted else { continue };
                    let target = resolve(&id, remote).filter(|&t| t != local);
                    if let Some(target) = target {
                        tauri::async_runtime::spawn(forward(inbound, target));
                    }
                }
            }
            Err(e) => {
                // `devtunnel connect` itself owning the pinned port is fine: clients reach it directly.
                let direct = resolve(&id, remote) == Some(local);
                *status.lock().unwrap() = Status {
                    listening: false,
                    error: (!direct).then(|| format!("Can't listen on localhost:{local}: {e}")),
                };
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

async fn forward(mut inbound: TcpStream, target: u16) {
    let connect = async {
        match TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, target))).await {
            Ok(s) => Ok(s),
            Err(_) => TcpStream::connect(SocketAddr::from((Ipv6Addr::LOCALHOST, target))).await,
        }
    };
    let Ok(Ok(mut outbound)) = tokio::time::timeout(Duration::from_secs(5), connect).await else { return };
    let _ = inbound.set_nodelay(true);
    let _ = outbound.set_nodelay(true);
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicU16, Ordering};

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    /// Echo server that prefixes replies with `tag`.
    fn echo_server(tag: &'static str) -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut buf = [0u8; 64];
                    if let Ok(n) = s.read(&mut buf) {
                        let _ = s.write_all(format!("{tag}:{}", String::from_utf8_lossy(&buf[..n])).as_bytes());
                    }
                });
            }
        });
        port
    }

    fn roundtrip(port: u16) -> String {
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = c.write_all(b"hi");
        let mut s = String::new();
        let _ = c.read_to_string(&mut s);
        s
    }

    fn wait(f: impl Fn() -> bool) {
        for _ in 0..100 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!("timed out");
    }

    #[test]
    fn forwards_to_current_target_and_follows_changes() {
        let (a, b) = (echo_server("a"), echo_server("b"));
        let target = Arc::new(AtomicU16::new(a));
        let t2 = target.clone();
        let pins = Pins::new(Arc::new(move |id, remote| (id == "t" && remote == 22).then(|| t2.load(Ordering::SeqCst))));
        let local = free_port();
        pins.sync(&[("t".into(), 22, local)]);
        wait(|| pins.view("t", 22, local).listening);
        assert_eq!(roundtrip(local), "a:hi");
        // A reconnect moved the forward to another port; the pin follows it.
        target.store(b, Ordering::SeqCst);
        assert_eq!(roundtrip(local), "b:hi");
        let v = pins.view("t", 22, local);
        assert_eq!((v.target_port, v.error), (Some(b), None));

        pins.sync(&[]);
        wait(|| std::net::TcpStream::connect(("127.0.0.1", local)).is_err());
        assert!(!pins.view("t", 22, local).listening);
    }

    #[test]
    fn reports_busy_port_unless_connect_owns_it() {
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = busy.local_addr().unwrap().port();
        let owner = Arc::new(AtomicU16::new(0));
        let o2 = owner.clone();
        let pins = Pins::new(Arc::new(move |_, _| Some(o2.load(Ordering::SeqCst)).filter(|&p| p != 0)));
        pins.sync(&[("t".into(), 22, port)]);
        wait(|| pins.view("t", 22, port).error.is_some());
        assert!(!pins.view("t", 22, port).listening);
        // When devtunnel itself listens on the pinned port there's nothing to proxy and no error.
        owner.store(port, Ordering::SeqCst);
        for _ in 0..80 {
            if pins.view("t", 22, port).error.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let v = pins.view("t", 22, port);
        assert_eq!((v.listening, v.error, v.target_port), (false, None, Some(port)));
        drop(busy);
    }

    #[test]
    fn no_target_closes_connection() {
        let pins = Pins::new(Arc::new(|_, _| None));
        let local = free_port();
        pins.sync(&[("t".into(), 22, local)]);
        wait(|| pins.view("t", 22, local).listening);
        assert_eq!(roundtrip(local), "");
    }
}
