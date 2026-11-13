//! Decides which published container ports serve a web page by probing them over the network.
//!
//! Probes run on background threads and their verdicts are cached, so annotating a listing never
//! waits on the network. A port gets a scheme only after a probe confirmed it.

use crate::models::{ContainerInfo, ContainerListing, PublishedPortMapping, WebScheme};
use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

/// Images that must never be contacted: Testcontainers' Ryuk reaps containers once a client
/// connects and then goes away.
const NEVER_PROBED_IMAGES: &[&str] = &["testcontainers/ryuk"];

/// Container ports that are well known to carry something other than HTTP. Several of them speak
/// TLS (465, 993, 995), which the TLS step would otherwise report as HTTPS.
const NON_WEB_CONTAINER_PORTS: &[u16] = &[
    21, 22, 25, 110, 143, 465, 587, 993, 995, 1025, 1433, 1521, 1883, 2181, 3306, 4222, 5432, 5672,
    6379, 9042, 9092, 11211, 27017,
];

/// Delays before a port that was not a web server is probed again, for containers that are still
/// starting up. The last delay repeats.
const RETRY_DELAYS: &[Duration] = &[
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
];

const HEAD_REQUEST: &[u8] = b"HEAD / HTTP/1.0\r\nHost: localhost\r\n\r\n";

/// A hand-built TLS ClientHello offering TLS 1.3 and 1.2 with common cipher suites, SNI
/// `localhost`, and an X25519 key share holding the curve's base point (u = 9). Its only purpose is
/// to make a TLS server answer with a ServerHello or an alert; the handshake is never completed.
/// `client_hello_lengths_are_consistent` checks the length fields.
const CLIENT_HELLO: &[u8] = &[
    // Record header: handshake, legacy version TLS 1.0, length 196.
    0x16, 0x03, 0x01, 0x00, 0xc4, //
    // Handshake header: ClientHello, length 192.
    0x01, 0x00, 0x00, 0xc0, //
    // Legacy version TLS 1.2, fixed random, empty session ID.
    0x03, 0x03, //
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
    0x00, //
    // 17 cipher suites: TLS 1.3, then ECDHE and RSA suites for TLS 1.2.
    0x00, 0x22, //
    0x13, 0x01, 0x13, 0x02, 0x13, 0x03, 0xc0, 0x2b, 0xc0, 0x2f, 0xc0, 0x2c, 0xc0, 0x30, 0xcc, 0xa9,
    0xcc, 0xa8, 0xc0, 0x09, 0xc0, 0x13, 0xc0, 0x0a, 0xc0, 0x14, 0x00, 0x9c, 0x00, 0x9d, 0x00, 0x2f,
    0x00, 0x35, //
    // Null compression only.
    0x01, 0x00, //
    // Extensions, 117 bytes.
    0x00, 0x75, //
    // server_name: localhost.
    0x00, 0x00, 0x00, 0x0e, 0x00, 0x0c, 0x00, 0x00, 0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o',
    b's', b't', //
    // supported_groups: x25519, secp256r1, secp384r1.
    0x00, 0x0a, 0x00, 0x08, 0x00, 0x06, 0x00, 0x1d, 0x00, 0x17, 0x00, 0x18, //
    // ec_point_formats: uncompressed.
    0x00, 0x0b, 0x00, 0x02, 0x01, 0x00, //
    // signature_algorithms: ECDSA, RSA-PSS, PKCS#1 v1.5 with SHA-256/384/512, and Ed25519.
    0x00, 0x0d, 0x00, 0x14, 0x00, 0x12, 0x04, 0x03, 0x08, 0x04, 0x04, 0x01, 0x05, 0x03, 0x08, 0x05,
    0x05, 0x01, 0x08, 0x06, 0x06, 0x01, 0x08, 0x07, //
    // supported_versions: TLS 1.3, TLS 1.2.
    0x00, 0x2b, 0x00, 0x05, 0x04, 0x03, 0x04, 0x03, 0x03, //
    // psk_key_exchange_modes: psk_dhe_ke.
    0x00, 0x2d, 0x00, 0x02, 0x01, 0x01, //
    // key_share: x25519 with the base point as the public key.
    0x00, 0x33, 0x00, 0x26, 0x00, 0x24, 0x00, 0x1d, 0x00, 0x20, //
    0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

#[derive(Debug, Clone, Copy)]
struct ProbeTimeouts {
    connect: Duration,
    /// How long a server may stay silent after the connection before it counts as one that waits
    /// for the client to speak first.
    passive: Duration,
    reply: Duration,
}

const PROBE_TIMEOUTS: ProbeTimeouts = ProbeTimeouts {
    connect: Duration::from_millis(300),
    passive: Duration::from_millis(100),
    reply: Duration::from_secs(1),
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProbeKey {
    container_id: String,
    address: SocketAddr,
    container_port: u16,
}

#[derive(Debug)]
struct CacheEntry {
    verdict: Option<WebScheme>,
    in_flight: bool,
    failures: usize,
    next_probe: Instant,
}

#[derive(Debug, Default)]
struct ProbeCache {
    entries: HashMap<ProbeKey, CacheEntry>,
}

impl ProbeCache {
    /// Forgets keys that are no longer listed and returns the keys to probe now, marking them in
    /// flight.
    fn schedule(&mut self, keys: &HashSet<ProbeKey>, now: Instant) -> Vec<ProbeKey> {
        self.entries.retain(|key, _| keys.contains(key));
        let mut due = Vec::new();
        for key in keys {
            let entry = self.entries.entry(key.clone()).or_insert(CacheEntry {
                verdict: None,
                in_flight: false,
                failures: 0,
                next_probe: now,
            });
            if !entry.in_flight && entry.verdict.is_none() && entry.next_probe <= now {
                entry.in_flight = true;
                due.push(key.clone());
            }
        }
        due
    }

    /// Stores a probe's verdict. A key that was forgotten while its probe ran stays forgotten.
    fn record(&mut self, key: &ProbeKey, verdict: Option<WebScheme>, now: Instant) {
        let Some(entry) = self.entries.get_mut(key) else {
            return;
        };
        entry.in_flight = false;
        entry.verdict = verdict;
        if verdict.is_none() {
            let delay = RETRY_DELAYS[entry.failures.min(RETRY_DELAYS.len() - 1)];
            entry.failures += 1;
            entry.next_probe = now + delay;
        }
    }

    fn verdict(&self, key: &ProbeKey) -> Option<WebScheme> {
        self.entries.get(key).and_then(|entry| entry.verdict)
    }
}

/// Probes published ports of running containers in the background and remembers which ones serve
/// HTTP or HTTPS.
#[derive(Clone, Default)]
pub struct PortProbes {
    cache: Arc<Mutex<ProbeCache>>,
}

impl PortProbes {
    /// Sets each mapping's `web_scheme` from the cached verdicts and starts probes for mappings
    /// without a current verdict. It never waits for a probe; new verdicts show up in a later
    /// listing.
    pub fn annotate(&self, listing: &mut ContainerListing) {
        let keys = listing
            .containers
            .iter()
            .flat_map(|container| {
                container
                    .port_mappings
                    .iter()
                    .filter_map(|mapping| probe_key(container, mapping))
            })
            .collect::<HashSet<_>>();
        let due = {
            let mut cache = lock(&self.cache);
            let due = cache.schedule(&keys, Instant::now());
            for container in &mut listing.containers {
                for index in 0..container.port_mappings.len() {
                    let key = probe_key(container, &container.port_mappings[index]);
                    container.port_mappings[index].web_scheme =
                        key.and_then(|key| cache.verdict(&key));
                }
            }
            due
        };
        for key in due {
            self.spawn_probe(key);
        }
    }

    fn spawn_probe(&self, key: ProbeKey) {
        let cache = Arc::clone(&self.cache);
        let thread_key = key.clone();
        let spawned = thread::Builder::new()
            .name("port-probe".into())
            .spawn(move || {
                let verdict = probe(thread_key.address, PROBE_TIMEOUTS);
                lock(&cache).record(&thread_key, verdict, Instant::now());
            });
        if spawned.is_err() {
            lock(&self.cache).record(&key, None, Instant::now());
        }
    }
}

fn lock(cache: &Mutex<ProbeCache>) -> MutexGuard<'_, ProbeCache> {
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Returns what to probe for a mapping, or `None` when it must not be probed: the container is not
/// running, the mapping is not TCP, it is on the denylist, or its bind address is malformed.
fn probe_key(container: &ContainerInfo, mapping: &PublishedPortMapping) -> Option<ProbeKey> {
    if !container.state.eq_ignore_ascii_case("running")
        || !mapping.protocol.eq_ignore_ascii_case("tcp")
        || is_denied(&container.image, mapping.container_port)
    {
        return None;
    }
    let ip = probe_ip(container, mapping)?;
    Some(ProbeKey {
        container_id: container.id.clone(),
        address: SocketAddr::new(ip, mapping.host_port),
        container_port: mapping.container_port,
    })
}

fn is_denied(image: &str, container_port: u16) -> bool {
    let image = image.to_ascii_lowercase();
    NEVER_PROBED_IMAGES
        .iter()
        .any(|denied| image.contains(denied))
        || NON_WEB_CONTAINER_PORTS.contains(&container_port)
}

enum BindAddress {
    LocalV4,
    LocalV6,
    Specific(IpAddr),
}

fn bind_address(host_ip: Option<&str>) -> Option<BindAddress> {
    let value = host_ip.map(str::trim).unwrap_or_default();
    if value.is_empty() || value == "*" || value.eq_ignore_ascii_case("localhost") {
        return Some(BindAddress::LocalV4);
    }
    match value.parse::<IpAddr>().ok()? {
        IpAddr::V4(ip) if ip.is_unspecified() || ip.is_loopback() => Some(BindAddress::LocalV4),
        IpAddr::V6(ip) if ip.is_unspecified() || ip.is_loopback() => Some(BindAddress::LocalV6),
        ip => Some(BindAddress::Specific(ip)),
    }
}

/// Wildcard and loopback bindings are probed on IPv4 loopback, or on IPv6 loopback when the host
/// port is bound only on IPv6, so a port listed for both families is probed once.
fn probe_ip(container: &ContainerInfo, mapping: &PublishedPortMapping) -> Option<IpAddr> {
    match bind_address(mapping.host_ip.as_deref())? {
        BindAddress::LocalV4 => Some(Ipv4Addr::LOCALHOST.into()),
        BindAddress::Specific(ip) => Some(ip),
        BindAddress::LocalV6 => {
            let bound_on_v4 = container.port_mappings.iter().any(|other| {
                other.host_port == mapping.host_port
                    && other.protocol.eq_ignore_ascii_case("tcp")
                    && matches!(
                        bind_address(other.host_ip.as_deref()),
                        Some(BindAddress::LocalV4)
                    )
            });
            Some(if bound_on_v4 {
                Ipv4Addr::LOCALHOST.into()
            } else {
                Ipv6Addr::LOCALHOST.into()
            })
        }
    }
}

enum ReadOutcome {
    Data(Vec<u8>),
    /// The peer closed or reset the connection, or writing to it failed.
    Closed,
    TimedOut,
}

fn read_reply(stream: &mut TcpStream, timeout: Duration) -> ReadOutcome {
    if stream.set_read_timeout(Some(timeout)).is_err() {
        return ReadOutcome::Closed;
    }
    let mut buffer = [0; 512];
    match stream.read(&mut buffer) {
        Ok(0) => ReadOutcome::Closed,
        Ok(count) => ReadOutcome::Data(buffer[..count].to_vec()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            ReadOutcome::TimedOut
        }
        Err(_) => ReadOutcome::Closed,
    }
}

fn send_and_read(stream: &mut TcpStream, request: &[u8], timeout: Duration) -> ReadOutcome {
    if stream.write_all(request).is_err() {
        return ReadOutcome::Closed;
    }
    read_reply(stream, timeout)
}

/// Classifies the server behind `address` without completing any protocol exchange.
///
/// Servers that speak first or close at once (docker-proxy with nothing listening in the container)
/// are not web servers. A plain `HEAD` request then identifies HTTP; a 400 reply, a closed
/// connection or a TLS alert leads to a canned ClientHello on a new connection to detect HTTPS.
fn probe(address: SocketAddr, timeouts: ProbeTimeouts) -> Option<WebScheme> {
    let mut stream = TcpStream::connect_timeout(&address, timeouts.connect).ok()?;
    match read_reply(&mut stream, timeouts.passive) {
        ReadOutcome::TimedOut => {}
        ReadOutcome::Data(_) | ReadOutcome::Closed => return None,
    }
    let plain_http_rejected = match send_and_read(&mut stream, HEAD_REQUEST, timeouts.reply) {
        // TLS servers reject plaintext at once, so a peer that stays silent is not TLS either.
        ReadOutcome::TimedOut => return None,
        ReadOutcome::Closed => false,
        ReadOutcome::Data(reply) => match http_status(&reply) {
            Some(400) => true,
            Some(_) => return Some(WebScheme::Http),
            None if reply.first() == Some(&0x15) => false,
            None => return None,
        },
    };
    drop(stream);

    let tls_reply = TcpStream::connect_timeout(&address, timeouts.connect)
        .ok()
        .map(|mut stream| send_and_read(&mut stream, CLIENT_HELLO, timeouts.reply));
    match tls_reply {
        Some(ReadOutcome::Data(reply)) if is_tls_server_reply(&reply) => Some(WebScheme::Https),
        _ => plain_http_rejected.then_some(WebScheme::Http),
    }
}

/// Parses the status code of an HTTP/1.x status line such as `HTTP/1.1 200 OK`.
fn http_status(reply: &[u8]) -> Option<u16> {
    let rest = reply.strip_prefix(b"HTTP/")?;
    let space = rest.iter().position(|&byte| byte == b' ')?;
    let code = rest.get(space + 1..space + 4)?;
    std::str::from_utf8(code).ok()?.parse().ok()
}

/// A TLS handshake record holding a ServerHello (or HelloRetryRequest), or a TLS alert.
fn is_tls_server_reply(reply: &[u8]) -> bool {
    matches!(reply, [0x16, 0x03, _, _, _, 0x02, ..] | [0x15, 0x03, ..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const TEST_TIMEOUTS: ProbeTimeouts = ProbeTimeouts {
        connect: Duration::from_millis(300),
        passive: Duration::from_millis(50),
        reply: Duration::from_millis(300),
    };

    /// Serves `connections` connections on a loopback port, handing each to `handle`.
    fn serve(connections: usize, handle: impl Fn(usize, TcpStream) + Send + 'static) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for index in 0..connections {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                handle(index, stream);
            }
        });
        address
    }

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut buffer = [0; 1024];
        let count = stream.read(&mut buffer).unwrap_or(0);
        buffer[..count].to_vec()
    }

    #[test]
    fn an_http_server_is_http() {
        let address = serve(1, |_, mut stream| {
            assert!(read_request(&mut stream).starts_with(b"HEAD / HTTP/1.0\r\n"));
            stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
        });
        assert_eq!(probe(address, TEST_TIMEOUTS), Some(WebScheme::Http));
    }

    #[test]
    fn a_server_rejecting_plain_http_that_answers_a_client_hello_is_https() {
        let address = serve(2, |index, mut stream| {
            let request = read_request(&mut stream);
            if index == 0 {
                stream
                    .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
                    .unwrap();
            } else {
                assert_eq!(request, CLIENT_HELLO);
                stream
                    .write_all(&[0x16, 0x03, 0x03, 0x00, 0x7a, 0x02, 0x00, 0x00, 0x76])
                    .unwrap();
            }
        });
        assert_eq!(probe(address, TEST_TIMEOUTS), Some(WebScheme::Https));
    }

    #[test]
    fn a_server_that_closes_after_plain_http_and_alerts_on_a_client_hello_is_https() {
        let address = serve(2, |index, mut stream| {
            read_request(&mut stream);
            if index == 1 {
                stream
                    .write_all(&[0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x28])
                    .unwrap();
            }
        });
        assert_eq!(probe(address, TEST_TIMEOUTS), Some(WebScheme::Https));
    }

    #[test]
    fn an_http_400_without_tls_stays_http() {
        let address = serve(2, |_, mut stream| {
            read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
                .unwrap();
        });
        assert_eq!(probe(address, TEST_TIMEOUTS), Some(WebScheme::Http));
    }

    #[test]
    fn a_server_that_speaks_first_is_not_web() {
        let address = serve(1, |_, mut stream| {
            stream.write_all(b"220 mail.test ESMTP\r\n").unwrap();
            thread::sleep(Duration::from_millis(500));
        });
        assert_eq!(probe(address, TEST_TIMEOUTS), None);
    }

    #[test]
    fn a_connection_closed_right_away_is_not_web() {
        let address = serve(1, |_, stream| drop(stream));
        assert_eq!(probe(address, TEST_TIMEOUTS), None);
    }

    #[test]
    fn a_silent_listener_is_not_web() {
        let address = serve(1, |_, stream| {
            thread::sleep(Duration::from_millis(600));
            drop(stream);
        });
        assert_eq!(probe(address, TEST_TIMEOUTS), None);
    }

    #[test]
    fn a_non_http_reply_is_not_web() {
        let address = serve(1, |_, mut stream| {
            read_request(&mut stream);
            stream.write_all(b"ACK\n").unwrap();
        });
        assert_eq!(probe(address, TEST_TIMEOUTS), None);
    }

    #[test]
    fn a_refused_port_is_not_web() {
        let address = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        assert_eq!(probe(address, TEST_TIMEOUTS), None);
    }

    #[test]
    fn reads_http_status_codes() {
        assert_eq!(http_status(b"HTTP/1.1 404 Not Found\r\n"), Some(404));
        assert_eq!(http_status(b"HTTP/1.0 400 Bad Request"), Some(400));
        assert_eq!(http_status(b"HTTP/1.1"), None);
        assert_eq!(http_status(b"SSH-2.0-OpenSSH"), None);
    }

    #[test]
    fn client_hello_lengths_are_consistent() {
        let length =
            |at: usize| usize::from(CLIENT_HELLO[at]) << 8 | usize::from(CLIENT_HELLO[at + 1]);
        assert_eq!(&CLIENT_HELLO[..3], &[0x16, 0x03, 0x01]);
        assert_eq!(length(3), CLIENT_HELLO.len() - 5);
        assert_eq!(CLIENT_HELLO[5], 0x01);
        assert_eq!(length(7), CLIENT_HELLO.len() - 9);

        let session_id = 9 + 2 + 32;
        assert_eq!(CLIENT_HELLO[session_id], 0);
        let suites = session_id + 1;
        let compression = suites + 2 + length(suites);
        assert_eq!(&CLIENT_HELLO[compression..compression + 2], &[0x01, 0x00]);
        let extensions = compression + 2;
        assert_eq!(length(extensions), CLIENT_HELLO.len() - extensions - 2);

        let mut offset = extensions + 2;
        let mut types = Vec::new();
        while offset < CLIENT_HELLO.len() {
            types.push(length(offset));
            offset += 4 + length(offset + 2);
        }
        assert_eq!(offset, CLIENT_HELLO.len());
        assert_eq!(types, [0x00, 0x0a, 0x0b, 0x0d, 0x2b, 0x2d, 0x33]);
    }

    fn container(
        image: &str,
        state: &str,
        mappings: &[(Option<&str>, u16, u16, &str)],
    ) -> ContainerInfo {
        ContainerInfo {
            id: "c".repeat(64),
            name: "web".into(),
            image: image.into(),
            state: state.into(),
            status: "Up".into(),
            ports: vec![],
            port_mappings: mappings
                .iter()
                .map(
                    |&(host_ip, host_port, container_port, protocol)| PublishedPortMapping {
                        host_ip: host_ip.map(str::to_string),
                        host_port,
                        container_port,
                        protocol: protocol.into(),
                        web_scheme: None,
                    },
                )
                .collect(),
            compose_project: None,
            compose_service: None,
            compose_working_dir: None,
        }
    }

    fn addresses(container: &ContainerInfo) -> Vec<Option<SocketAddr>> {
        container
            .port_mappings
            .iter()
            .map(|mapping| probe_key(container, mapping).map(|key| key.address))
            .collect()
    }

    #[test]
    fn probes_wildcard_and_loopback_bindings_on_loopback_once_per_host_port() {
        let both = container(
            "nginx",
            "running",
            &[
                (Some("0.0.0.0"), 8080, 80, "tcp"),
                (Some("::"), 8080, 80, "tcp"),
            ],
        );
        let v4: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        assert_eq!(addresses(&both), [Some(v4), Some(v4)]);

        for host_ip in [
            None,
            Some(""),
            Some("*"),
            Some("localhost"),
            Some("127.0.0.2"),
        ] {
            let single = container("nginx", "running", &[(host_ip, 8080, 80, "tcp")]);
            assert_eq!(addresses(&single), [Some(v4)], "{host_ip:?}");
        }

        let v6: SocketAddr = "[::1]:8080".parse().unwrap();
        for host_ip in ["::", "::1", "0:0:0:0:0:0:0:1"] {
            let single = container("nginx", "running", &[(Some(host_ip), 8080, 80, "tcp")]);
            assert_eq!(addresses(&single), [Some(v6)], "{host_ip}");
        }
    }

    #[test]
    fn probes_specific_bindings_on_their_address_and_skips_malformed_ones() {
        let explicit = container(
            "nginx",
            "running",
            &[
                (Some("192.168.1.20"), 8080, 80, "tcp"),
                (Some("fd00::5"), 8081, 80, "tcp"),
                (Some("http://example.test"), 8082, 80, "tcp"),
            ],
        );
        assert_eq!(
            addresses(&explicit),
            [
                Some("192.168.1.20:8080".parse().unwrap()),
                Some("[fd00::5]:8081".parse().unwrap()),
                None
            ]
        );
    }

    #[test]
    fn never_probes_ryuk_known_non_web_ports_udp_or_stopped_containers() {
        let ryuk = container(
            "docker.io/testcontainers/ryuk:0.11.0",
            "running",
            &[(Some("0.0.0.0"), 32768, 8080, "tcp")],
        );
        let services = container(
            "postgres:16",
            "running",
            &[
                (Some("0.0.0.0"), 15432, 5432, "tcp"),
                (Some("0.0.0.0"), 1025, 1025, "tcp"),
                (Some("0.0.0.0"), 5353, 80, "udp"),
            ],
        );
        let stopped = container("nginx", "exited", &[(Some("0.0.0.0"), 8080, 80, "tcp")]);
        assert_eq!(addresses(&ryuk), [None]);
        assert_eq!(addresses(&services), [None, None, None]);
        assert_eq!(addresses(&stopped), [None]);
    }

    fn key(port: u16) -> ProbeKey {
        ProbeKey {
            container_id: "c".repeat(64),
            address: SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
            container_port: port,
        }
    }

    #[test]
    fn schedules_each_key_once_while_its_probe_runs_and_keeps_web_verdicts() {
        let mut cache = ProbeCache::default();
        let start = Instant::now();
        let keys = HashSet::from([key(8080)]);

        assert_eq!(cache.schedule(&keys, start), [key(8080)]);
        assert!(cache
            .schedule(&keys, start + Duration::from_secs(10))
            .is_empty());

        cache.record(&key(8080), Some(WebScheme::Https), start);
        assert_eq!(cache.verdict(&key(8080)), Some(WebScheme::Https));
        assert!(cache
            .schedule(&keys, start + Duration::from_secs(3600))
            .is_empty());
        assert_eq!(cache.verdict(&key(8080)), Some(WebScheme::Https));
    }

    #[test]
    fn retries_ports_that_were_not_web_with_growing_delays() {
        let mut cache = ProbeCache::default();
        let keys = HashSet::from([key(8080)]);
        let mut now = Instant::now();
        assert_eq!(cache.schedule(&keys, now), [key(8080)]);

        for delay in [2, 5, 15, 30, 60, 60] {
            cache.record(&key(8080), None, now);
            assert_eq!(cache.verdict(&key(8080)), None);
            let almost = now + Duration::from_secs(delay) - Duration::from_millis(1);
            assert!(cache.schedule(&keys, almost).is_empty(), "{delay}");
            now += Duration::from_secs(delay);
            assert_eq!(cache.schedule(&keys, now), [key(8080)], "{delay}");
        }

        cache.record(&key(8080), Some(WebScheme::Http), now);
        assert_eq!(cache.verdict(&key(8080)), Some(WebScheme::Http));
    }

    #[test]
    fn forgets_keys_that_are_no_longer_listed_even_while_probing() {
        let mut cache = ProbeCache::default();
        let now = Instant::now();
        cache.schedule(&HashSet::from([key(8080), key(9090)]), now);
        cache.record(&key(9090), Some(WebScheme::Http), now);

        assert_eq!(
            cache.schedule(&HashSet::from([key(7070)]), now),
            [key(7070)]
        );
        assert_eq!(cache.verdict(&key(9090)), None);
        cache.record(&key(8080), Some(WebScheme::Http), now);
        assert_eq!(cache.verdict(&key(8080)), None);
        assert_eq!(cache.entries.len(), 1);

        assert_eq!(
            cache.schedule(&HashSet::from([key(9090)]), now),
            [key(9090)]
        );
    }

    #[test]
    fn annotates_mappings_from_cached_verdicts_without_waiting() {
        let probes = PortProbes::default();
        let mut listing = ContainerListing {
            available: true,
            containers: vec![container(
                "nginx",
                "running",
                &[(Some("0.0.0.0"), 1, 80, "tcp"), (Some("::"), 1, 80, "tcp")],
            )],
            message: None,
        };
        let probed = probe_key(
            &listing.containers[0],
            &listing.containers[0].port_mappings[0],
        )
        .unwrap();
        {
            let mut cache = lock(&probes.cache);
            cache.schedule(&HashSet::from([probed.clone()]), Instant::now());
            cache.record(&probed, Some(WebScheme::Http), Instant::now());
        }

        probes.annotate(&mut listing);
        assert!(listing.containers[0]
            .port_mappings
            .iter()
            .all(|mapping| mapping.web_scheme == Some(WebScheme::Http)));
    }
}
