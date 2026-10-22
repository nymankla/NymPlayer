//! Recording the input to a file, untouched by volume and mute (issue #1).
//!
//! The input callback keeps pushing into the lock-free ring buffer; a writer thread drains
//! it to the file, so the audio callback never touches the disk. Formats: 16-bit WAV
//! (`hound`) and 16-bit FLAC (`flacenc`, encoded frame by frame so long recordings don't
//! pile up in memory).

use flacenc::component::{BitRepr, Stream, StreamInfo};
use flacenc::error::Verify;
use flacenc::source::{Fill, FrameBuf};
use ringbuf::HeapCons;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Format {
    Wav,
    Flac,
}

impl Format {
    pub const ALL: [Format; 2] = [Format::Wav, Format::Flac];

    pub fn ext(self) -> &'static str {
        match self {
            Format::Wav => "wav",
            Format::Flac => "flac",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Wav => "WAV (uncompressed)",
            Format::Flac => "FLAC (lossless, about half the size)",
        }
    }

    /// The format a file name asks for, by its extension.
    pub fn of(path: &Path) -> Option<Format> {
        Format::from_ext(path.extension()?.to_str()?)
    }

    pub fn from_ext(ext: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.ext().eq_ignore_ascii_case(ext))
    }
}

/// What to record to: chosen in the save dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub path: PathBuf,
    pub format: Format,
}

impl Target {
    /// Identifies the recording in `Running::out_id`, next to cpal device IDs.
    pub fn id(&self) -> String {
        format!("record:{}", self.path.display())
    }

    pub fn file_name(&self) -> String {
        self.path.file_name().map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }
}

/// A running recording. Dropping it stops recording and finishes the file.
pub struct Recording {
    stop: Arc<AtomicBool>,
    ended: Arc<Mutex<Option<String>>>,
    writer: Option<JoinHandle<()>>,
    pub started: Instant,
}

impl Recording {
    /// Creates the file (so a bad folder fails right away) and starts the writer thread.
    pub fn start(target: &Target, rate: u32, ich: usize, mut cons: HeapCons<f32>) -> Result<Self, String> {
        let mut writer =
            Writer::create(target, rate, ich).map_err(|e| format!("can't create {}: {e}", target.file_name()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(Mutex::new(None));
        let (s, end, name) = (stop.clone(), ended.clone(), target.file_name());
        let thread = std::thread::spawn(move || {
            let mut samples = vec![0.0f32; 4096 * ich];
            let mut heard = Instant::now();
            // Keep writing until stopped, then drain what's left in the buffer.
            let result = loop {
                let stopping = s.load(Ordering::Relaxed);
                // The input pushes whole frames, so this is a multiple of `ich`.
                let n = crate::pop_or_silence(&mut cons, &mut samples, &mut heard, rate, ich, !stopping);
                if n > 0
                    && let Err(e) = writer.write(&samples[..n])
                {
                    break Err(e);
                }
                if stopping && n == 0 {
                    break Ok(());
                }
                if n == 0 {
                    std::thread::sleep(Duration::from_millis(20));
                }
            };
            // Finish the file even after a write error, so what was recorded stays playable.
            let finished = writer.finish();
            if let Err(e) = result.and(finished) {
                *end.lock().unwrap() = Some(format!("Error: recording to {name} stopped: {e}"));
            }
        });
        Ok(Self { stop, ended, writer: Some(thread), started: Instant::now() })
    }

    /// Why the recording stopped on its own (a write error such as a full disk), if it did.
    pub fn ended(&self) -> Option<String> {
        self.ended.lock().unwrap().clone()
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wait for the rest of the buffer and the file header, so the file is complete.
        if let Some(t) = self.writer.take() {
            let _ = t.join();
        }
    }
}

enum Writer {
    Wav(hound::WavWriter<BufWriter<File>>),
    Flac(Box<Flac>),
}

impl Writer {
    fn create(target: &Target, rate: u32, ich: usize) -> std::io::Result<Self> {
        let err = |e: hound::Error| std::io::Error::other(e.to_string());
        Ok(match target.format {
            Format::Wav => {
                let spec = hound::WavSpec {
                    channels: ich as u16,
                    sample_rate: rate,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                Writer::Wav(hound::WavWriter::create(&target.path, spec).map_err(err)?)
            }
            Format::Flac => Writer::Flac(Box::new(Flac::create(&target.path, rate, ich)?)),
        })
    }

    fn write(&mut self, samples: &[f32]) -> std::io::Result<()> {
        match self {
            Writer::Wav(w) => {
                let mut w = w.get_i16_writer(samples.len() as u32);
                samples.iter().for_each(|&x| w.write_sample(crate::to_i16(x)));
                w.flush().map_err(|e| std::io::Error::other(e.to_string()))
            }
            Writer::Flac(f) => f.write(samples),
        }
    }

    // shortcut: WAV stops at 4 GB (about 6 hours of 48 kHz stereo) with an error; add RF64 if longer recordings are needed.
    fn finish(&mut self) -> std::io::Result<()> {
        match self {
            Writer::Wav(w) => w.flush().map_err(|e| std::io::Error::other(e.to_string())),
            Writer::Flac(f) => f.finish(),
        }
    }
}

/// Streaming FLAC: the header first, then each frame as it's encoded; on finish, the
/// header is rewritten with the final sizes and sample count.
struct Flac {
    file: BufWriter<File>,
    info: StreamInfo,
    config: flacenc::error::Verified<flacenc::config::Encoder>,
    block: FrameBuf,
    ich: usize,
    pending: Vec<i32>,
    frames: usize,
}

const FLAC_BLOCK: usize = 4096;

impl Flac {
    fn create(path: &Path, rate: u32, ich: usize) -> std::io::Result<Self> {
        let bad = |e: String| std::io::Error::other(e);
        let info = StreamInfo::new(rate as usize, ich, 16).map_err(|e| bad(e.to_string()))?;
        let config = flacenc::config::Encoder::default().into_verified().map_err(|(_, e)| bad(e.to_string()))?;
        let block = FrameBuf::with_size(ich, FLAC_BLOCK).map_err(|e| bad(e.to_string()))?;
        let mut f =
            Self { file: BufWriter::new(File::create(path)?), info, config, block, ich, pending: vec![], frames: 0 };
        f.write_header()?;
        Ok(f)
    }

    /// "fLaC" and the STREAMINFO block: same length every time, so it can be rewritten.
    fn write_header(&mut self) -> std::io::Result<()> {
        let mut sink = flacenc::bitsink::ByteSink::new();
        Stream::with_stream_info(self.info.clone())
            .write(&mut sink)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        self.file.write_all(sink.as_slice())
    }

    fn write(&mut self, samples: &[f32]) -> std::io::Result<()> {
        self.pending.extend(samples.iter().map(|&x| i32::from(crate::to_i16(x))));
        while self.pending.len() >= FLAC_BLOCK * self.ich {
            let block: Vec<i32> = self.pending.drain(..FLAC_BLOCK * self.ich).collect();
            self.encode(&block)?;
        }
        Ok(())
    }

    fn encode(&mut self, interleaved: &[i32]) -> std::io::Result<()> {
        let bad = |e: String| std::io::Error::other(e);
        self.block.resize(interleaved.len() / self.ich);
        self.block.fill_interleaved(interleaved).map_err(|e| bad(e.to_string()))?;
        let frame = flacenc::encode_fixed_size_frame(&self.config, &self.block, self.frames, &self.info)
            .map_err(|e| bad(e.to_string()))?;
        self.info.update_frame_info(&frame);
        self.frames += 1;
        let mut sink = flacenc::bitsink::ByteSink::new();
        frame.write(&mut sink).map_err(|e| bad(e.to_string()))?;
        self.file.write_all(sink.as_slice())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        if !self.pending.is_empty() {
            // The last frame may be short, but not below FLAC's minimum block size.
            let min = flacenc::constant::MIN_BLOCK_SIZE * self.ich;
            if self.pending.len() < min {
                self.pending.resize(min, 0);
            }
            let block = std::mem::take(&mut self.pending);
            self.encode(&block)?;
        }
        self.file.seek(SeekFrom::Start(0))?;
        self.write_header()?;
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flacenc::component::Decode;
    use ringbuf::HeapRb;
    use ringbuf::traits::{Producer, Split};

    fn tone(frames: usize, ich: usize) -> Vec<f32> {
        (0..frames * ich).map(|i| ((i / ich) as f32 * 0.05).sin() * 0.5).collect()
    }

    /// Records `input` through the real writer thread and returns the file's path.
    fn record(format: Format, input: &[f32], rate: u32, ich: usize) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("nymplayer-test-{}-{format:?}.{}", std::process::id(), format.ext()));
        let (mut prod, cons) = HeapRb::<f32>::new(input.len()).split();
        prod.push_slice(input);
        let rec = Recording::start(&Target { path: path.clone(), format }, rate, ich, cons).unwrap();
        drop(rec); // stops, drains the buffer and finishes the file
        path
    }

    #[test]
    fn wav_recording_holds_the_input() {
        let input = tone(10_000, 2);
        let path = record(Format::Wav, &input, 44100, 2);
        let mut r = hound::WavReader::open(&path).unwrap();
        assert_eq!((r.spec().channels, r.spec().sample_rate, r.spec().bits_per_sample), (2, 44100, 16));
        let got: Vec<i32> = r.samples::<i16>().map(|s| s.unwrap() as i32).collect();
        assert_eq!(got, input.iter().map(|&x| i32::from(crate::to_i16(x))).collect::<Vec<_>>());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn flac_recording_decodes_to_the_input() {
        // Not a multiple of the block size, so the short last frame is covered too.
        let input = tone(FLAC_BLOCK * 2 + 1000, 2);
        let path = record(Format::Flac, &input, 48000, 2);
        let bytes = std::fs::read(&path).unwrap();
        let (_, stream) = flacenc::component::parser::stream::<()>(&bytes).unwrap();
        let info = stream.stream_info();
        assert_eq!((info.sample_rate(), info.channels(), info.bits_per_sample()), (48000, 2, 16));
        assert_eq!(info.total_samples(), input.len() / 2);
        let mut got = vec![];
        for n in 0..stream.frame_count() {
            got.extend(stream.frame(n).unwrap().decode());
        }
        assert_eq!(got, input.iter().map(|&x| i32::from(crate::to_i16(x))).collect::<Vec<_>>());
        std::fs::remove_file(path).unwrap();
    }

    /// No input at all (a quiet loopback input) still records silence, in real time.
    #[test]
    fn records_silence_while_the_input_is_quiet() {
        let path = std::env::temp_dir().join(format!("nymplayer-test-{}-quiet.wav", std::process::id()));
        let (_prod, cons) = HeapRb::<f32>::new(1024).split();
        let rec = Recording::start(&Target { path: path.clone(), format: Format::Wav }, 8000, 1, cons).unwrap();
        std::thread::sleep(Duration::from_millis(450));
        drop(rec);
        let mut r = hound::WavReader::open(&path).unwrap();
        let samples: Vec<i16> = r.samples::<i16>().map(Result::unwrap).collect();
        // About 0.3–0.45 s of 8 kHz mono, depending on timing; all of it silent.
        assert!((2000..=4000).contains(&samples.len()), "{}", samples.len());
        assert!(samples.iter().all(|&s| s == 0));
        drop(r);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn format_follows_the_extension() {
        assert_eq!(Format::of(Path::new("a/take.FLAC")), Some(Format::Flac));
        assert_eq!(Format::of(Path::new("take.wav")), Some(Format::Wav));
        assert_eq!(Format::of(Path::new("take.mp3")), None);
        assert_eq!(Format::of(Path::new("take")), None);
    }

    #[test]
    fn unwritable_folder_fails_at_start() {
        let target = Target { path: PathBuf::from("/no/such/folder/take.wav"), format: Format::Wav };
        let (_, cons) = HeapRb::<f32>::new(16).split();
        assert!(Recording::start(&target, 48000, 2, cons).is_err());
    }
}
