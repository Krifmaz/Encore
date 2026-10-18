//! One Encore window at a time, per user: the first instance listens on a local port picked
//! from the user's config folder; a later launch by the same user connects, both prove they hold
//! the key kept in that folder (`instance.key`, readable by its user only), the later one says
//! "raise" and exits, and the first window comes forward (not when the later one was started by
//! the system at sign-in, with `--minimized`). Another user's Encore on the same computer (fast
//! user switching, two people signed in) cannot answer for this user, so both run; a port taken
//! by someone else moves this user to the next one.

use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

/// The first port this app may listen on (Cadenza's own desktop app uses 47731).
const BASE_PORT: u16 = 47_735;
/// How many ports the config folder can pick from.
const SPREAD: u16 = 1_000;
/// How many ports in a row are tried when one is taken by someone else.
const TRIES: u16 = 4;

/// Set by the updater on the new version it starts: the old one may still be quitting, so its
/// answer is not a reason to exit at once (see [`acquire`]).
pub const RELAUNCH_ENV: &str = "ENCORE_DESKTOP_RELAUNCH";

/// `true` when this process is the one and only instance of this user (and now listens for
/// later launches); `false` when this user's Encore already runs and has been asked to come
/// forward. Started by the updater, it waits up to 10 s for the old version to quit first.
pub fn acquire() -> bool {
    let dir = crate::config::dir();
    let key = key(&dir);
    let ports = ports(&dir.to_string_lossy());
    let relaunched = std::env::var_os(RELAUNCH_ENV).is_some();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let waiting = relaunched && Instant::now() < until;
        let verb = if waiting || super::autostart::started_minimized() {
            "check"
        } else {
            "raise"
        };
        match claim(&ports, &key, verb) {
            Claim::Listening => return true,
            Claim::Running if waiting => std::thread::sleep(Duration::from_millis(500)),
            Claim::Running => return false,
            Claim::Neither => return true,
        }
    }
}

enum Claim {
    /// This process listens now.
    Listening,
    /// This user's Encore runs and answered.
    Running,
    /// Every port is taken by someone else: run without the check.
    Neither,
}

fn claim(ports: &[u16], key: &str, verb: &str) -> Claim {
    if ports.iter().any(|&p| ask(p, key, verb)) {
        return Claim::Running;
    }
    for &p in ports {
        match TcpListener::bind(("127.0.0.1", p)) {
            Ok(l) => {
                listen(l, key.to_string());
                return Claim::Listening;
            }
            // started at the same moment as another launch of this user's
            Err(_) if ask(p, key, verb) => return Claim::Running,
            Err(_) => {}
        }
    }
    log::warn!("every port for the one-window check is taken: running without it");
    Claim::Neither
}

/// The ports this config folder uses, in the order they are tried.
fn ports(seed: &str) -> Vec<u16> {
    let start = (crate::link::fnv1a(seed) % SPREAD as u64) as u16;
    (0..TRIES)
        .map(|i| BASE_PORT + (start + i) % SPREAD)
        .collect()
}

/// `<config dir>/instance.key`, made on the first start (only its user can read it).
fn key(dir: &Path) -> String {
    let file = dir.join("instance.key");
    let read = || {
        std::fs::read_to_string(&file)
            .ok()
            .map(|k| k.trim().to_string())
            .filter(|k| k.len() >= 32)
    };
    if let Some(k) = read() {
        return k;
    }
    let k = random_hex();
    if let Err(e) = crate::config::write_private(&file, k.as_bytes()) {
        log::warn!("could not save {}: {e}", file.display());
        return k;
    }
    // Two first launches at once: both use whichever was written last.
    read().unwrap_or(k)
}

/// 32 hex digits from the OS's random seed (std's `RandomState`), mixed with the time and pid.
fn random_hex() -> String {
    use std::hash::{BuildHasher, Hasher};
    let part = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u32(std::process::id());
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.finish()
    };
    format!("{:016x}{:016x}", part(1), part(2))
}

/// The proof that `role` ("c" for the caller, "s" for the instance) holds `key` for `nonce`.
fn proof(key: &str, role: &str, nonce: &str) -> String {
    let d = Sha256::digest(format!("{key}:{role}:{nonce}").as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Answer later launches: "<verb> <nonce> <caller's proof>" gets "ok <instance's proof>" when the
/// caller holds the key (and "raise" brings the window forward), "no" otherwise.
fn listen(l: TcpListener, key: String) {
    let _ = std::thread::Builder::new()
        .name("encore-instance".into())
        .spawn(move || {
            for s in l.incoming().flatten() {
                answer(s, &key);
            }
        });
}

fn answer(mut s: TcpStream, key: &str) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    if BufReader::new((&s).take(256)).read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let (verb, nonce, given) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    );
    if nonce.is_empty() || given != proof(key, "c", nonce) {
        let _ = s.write_all(b"no\n");
        return;
    }
    let _ = s.write_all(format!("ok {}\n", proof(key, "s", nonce)).as_bytes());
    if verb == "raise" {
        super::send(super::Message::Raise);
    }
}

/// Whether this user's Encore listens on `port` (it is told `verb`).
fn ask(port: u16, key: &str, verb: &str) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_secs(1)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let nonce = random_hex();
    let hello = format!("{verb} {nonce} {}\n", proof(key, "c", &nonce));
    if s.write_all(hello.as_bytes()).is_err() {
        return false;
    }
    let mut reply = String::new();
    if BufReader::new((&s).take(256)).read_line(&mut reply).is_err() {
        return false;
    }
    reply.trim() == format!("ok {}", proof(key, "s", &nonce))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_per_config_folder_and_stay_in_range() {
        let a = ports("/home/alice/.config/encore-desktop");
        let b = ports("/home/bob/.config/encore-desktop");
        assert_eq!(a, ports("/home/alice/.config/encore-desktop"));
        assert_ne!(a, b);
        assert_eq!(a.len(), TRIES as usize);
        assert!(a
            .iter()
            .chain(&b)
            .all(|p| (BASE_PORT..BASE_PORT + SPREAD).contains(p)));
    }

    #[test]
    fn only_the_same_user_gets_an_answer() {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        listen(l, "alice-key-0123456789abcdef0123456789".into());
        assert!(ask(port, "alice-key-0123456789abcdef0123456789", "check"));
        assert!(!ask(port, "bob-key-0123456789abcdef0123456789ab", "check"));
        // something else on the port that just says "ok" is not taken for Encore
        let fake = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let fake_port = fake.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in fake.incoming().flatten() {
                let _ = s.write_all(b"ok\n");
            }
        });
        assert!(!ask(fake_port, "alice-key-0123456789abcdef0123456789", "check"));
    }

    #[test]
    fn the_key_is_made_once_and_kept() {
        let d = tempfile::tempdir().unwrap();
        let k = key(d.path());
        assert_eq!(k.len(), 32);
        assert_eq!(key(d.path()), k);
        assert_ne!(random_hex(), random_hex());
    }
}
