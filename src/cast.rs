//! Casting to Google Cast speakers (Google Home, Nest, Chromecast). Experimental.
//!
//! - Discovery: one mDNS question (`_googlecast._tcp`) and its replies, parsed here;
//!   speaker groups included.
//! - Audio: a Cast speaker plays media from a URL, so `Session` runs a small HTTP server
//!   that streams the live input as endless 16-bit stereo WAV, and tells the speaker (over
//!   the Cast v2 protocol, `Channel`, to its Default Media Receiver) to play that URL. The
//!   speaker connects back to this PC, so the firewall must allow incoming connections.
//! - Everything is blocking code on its own threads: one for discovery, two per session
//!   (the HTTP stream and the Cast control channel).

use crate::Shared;
use ringbuf::HeapCons;
use ringbuf::traits::Consumer;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A speaker (or speaker group) found on the network.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub name: String,
    pub ip: IpAddr,
    pub port: u16,
    /// A speaker group set up in the Google Home app.
    pub group: bool,
}

impl Device {
    /// Identifies the cast target in `Running::out_id`, next to cpal device IDs.
    pub fn id(&self) -> String {
        format!("cast:{}", self.name)
    }
}

/// Results of the latest search, filled in by a background thread.
#[derive(Default)]
pub struct Discovery {
    pub devices: Mutex<Vec<Device>>,
    pub searching: AtomicBool,
}

/// How long a search listens; speakers usually answer within half a second.
const SEARCH: Duration = Duration::from_secs(4);
/// Without a request from the speaker by then, something (usually the firewall) blocks it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 64 unpredictable bits from the standard library's randomly keyed hasher, so no crate
/// is needed.
fn secret() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new().build_hasher().finish()
}

impl Discovery {
    /// Starts a search unless one is running; calls `changed` whenever the list changes.
    pub fn search(self: &Arc<Self>, changed: impl Fn() + Send + 'static) {
        if self.searching.swap(true, Ordering::Relaxed) {
            return;
        }
        let this = self.clone();
        std::thread::spawn(move || {
            if let Ok(socket) = query_socket() {
                let deadline = Instant::now() + SEARCH;
                let mut next_query = Instant::now();
                let mut packet = [0u8; 9000];
                while Instant::now() < deadline {
                    // Asked a few times: replies can get lost, and the firewall only lets
                    // replies in for a few seconds after each question.
                    if Instant::now() >= next_query {
                        let _ = socket.send_to(&QUERY, ("224.0.0.251", 5353));
                        next_query += Duration::from_secs(1);
                    }
                    let Ok((n, from)) = socket.recv_from(&mut packet) else { continue };
                    for dev in parse_reply(&packet[..n], from.ip()) {
                        if add(&mut this.devices.lock().unwrap(), dev) {
                            changed();
                        }
                    }
                }
            }
            this.searching.store(false, Ordering::Relaxed);
            changed();
        });
    }
}

/// mDNS question for Cast devices: PTR `_googlecast._tcp.local`.
const QUERY: [u8; 40] = *b"\0\0\0\0\0\x01\0\0\0\0\0\0\x0b_googlecast\x04_tcp\x05local\0\0\x0c\0\x01";

/// A socket for asking from a random port ("legacy unicast", RFC 6762 6.7): speakers reply
/// straight to it, so nothing needs to listen on port 5353 or join the multicast group.
/// It's bound to the adapter that carries internet traffic, which sends the question there
/// rather than to a virtual adapter (Hyper-V, VPN).
// shortcut: only that one adapter is searched; search all adapters if speakers on a second network are needed.
fn query_socket() -> std::io::Result<UdpSocket> {
    let probe = UdpSocket::bind(("0.0.0.0", 0))?;
    probe.connect(("8.8.8.8", 53))?; // no packet is sent; this only picks the route
    let socket = UdpSocket::bind((probe.local_addr()?.ip(), 0))?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    Ok(socket)
}

/// The Cast devices in one mDNS reply: SRV gives the port (a group's differs from its host
/// speaker's), TXT the name (`fn`) and model (`md`), A the address (else the sender's).
fn parse_reply(p: &[u8], from: IpAddr) -> Vec<Device> {
    use std::collections::HashMap;
    let (mut srv, mut txt, mut addr) = (HashMap::new(), HashMap::new(), HashMap::new());
    let count = |i: usize| p.get(i..i + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]) as usize);
    if p.len() < 12 || p[2] & 0x80 == 0 {
        return vec![]; // not a response
    }
    let mut i = 12;
    for _ in 0..count(4) {
        let Some((_, end)) = dns_name(p, i) else { return vec![] };
        i = end + 4; // type, class
    }
    for _ in 0..count(6) + count(8) + count(10) {
        let Some((owner, end)) = dns_name(p, i) else { break };
        let (kind, len) = (count(end), count(end + 8));
        let Some(data) = p.get(end + 10..end + 10 + len) else { break };
        match kind {
            1 if len == 4 => _ = addr.insert(owner, IpAddr::from([data[0], data[1], data[2], data[3]])),
            16 => _ = txt.insert(owner, txt_pairs(data)),
            33 if len > 6 => {
                if let Some((target, _)) = dns_name(p, end + 16) {
                    srv.insert(owner, (target, u16::from_be_bytes([data[4], data[5]])));
                }
            }
            _ => {}
        }
        i = end + 10 + len;
    }
    let mut found: Vec<_> = srv
        .into_iter()
        .filter(|(instance, _)| instance.ends_with("._googlecast._tcp.local"))
        .map(|(instance, (target, port))| {
            let t = txt.remove(&instance).unwrap_or_default();
            let get = |k: &str| t.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
            let name = get("fn").unwrap_or_else(|| instance.split('.').next().unwrap_or_default().to_owned());
            let group = get("md").as_deref() == Some("Google Cast Group");
            Device { name, ip: addr.get(&target).copied().unwrap_or(from), port, group }
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// A DNS name at `i`, following compression pointers; also returns where it ends at `i`.
fn dns_name(p: &[u8], mut i: usize) -> Option<(String, usize)> {
    let (mut name, mut end) = (String::new(), None);
    for _ in 0..64 {
        let len = *p.get(i)? as usize;
        if len & 0xC0 == 0xC0 {
            end.get_or_insert(i + 2);
            i = (len & 0x3F) << 8 | *p.get(i + 1)? as usize;
        } else if len == 0 {
            return Some((name, end.unwrap_or(i + 1)));
        } else {
            if !name.is_empty() {
                name.push('.');
            }
            name.push_str(&String::from_utf8_lossy(p.get(i + 1..i + 1 + len)?));
            i += 1 + len;
        }
    }
    None // a pointer loop
}

/// TXT record strings as `key=value` pairs.
fn txt_pairs(mut data: &[u8]) -> Vec<(String, String)> {
    let mut pairs = vec![];
    while let Some((&len, rest)) = data.split_first() {
        let (s, rest) = rest.split_at(rest.len().min(len as usize));
        let s = String::from_utf8_lossy(s);
        if let Some((k, v)) = s.split_once('=') {
            pairs.push((k.to_owned(), v.to_owned()));
        }
        data = rest;
    }
    pairs
}

/// Adds a newly found device unless one with that name is listed; keeps the list sorted,
/// speakers first, then groups.
fn add(list: &mut Vec<Device>, dev: Device) -> bool {
    if list.iter().any(|x| x.name == dev.name) {
        return false;
    }
    list.push(dev);
    list.sort_by(|a, b| (a.group, &a.name).cmp(&(b.group, &b.name)));
    true
}

/// A running cast. Dropping it stops the stream and the speaker's playback.
pub struct Session {
    stop: Arc<AtomicBool>,
    ended: Arc<Mutex<Option<String>>>,
}

impl Session {
    /// Streams `ich`-channel audio at `rate` from `cons` to `dev`, applying `gain`.
    pub fn start(
        dev: &Device,
        rate: u32,
        ich: usize,
        cons: HeapCons<f32>,
        gain: Arc<AtomicU32>,
        shared: Arc<Shared>,
    ) -> Result<Self, String> {
        let e = |e: std::io::Error| e.to_string();
        // Serve on the local address that routes to the speaker.
        let udp = UdpSocket::bind(("0.0.0.0", 0)).map_err(e)?;
        udp.connect((dev.ip, dev.port)).map_err(e)?;
        let listener = TcpListener::bind((udp.local_addr().map_err(e)?.ip(), 0)).map_err(e)?;
        listener.set_nonblocking(true).map_err(e)?;
        // A secret path, so other devices on the network can't listen in or take the stream over.
        let path = format!("/nymplayer-{:016x}.wav", secret());
        let url = format!("http://{}{path}", listener.local_addr().map_err(e)?);

        let stop = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(Mutex::new(None));
        let fetched = Arc::new(AtomicBool::new(false));
        let (s, f) = (stop.clone(), fetched.clone());
        std::thread::spawn(move || serve(listener, &path, rate, ich, cons, gain, shared, s, f));

        let (dev, s, end) = (dev.clone(), stop.clone(), ended.clone());
        std::thread::spawn(move || {
            let reason = control(&dev, &url, &s, &fetched);
            if !s.load(Ordering::Relaxed) {
                *end.lock().unwrap() = Some(reason);
            }
        });
        Ok(Self { stop, ended })
    }

    /// Why the cast stopped on its own (speaker stopped it, error), if it did.
    pub fn ended(&self) -> Option<String> {
        self.ended.lock().unwrap().clone()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

const NS_CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
const NS_HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const NS_RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
const NS_MEDIA: &str = "urn:x-cast:com.google.cast.media";
/// Google's Default Media Receiver, the player app on every Cast device.
const MEDIA_APP: &str = "CC1AD845";

/// Starts playback on the speaker and watches it until `stop` or the speaker ends it.
/// Returns the reason it ended (empty when stopped by `stop`).
fn control(dev: &Device, url: &str, stop: &AtomicBool, fetched: &AtomicBool) -> String {
    run(dev, url, stop, fetched).unwrap_or_else(|e| format!("Error: casting to {} failed: {e}", dev.name))
}

fn run(dev: &Device, url: &str, stop: &AtomicBool, fetched: &AtomicBool) -> std::io::Result<String> {
    let mut ch = Channel::open(dev)?;
    ch.send("receiver-0", NS_CONNECTION, r#"{"type":"CONNECT"}"#)?;
    ch.send("receiver-0", NS_RECEIVER, &format!(r#"{{"type":"LAUNCH","appId":"{MEDIA_APP}","requestId":1}}"#))?;
    let launching = Instant::now();
    let (transport, session) = loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(String::new());
        }
        if launching.elapsed() > CONNECT_TIMEOUT {
            return Err(std::io::Error::other("the speaker didn't start its media player"));
        }
        if let Some((ns, json)) = ch.recv()?
            && ns == NS_RECEIVER
            && let Some(app) = media_app(&json)
        {
            break app;
        }
    };
    ch.send(&transport, NS_CONNECTION, r#"{"type":"CONNECT"}"#)?;
    let url = url.replace('"', "");
    let load = format!(
        r#"{{"type":"LOAD","requestId":2,"autoplay":true,"media":{{"contentId":"{url}","contentType":"audio/wav","streamType":"LIVE","metadata":{{"metadataType":0,"title":"NymPlayer"}}}}}}"#
    );
    ch.send(&transport, NS_MEDIA, &load)?;
    let started = Instant::now();
    let stop_app = |ch: &mut Channel| {
        let json = format!(r#"{{"type":"STOP","requestId":3,"sessionId":"{session}"}}"#);
        let _ = ch.send("receiver-0", NS_RECEIVER, &json);
    };
    loop {
        if stop.load(Ordering::Relaxed) {
            stop_app(&mut ch);
            return Ok(String::new());
        }
        if !fetched.load(Ordering::Relaxed) && started.elapsed() > CONNECT_TIMEOUT {
            stop_app(&mut ch);
            return Ok(format!(
                "Error: {} didn't connect to NymPlayer; allow NymPlayer through the firewall",
                dev.name
            ));
        }
        if let Some((ns, json)) = ch.recv()?
            && let Some(reason) = ended_by(&ns, &json, &session, &dev.name)
        {
            return Ok(reason);
        }
    }
}

/// Whether this message from the speaker ends the cast, and why.
fn ended_by(ns: &str, json: &str, session: &str, name: &str) -> Option<String> {
    let kind = json_str(json, "type")?;
    match (ns, kind) {
        (NS_MEDIA, "LOAD_FAILED") => Some(format!("Error: {name} couldn't play the stream")),
        // IDLE without a reason is the state before playback starts.
        (NS_MEDIA, "MEDIA_STATUS") => match json_str(json, "idleReason")? {
            "ERROR" => Some(format!("Error: {name} couldn't play the stream")),
            _ => Some(format!("{name} stopped casting")),
        },
        // Our media player was closed, e.g. "Hey Google, stop".
        (NS_RECEIVER, "RECEIVER_STATUS") if !json.contains(session) => Some(format!("{name} stopped casting")),
        (NS_CONNECTION, "CLOSE") => Some(format!("{name} stopped casting")),
        _ => None,
    }
}

/// The media player's transport ID and session ID, once a RECEIVER_STATUS lists it.
fn media_app(json: &str) -> Option<(String, String)> {
    if json_str(json, "type")? != "RECEIVER_STATUS" {
        return None;
    }
    // The fields after its appId belong to the same app entry.
    let app = &json[json.find(&format!(r#""appId":"{MEDIA_APP}""#))?..];
    Some((json_str(app, "transportId")?.to_owned(), json_str(app, "sessionId")?.to_owned()))
}

/// The first string value of `key` in compact JSON, as Cast devices send it. Enough for
/// the few IDs and states read here, whose values have no escapes.
fn json_str<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let rest = json[json.find(&format!(r#""{key}":"#))? + key.len() + 3..].trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

/// A Cast v2 connection: TLS to the speaker, each message a 4-byte length and a protobuf
/// `CastMessage` carrying JSON text. Answers the speaker's heartbeat pings.
struct Channel {
    tls: rustls::StreamOwned<rustls::ClientConnection, TcpStream>,
    inbox: Vec<u8>,
    pinged: Instant,
}

impl Channel {
    fn open(dev: &Device) -> std::io::Result<Self> {
        let other = std::io::Error::other;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(other)?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SelfSigned(provider)))
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::IpAddress(dev.ip.into());
        let mut conn = rustls::ClientConnection::new(Arc::new(config), name).map_err(other)?;
        let mut tcp = TcpStream::connect_timeout(&(dev.ip, dev.port).into(), Duration::from_secs(5))?;
        tcp.set_read_timeout(Some(Duration::from_secs(5)))?;
        while conn.is_handshaking() {
            conn.complete_io(&mut tcp)?;
        }
        // Short from now on, so `recv` returns often enough to check for Stop.
        tcp.set_read_timeout(Some(Duration::from_millis(100)))?;
        Ok(Self { tls: rustls::StreamOwned::new(conn, tcp), inbox: vec![], pinged: Instant::now() })
    }

    fn send(&mut self, dest: &str, ns: &str, json: &str) -> std::io::Result<()> {
        let msg = cast_message("sender-0", dest, ns, json);
        self.tls.write_all(&(msg.len() as u32).to_be_bytes())?;
        self.tls.write_all(&msg)?;
        self.tls.flush()
    }

    /// The next message (namespace, JSON) if one arrives within the read timeout.
    fn recv(&mut self) -> std::io::Result<Option<(String, String)>> {
        // The speaker drops connections that stay quiet for long.
        if self.pinged.elapsed() > Duration::from_secs(5) {
            self.pinged = Instant::now();
            self.send("receiver-0", NS_HEARTBEAT, r#"{"type":"PING"}"#)?;
        }
        loop {
            if let Some(len) = self.inbox.get(..4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
                && self.inbox.len() >= 4 + len
            {
                let msg: Vec<u8> = self.inbox.drain(..4 + len).skip(4).collect();
                let Some((ns, json)) = read_cast_message(&msg) else { continue };
                if ns == NS_HEARTBEAT && json_str(&json, "type") == Some("PING") {
                    self.send("receiver-0", NS_HEARTBEAT, r#"{"type":"PONG"}"#)?;
                    continue;
                }
                return Ok(Some((ns, json)));
            }
            let mut buf = [0u8; 4096];
            match self.tls.read(&mut buf) {
                Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.inbox.extend(&buf[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return Ok(None),
                Err(e) => return Err(e),
            }
        }
    }
}

/// Cast devices use self-signed certificates, so there's no chain to check (Google's own
/// senders don't either); the handshake signature is still verified.
#[derive(Debug)]
struct SelfSigned(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for SelfSigned {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// A protobuf `CastMessage`: protocol version 0, source, destination, namespace, payload
/// type 0 (text) and the JSON payload (fields 1 to 6).
fn cast_message(source: &str, dest: &str, ns: &str, json: &str) -> Vec<u8> {
    let mut m = vec![1 << 3, 0];
    for (field, s) in [(2u8, source), (3, dest), (4, ns)] {
        text_field(&mut m, field, s);
    }
    m.extend([5 << 3, 0]);
    text_field(&mut m, 6, json);
    m
}

fn text_field(m: &mut Vec<u8>, field: u8, s: &str) {
    m.push(field << 3 | 2);
    let mut n = s.len();
    while n >= 0x80 {
        m.push(n as u8 | 0x80);
        n >>= 7;
    }
    m.push(n as u8);
    m.extend(s.as_bytes());
}

/// Namespace and JSON payload of a protobuf `CastMessage`; other fields are skipped.
fn read_cast_message(mut m: &[u8]) -> Option<(String, String)> {
    fn varint(m: &mut &[u8]) -> Option<usize> {
        let mut v = 0usize;
        for shift in (0..64).step_by(7) {
            let (&b, rest) = m.split_first()?;
            *m = rest;
            v |= ((b & 0x7f) as usize) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    let (mut ns, mut json) = (None, None);
    while !m.is_empty() {
        let key = varint(&mut m)?;
        match key & 7 {
            0 => _ = varint(&mut m)?,
            2 => {
                let len = varint(&mut m)?;
                let (value, rest) = (m.get(..len)?, m.get(len..)?);
                m = rest;
                match key >> 3 {
                    4 => ns = Some(String::from_utf8_lossy(value).into_owned()),
                    6 => json = Some(String::from_utf8_lossy(value).into_owned()),
                    _ => {}
                }
            }
            _ => return None,
        }
    }
    Some((ns?, json?))
}

/// The HTTP side: answers each request for `path` with an endless WAV stream of the live
/// input. A new request takes over from the previous one.
// shortcut: no drift correction between the input clock and the speaker's; its buffer absorbs it, add rate matching if long casts glitch.
#[allow(clippy::too_many_arguments)]
fn serve(
    listener: TcpListener,
    path: &str,
    rate: u32,
    ich: usize,
    mut cons: HeapCons<f32>,
    gain: Arc<AtomicU32>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    fetched: Arc<AtomicBool>,
) {
    let mut conn: Option<TcpStream> = None;
    let mut samples = vec![0.0f32; 4096 * ich];
    let mut bytes = Vec::with_capacity(4096 * 4);
    let mut heard = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if let Ok((c, _)) = listener.accept()
            && let Ok(c) = read_request(c, path)
        {
            // Both before the headers go out: once the client has them, it may already be
            // waiting for audio pushed after them.
            fetched.store(true, Ordering::Relaxed);
            cons.clear(); // start from live audio, not what piled up while waiting
            conn = send_headers(c, rate).ok();
        }
        let n = crate::pop_or_silence(&mut cons, &mut samples, &mut heard, rate, ich, conn.is_some());
        let Some(c) = conn.as_mut().filter(|_| n > 0) else {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        };
        let g = f32::from_bits(gain.load(Ordering::Relaxed));
        bytes.clear();
        let mut peak = 0.0f32;
        // The input pushes whole frames, so `n` is a multiple of `ich`.
        for frame in samples[..n].chunks(ich) {
            for ch in 0..2 {
                let x = (frame[ch.min(ich - 1)] * g).clamp(-1.0, 1.0);
                peak = peak.max(x.abs());
                bytes.extend(crate::to_i16(x).to_le_bytes());
            }
        }
        shared.out_peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
        if c.write_all(&bytes).is_err() {
            conn = None;
        }
    }
}

/// Reads the request and accepts it only if it's for `path` (dropping `c` otherwise).
fn read_request(mut c: TcpStream, path: &str) -> std::io::Result<TcpStream> {
    c.set_nonblocking(false)?;
    c.set_read_timeout(Some(Duration::from_secs(2)))?;
    // A speaker that stops reading would otherwise block the stream thread forever.
    c.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut req = [0u8; 4096];
    let n = c.read(&mut req)?;
    // Request line: "GET <path> HTTP/1.1".
    let target = std::str::from_utf8(&req[..n]).ok().and_then(|r| r.split(' ').nth(1));
    if target != Some(path) {
        return Err(ErrorKind::NotFound.into());
    }
    Ok(c)
}

/// Sends the headers of the endless WAV stream.
fn send_headers(mut c: TcpStream, rate: u32) -> std::io::Result<TcpStream> {
    c.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n")?;
    c.write_all(&wav_header(rate, 2))?;
    Ok(c)
}

/// 16-bit PCM WAV header for a stream of unknown length (sizes set near the maximum).
fn wav_header(rate: u32, channels: u16) -> Vec<u8> {
    let data = u32::MAX - 64;
    let align = channels * 2;
    let mut h = Vec::with_capacity(44);
    h.extend(b"RIFF");
    h.extend((data + 36).to_le_bytes());
    h.extend(b"WAVEfmt ");
    h.extend(16u32.to_le_bytes());
    h.extend(1u16.to_le_bytes()); // PCM
    h.extend(channels.to_le_bytes());
    h.extend(rate.to_le_bytes());
    h.extend((rate * align as u32).to_le_bytes());
    h.extend(align.to_le_bytes());
    h.extend(16u16.to_le_bytes());
    h.extend(b"data");
    h.extend(data.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::HeapRb;
    use ringbuf::traits::{Producer, Split};

    /// A reply laid out like a speaker group's: the question, then PTR, SRV (with a
    /// compressed target), TXT and A records.
    #[test]
    fn parses_an_mdns_reply() {
        let mut p = vec![0, 0, 0x84, 0, 0, 1, 0, 1, 0, 0, 0, 3];
        p.extend(&QUERY[12..]); // the question, `_googlecast._tcp.local` at offset 12
        let name = |p: &mut Vec<u8>, labels: &[&str], tail: u16| {
            labels.iter().for_each(|l| {
                p.push(l.len() as u8);
                p.extend(l.as_bytes());
            });
            p.extend((0xC000 | tail).to_be_bytes());
        };
        let record = |p: &mut Vec<u8>, kind: u16, data: &[u8]| {
            p.extend(kind.to_be_bytes());
            p.extend([0x80, 1, 0, 0, 0, 120]); // class IN (cache flush), TTL
            p.extend((data.len() as u16).to_be_bytes());
            p.extend(data);
        };
        // PTR -> "Google-Cast-Group-1._googlecast._tcp.local"
        let mut ptr = vec![];
        name(&mut ptr, &["Google-Cast-Group-1"], 12);
        p.extend([0xC0, 12]);
        record(&mut p, 12, &ptr);
        let instance = p.len() - ptr.len(); // the instance name inside the PTR data
        // SRV: port 32109, target "host-1.local" (written out; ends in a pointer to "local")
        let local = 12 + 1 + 11 + 1 + 4; // "local" inside the question name
        let mut srv = vec![0, 0, 0, 0, 0x7d, 0x6d];
        name(&mut srv, &["host-1"], local as u16);
        p.extend((0xC000 | instance as u16).to_be_bytes());
        record(&mut p, 33, &srv);
        let target = p.len() - srv.len() + 6;
        // TXT: id=..., fn=Kök Och Matrum, md=Google Cast Group
        let mut txt = vec![];
        for s in ["id=abc", "fn=Kök Och Matrum", "md=Google Cast Group"] {
            txt.push(s.len() as u8);
            txt.extend(s.as_bytes());
        }
        p.extend((0xC000 | instance as u16).to_be_bytes());
        record(&mut p, 16, &txt);
        // A: host-1.local -> 192.168.1.122
        p.extend((0xC000 | target as u16).to_be_bytes());
        record(&mut p, 1, &[192, 168, 1, 122]);

        let found = parse_reply(&p, [10, 0, 0, 1].into());
        let ip: IpAddr = [192, 168, 1, 122].into();
        assert_eq!(found, [Device { name: "Kök Och Matrum".into(), ip, port: 32109, group: true }]);
        // Not a response (a question from someone else): ignored.
        assert!(parse_reply(&QUERY, ip).is_empty());
        // Truncated or garbage packets don't panic.
        for n in 0..p.len() {
            parse_reply(&p[..n], ip);
        }
    }

    #[test]
    fn cast_message_round_trip() {
        let m = cast_message("sender-0", "receiver-0", NS_RECEIVER, &"x".repeat(300));
        assert_eq!(&m[..4], [0x08, 0, 0x12, 8]); // version 0, then source "sender-0"
        assert_eq!(read_cast_message(&m), Some((NS_RECEIVER.to_owned(), "x".repeat(300))));
        assert_eq!(read_cast_message(&m[..m.len() - 1]), None); // truncated
    }

    /// As a Nest Hub sends it: a backdrop app listed before the media player.
    #[test]
    fn finds_the_media_player_session() {
        let status = r#"{"type":"RECEIVER_STATUS","requestId":1,"status":{"applications":[{"appId":"E8C28D3C","sessionId":"backdrop","transportId":"backdrop-t"},{"appId":"CC1AD845","appType":"WEB","displayName":"Default Media Receiver","namespaces":[{"name":"urn:x-cast:com.google.cast.media"}],"sessionId":"abc-123","statusText":"Ready","transportId":"abc-t"}],"volume":{"level":0.5}}}"#;
        assert_eq!(media_app(status), Some(("abc-t".into(), "abc-123".into())));
        assert_eq!(media_app(r#"{"type":"RECEIVER_STATUS","status":{"applications":[]}}"#), None);
    }

    #[test]
    fn recognises_the_end_of_a_cast() {
        let ended = |ns, json| ended_by(ns, json, "abc-123", "Kontor");
        let idle = r#"{"type":"MEDIA_STATUS","status":[{"playerState":"IDLE","idleReason":"CANCELLED"}]}"#;
        assert_eq!(ended(NS_MEDIA, idle).as_deref(), Some("Kontor stopped casting"));
        let error = r#"{"type":"MEDIA_STATUS","status":[{"playerState":"IDLE","idleReason":"ERROR"}]}"#;
        assert_eq!(ended(NS_MEDIA, error).as_deref(), Some("Error: Kontor couldn't play the stream"));
        let playing = r#"{"type":"MEDIA_STATUS","status":[{"playerState":"PLAYING"}]}"#;
        assert_eq!(ended(NS_MEDIA, playing), None);
        let closed = r#"{"type":"RECEIVER_STATUS","status":{"applications":[]}}"#;
        assert_eq!(ended(NS_RECEIVER, closed).as_deref(), Some("Kontor stopped casting"));
        let ours = r#"{"type":"RECEIVER_STATUS","status":{"applications":[{"sessionId":"abc-123"}]}}"#;
        assert_eq!(ended(NS_RECEIVER, ours), None);
    }

    #[test]
    fn group_and_its_host_speaker_are_both_listed() {
        let at =
            |name: &str, port| Device { name: name.into(), ip: [192, 168, 1, 122].into(), port, group: port != 8009 };
        let mut list = vec![];
        assert!(add(&mut list, at("Matrum", 8009)));
        assert!(add(&mut list, at("Kök Och Matrum", 32109))); // a group at the same IP, other port
        assert!(add(&mut list, at("Bibliotek", 8009)));
        assert!(!add(&mut list, at("Matrum", 8009))); // repeated answer
        let names: Vec<_> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Bibliotek", "Matrum", "Kök Och Matrum"]); // groups last
    }

    /// With no input at all, the speaker still gets (silent) audio after `GAP`.
    #[test]
    fn sends_silence_while_the_input_is_quiet() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let (_prod, cons) = HeapRb::<f32>::new(1024).split();
        let gain = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let (stop, fetched) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let s = stop.clone();
        let server =
            std::thread::spawn(move || serve(listener, "/s.wav", 8000, 1, cons, gain, Arc::default(), s, fetched));
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"GET /s.wav HTTP/1.1\r\n\r\n").unwrap();
        let mut got: Vec<u8> = vec![];
        // Headers (~100 bytes) + WAV header (44) + 100 ms of 8 kHz stereo silence (3200).
        while got.len() < 3000 {
            let mut b = [0u8; 4096];
            let n = c.read(&mut b).unwrap();
            got.extend(&b[..n]);
        }
        assert!(got[got.len() - 1000..].iter().all(|&b| b == 0));
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }

    #[test]
    fn wav_header_layout() {
        let h = wav_header(48000, 2);
        assert_eq!(h.len(), 44);
        assert_eq!(&h[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 48000);
        assert_eq!(u32::from_le_bytes(h[28..32].try_into().unwrap()), 48000 * 4);
        assert_eq!(&h[36..40], b"data");
    }

    /// Streams to a local "speaker": the response is the WAV header, then mono input as
    /// 16-bit stereo with gain applied.
    #[test]
    fn serves_live_wav_over_http() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let (mut prod, cons) = HeapRb::<f32>::new(1024).split();
        let gain = Arc::new(AtomicU32::new(0.5f32.to_bits()));
        let (stop, fetched) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (s, f) = (stop.clone(), fetched.clone());
        let server = std::thread::spawn(move || serve(listener, "/s.wav", 44100, 1, cons, gain, Arc::default(), s, f));

        // Someone guessing the port but not the secret path gets nothing.
        let mut other = TcpStream::connect(addr).unwrap();
        other.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        other.write_all(b"GET /nymplayer.wav HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(other.read(&mut [0u8; 16]).unwrap_or(0), 0);
        assert!(!fetched.load(Ordering::Relaxed));

        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap(); // fail rather than hang
        c.write_all(b"GET /s.wav HTTP/1.1\r\n\r\n").unwrap();
        let mut head = vec![0u8; 4096];
        let mut got = 0;
        // Headers and the WAV header arrive first; audio pushed after that is streamed.
        while !head[..got].windows(4).any(|w| w == b"data") {
            got += c.read(&mut head[got..]).unwrap();
        }
        assert!(fetched.load(Ordering::Relaxed));
        assert!(String::from_utf8_lossy(&head[..got]).starts_with("HTTP/1.1 200 OK"));
        let pos = head[..got].windows(4).position(|w| w == b"data").unwrap() + 8;
        let mut audio = head[pos..got].to_vec();
        prod.push_slice(&[1.0, -1.0]);
        while audio.len() < 8 {
            let mut b = [0u8; 64];
            let n = c.read(&mut b).unwrap();
            audio.extend(&b[..n]);
            // A slow machine may get silence (sent after `GAP`) before the pushed audio.
            while audio.len() >= 4 && audio[..4] == [0; 4] {
                audio.drain(..4);
            }
        }
        let s: Vec<i16> = audio[..8].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(s, [16383, 16383, -16383, -16383]);
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
