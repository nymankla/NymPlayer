//! Casting to Google Cast speakers (Google Home, Nest, Chromecast). Experimental.
//!
//! - Discovery: mDNS (`_googlecast._tcp`) via `oxicast`.
//! - Audio: a Cast speaker plays media from a URL, so `Session` runs a small HTTP server
//!   that streams the live input as endless 16-bit stereo WAV, and tells the speaker (via
//!   `oxicast`, Default Media Receiver) to play that URL. The speaker connects back to this
//!   PC, so the firewall must allow incoming connections to NymPlayer.
//! - `oxicast` is async; each discovery and each session runs a current-thread tokio
//!   runtime on its own thread, so the rest of the app stays synchronous.

use crate::Shared;
use oxicast::{CastApp, CastClient, CastEvent, IdleReason, MediaInfo, MediaMetadata};
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

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime").block_on(f)
}

impl Discovery {
    /// Starts a search unless one is running; calls `changed` whenever the list changes.
    pub fn search(self: &Arc<Self>, changed: impl Fn() + Send + 'static) {
        if self.searching.swap(true, Ordering::Relaxed) {
            return;
        }
        let this = self.clone();
        std::thread::spawn(move || {
            block_on(async {
                if let Ok(mut found) = oxicast::discovery::discover(SEARCH) {
                    while let Some(d) = found.recv().await {
                        let mut list = this.devices.lock().unwrap();
                        if !list.iter().any(|x| x.name == d.name) {
                            list.push(Device { name: d.name, ip: d.ip, port: d.port });
                            list.sort_by(|a, b| a.name.cmp(&b.name));
                            changed();
                        }
                    }
                }
            });
            this.searching.store(false, Ordering::Relaxed);
            changed();
        });
    }
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
        let url = format!("http://{}/nymplayer.wav", listener.local_addr().map_err(e)?);

        let stop = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(Mutex::new(None));
        let fetched = Arc::new(AtomicBool::new(false));
        let (s, f) = (stop.clone(), fetched.clone());
        std::thread::spawn(move || serve(listener, rate, ich, cons, gain, shared, s, f));

        let (dev, s, end) = (dev.clone(), stop.clone(), ended.clone());
        std::thread::spawn(move || {
            let reason = block_on(control(&dev, &url, &s, &fetched));
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

/// Starts playback on the speaker and watches it until `stop` or the speaker ends it.
/// Returns the reason it ended.
async fn control(dev: &Device, url: &str, stop: &AtomicBool, fetched: &AtomicBool) -> String {
    let fail = |e: oxicast::Error| format!("Error: casting to {} failed: {e}", dev.name);
    let client = match CastClient::connect(&dev.ip.to_string(), dev.port).await {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let app = match client.launch_app(&CastApp::DefaultMediaReceiver).await {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    let title = MediaMetadata::Generic { title: Some("NymPlayer".into()), subtitle: None, images: vec![] };
    let media = MediaInfo::live(url, "audio/wav").metadata(title);
    if let Err(e) = client.load_media(&media, true, 0.0, None).await {
        return fail(e);
    }
    let started = Instant::now();
    let reason = loop {
        tokio::select! {
            ev = client.next_event() => match ev {
                Some(CastEvent::MediaSessionEnded { idle_reason: IdleReason::Error, .. }) => {
                    break format!("Error: {} couldn't play the stream", dev.name);
                }
                Some(CastEvent::MediaSessionEnded { .. }) => break format!("{} stopped casting", dev.name),
                Some(CastEvent::Disconnected(_)) | None => break format!("Error: lost connection to {}", dev.name),
                _ => {}
            },
            () = tokio::time::sleep(Duration::from_millis(100)) => {
                if stop.load(Ordering::Relaxed) {
                    let _ = client.stop_app(&app.session_id).await;
                    return String::new();
                }
                if !fetched.load(Ordering::Relaxed) && started.elapsed() > CONNECT_TIMEOUT {
                    let _ = client.stop_app(&app.session_id).await;
                    break format!(
                        "Error: {} didn't connect to NymPlayer; allow NymPlayer through the firewall",
                        dev.name
                    );
                }
            }
        }
    };
    let _ = client.disconnect().await;
    reason
}

/// The HTTP side: answers each request from the speaker with an endless WAV stream of
/// the live input. A new request takes over from the previous one.
#[allow(clippy::too_many_arguments)]
fn serve(
    listener: TcpListener,
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
    while !stop.load(Ordering::Relaxed) {
        if let Ok((c, _)) = listener.accept() {
            // Both before responding: once the client has the headers, it may already be
            // waiting for audio pushed after them.
            fetched.store(true, Ordering::Relaxed);
            cons.clear(); // start from live audio, not what piled up while waiting
            conn = respond(c, rate).ok();
        }
        let n = cons.pop_slice(&mut samples);
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
                bytes.extend(((x * 32767.0) as i16).to_le_bytes());
            }
        }
        shared.out_peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
        if c.write_all(&bytes).is_err() {
            conn = None;
        }
    }
}

/// Reads the request (contents don't matter) and sends the headers of the endless WAV.
fn respond(mut c: TcpStream, rate: u32) -> std::io::Result<TcpStream> {
    c.set_nonblocking(false)?;
    c.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut req = [0u8; 4096];
    match c.read(&mut req) {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {}
        Err(e) => return Err(e),
    }
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
        let server = std::thread::spawn(move || serve(listener, 44100, 1, cons, gain, Arc::default(), s, f));

        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap(); // fail rather than hang
        c.write_all(b"GET /nymplayer.wav HTTP/1.1\r\n\r\n").unwrap();
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
        }
        let s: Vec<i16> = audio[..8].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(s, [16383, 16383, -16383, -16383]);
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
