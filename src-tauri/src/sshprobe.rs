//! Detects forwarded ports that lead to an SSH server by reading the banner the server sends on connect
//! (`SSH-2.0-...`). Probes never send bytes, run on their own threads, and are cached per
//! (tunnel id, local port) until the forward goes away.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
const READ_TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_BANNER: usize = 64;
/// Inconclusive probes (no reply, refused) are retried after this long; the host may not be up yet.
const RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Banner {
    Ssh,
    /// The server spoke first with something else.
    Other,
    /// Nothing was received (timeout, refused, closed).
    Silent,
}

/// Classifies the first bytes a server sent.
pub fn classify(bytes: &[u8]) -> Banner {
    if bytes.is_empty() {
        Banner::Silent
    } else if bytes.starts_with(b"SSH-") {
        Banner::Ssh
    } else {
        Banner::Other
    }
}

/// Connects to 127.0.0.1:`port` and reads the server-initiated banner, if any.
pub fn probe(port: u16) -> Banner {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) else {
        return Banner::Silent;
    };
    let deadline = Instant::now() + READ_TIMEOUT;
    let mut buf = [0u8; MAX_BANNER];
    let mut n = 0;
    while n < 4 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || s.set_read_timeout(Some(left)).is_err() {
            break;
        }
        match s.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    let _ = s.shutdown(std::net::Shutdown::Both);
    classify(&buf[..n])
}

#[derive(Debug, Clone, Copy)]
enum Entry {
    Pending,
    Done { banner: Banner, at: Instant },
}

type Key = (String, u16);
pub type OnChange = Arc<dyn Fn() + Send + Sync>;

/// Cache of probe results, keyed by (tunnel id, local port).
#[derive(Clone)]
pub struct SshProbes {
    entries: Arc<Mutex<HashMap<Key, Entry>>>,
    on_change: OnChange,
}

impl SshProbes {
    /// `on_change` runs (on the probe thread) when a probe finds an SSH server.
    pub fn new(on_change: OnChange) -> Self {
        SshProbes { entries: Default::default(), on_change }
    }

    /// Keeps only `active` keys (dropping results for closed forwards) and returns the local ports of
    /// `active` known to speak SSH. Starts probes for new keys and stale inconclusive ones.
    pub fn sync(&self, active: &[(String, u16)]) -> HashMap<String, Vec<u16>> {
        let mut entries = self.entries.lock().unwrap();
        let keep: HashSet<&Key> = active.iter().collect();
        entries.retain(|k, _| keep.contains(k));
        let mut out: HashMap<String, Vec<u16>> = HashMap::new();
        for key in active {
            let start = match entries.get(key) {
                None => true,
                Some(Entry::Done { banner: Banner::Silent, at }) => at.elapsed() >= RETRY_AFTER,
                Some(Entry::Done { banner: Banner::Ssh, .. }) => {
                    out.entry(key.0.clone()).or_default().push(key.1);
                    false
                }
                _ => false,
            };
            if start {
                entries.insert(key.clone(), Entry::Pending);
                self.spawn(key.clone());
            }
        }
        out
    }

    fn spawn(&self, key: Key) {
        let entries = self.entries.clone();
        let on_change = self.on_change.clone();
        let k2 = key.clone();
        let spawned = std::thread::Builder::new().name("ssh-probe".into()).spawn(move || {
            let banner = probe(key.1);
            let mut e = entries.lock().unwrap();
            // Dropped while probing (forward closed): discard so a new forward reprobes.
            if !matches!(e.get(&key), Some(Entry::Pending)) {
                return;
            }
            e.insert(key, Entry::Done { banner, at: Instant::now() });
            drop(e);
            if banner == Banner::Ssh {
                on_change();
            }
        });
        if spawned.is_err() {
            self.entries.lock().unwrap().remove(&k2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn classifies_banners() {
        assert_eq!(classify(b"SSH-2.0-OpenSSH_for_Windows_9.5\r\n"), Banner::Ssh);
        assert_eq!(classify(b"SSH-"), Banner::Ssh);
        assert_eq!(classify(b"HTTP/1.1 400 Bad Request\r\n"), Banner::Other);
        assert_eq!(classify(b"220 smtp ready"), Banner::Other);
        assert_eq!(classify(b"SS"), Banner::Other);
        assert_eq!(classify(b""), Banner::Silent);
    }

    fn listener(banner: Option<&'static [u8]>) -> u16 {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { continue };
                if let Some(b) = banner {
                    // Split writes: the probe must assemble a banner sent in pieces.
                    let _ = s.write_all(&b[..2]);
                    std::thread::sleep(Duration::from_millis(30));
                    let _ = s.write_all(&b[2..]);
                }
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(3));
                    drop(s);
                });
            }
        });
        port
    }

    #[test]
    fn probes_local_listeners() {
        assert_eq!(probe(listener(Some(b"SSH-2.0-OpenSSH_9.6\r\n"))), Banner::Ssh);
        assert_eq!(probe(listener(Some(b"HTTP/1.0 200 OK\r\n"))), Banner::Other);
        let t = Instant::now();
        assert_eq!(probe(listener(None)), Banner::Silent);
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn caches_per_forward_and_drops_closed_ones() {
        let ssh = listener(Some(b"SSH-2.0-test\r\n"));
        let http = listener(Some(b"HTTP/1.1 200 OK\r\n"));
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let probes = SshProbes::new(Arc::new(move || {
            h.fetch_add(1, Ordering::SeqCst);
        }));
        let active = vec![("t".to_string(), ssh), ("t".to_string(), http)];
        assert!(probes.sync(&active).is_empty(), "results arrive asynchronously");
        let deadline = Instant::now() + Duration::from_secs(5);
        while hits.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(probes.sync(&active).get("t"), Some(&vec![ssh]));
        // Forward closed: cache is dropped, and a new forward on the same port is probed again.
        assert!(probes.sync(&[]).is_empty());
        assert!(probes.entries.lock().unwrap().is_empty());
        probes.sync(&active[..1]);
        let deadline = Instant::now() + Duration::from_secs(5);
        while hits.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }
}
