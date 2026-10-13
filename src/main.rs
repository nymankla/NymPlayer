#![windows_subsystem = "windows"]

use cpal::FromSample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Adjustable, Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Empty selection means "follow the Windows default device".
const WINDOWS_DEFAULT: &str = "";

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([500.0, 300.0])
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap()),
        ..Default::default()
    };
    eframe::run_native(
        "NymPlayer",
        options,
        Box::new(|cc| {
            cc.egui_ctx.all_styles_mut(apply_theme);
            Ok(Box::new(App::new()))
        }),
    )
}

// Colours taken from the icon.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(224, 71, 47);
const WOOD: egui::Color32 = egui::Color32::from_rgb(186, 128, 78);
const BG: egui::Color32 = egui::Color32::from_rgb(24, 24, 28);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(38, 38, 44);
const GREEN: egui::Color32 = egui::Color32::from_rgb(80, 200, 120);
const AMBER: egui::Color32 = egui::Color32::from_rgb(240, 180, 60);

/// Dark theme in the icon's colours, used whatever the Windows theme is.
fn apply_theme(style: &mut egui::Style) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = egui::Color32::from_rgb(16, 16, 18);
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    v.hyperlink_color = WOOD;
    let r = egui::CornerRadius::same(8);
    for (w, fill) in [
        (&mut v.widgets.inactive, SURFACE),
        (&mut v.widgets.hovered, egui::Color32::from_rgb(52, 52, 60)),
        (&mut v.widgets.active, egui::Color32::from_rgb(64, 64, 74)),
        (&mut v.widgets.open, egui::Color32::from_rgb(52, 52, 60)),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.corner_radius = r;
    }
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, WOOD);
    // Slider rail (inactive bg_fill) must stand out from the SURFACE panel it sits on.
    v.widgets.inactive.bg_fill = egui::Color32::from_rgb(70, 70, 80);
    v.slider_trailing_fill = true;
    v.window_corner_radius = r;
    v.menu_corner_radius = r;
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    for font in style.text_styles.values_mut() {
        font.size += 2.0;
    }
}

/// State shared between the audio callbacks and the GUI. The audio itself goes through
/// a lock-free ring buffer owned by the two callbacks.
#[derive(Default)]
struct Shared {
    in_peak: AtomicU32,
    out_peak: AtomicU32,
    failed: AtomicBool,
}

struct Running {
    _input: cpal::Stream,
    _output: cpal::Stream,
    in_name: String,
    out_name: String,
    shared: Arc<Shared>,
}

struct App {
    host: cpal::Host,
    inputs: Vec<String>,
    outputs: Vec<String>,
    settings: Settings,
    /// Output gain as f32 bits, read by the output callback; 0 when muted.
    gain: Arc<AtomicU32>,
    playing: bool,
    running: Option<Running>,
    status: String,
    last_sync: Instant,
    in_level: f32,
    out_level: f32,
}

impl App {
    fn new() -> Self {
        let mut app = Self {
            host: cpal::default_host(),
            inputs: vec![],
            outputs: vec![],
            settings: Settings::load(),
            gain: Arc::default(),
            playing: false,
            running: None,
            status: "Stopped".into(),
            last_sync: Instant::now(),
            in_level: 0.0,
            out_level: 0.0,
        };
        app.update_gain();
        app.sync();
        app
    }

    fn update_gain(&self) {
        let s = &self.settings;
        let gain = if s.muted { 0.0 } else { s.volume };
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Refreshes device lists and (re)starts the streams when the device to use has
    /// changed: unplugged, plugged back in, or a new Windows default.
    fn sync(&mut self) {
        self.last_sync = Instant::now();
        self.inputs = self.host.input_devices().map(names).unwrap_or_default();
        self.outputs = self.host.output_devices().map(names).unwrap_or_default();
        if !self.playing {
            self.running = None;
            return;
        }
        let s = &self.settings;
        let inp = pick(self.host.input_devices().ok(), self.host.default_input_device(), &s.input);
        let out = pick(self.host.output_devices().ok(), self.host.default_output_device(), &s.output);
        let want = (inp.as_ref().and_then(name), out.as_ref().and_then(name));
        let stale = match &self.running {
            Some(r) => {
                r.shared.failed.load(Ordering::Relaxed)
                    || want != (Some(r.in_name.clone()), Some(r.out_name.clone()))
            }
            None => true,
        };
        if !stale {
            return;
        }
        self.running = None; // release the old devices first
        match (inp, out) {
            (Some(i), Some(o)) => match start(&i, &o, self.gain.clone()) {
                Ok(r) => {
                    self.status = format!("Playing: {} to {}", r.in_name, r.out_name);
                    self.running = Some(r);
                }
                Err(e) => self.status = format!("Error: {e}"),
            },
            (None, _) => self.status = "Waiting for input device…".into(),
            (_, None) => self.status = "Waiting for output device…".into(),
        }
    }
}

impl eframe::App for App {
    // Runs even while the window is minimized (unlike `ui`), so device switching keeps working.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_sync.elapsed() > Duration::from_secs(1) {
            self.sync();
        }
        let wait = if self.running.is_some() { 33 } else { 1000 };
        ctx.request_repaint_after(Duration::from_millis(wait));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let (in_peak, out_peak) = match &self.running {
            Some(r) => (take_peak(&r.shared.in_peak), take_peak(&r.shared.out_peak)),
            None => (0.0, 0.0),
        };
        // Instant attack, smooth fall-off.
        self.in_level = in_peak.max(self.in_level * 0.85);
        self.out_level = out_peak.max(self.out_level * 0.85);

        let mut changed = false;
        let mut volume_done = false;
        let live = self.running.is_some();
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(18.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    spinning_record(ui, live);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("NymPlayer").size(24.0).strong().color(egui::Color32::WHITE));
                        ui.label(egui::RichText::new("USB input to any output").color(WOOD));
                    });
                });
                ui.add_space(4.0);
                egui::Frame::new().fill(SURFACE).corner_radius(10).inner_margin(12.0).show(ui, |ui| {
                    egui::Grid::new("devices").num_columns(3).spacing([12.0, 10.0]).show(ui, |ui| {
                        let s = &mut self.settings;
                        ui.label("Input");
                        changed |= device_combo(ui, "in", &mut s.input, &self.inputs);
                        level_meter(ui, meter(self.in_level));
                        ui.end_row();
                        ui.label("Output");
                        changed |= device_combo(ui, "out", &mut s.output, &self.outputs);
                        level_meter(ui, meter(self.out_level));
                        ui.end_row();
                        ui.label("Volume");
                        ui.spacing_mut().slider_width = 180.0;
                        let slider = egui::Slider::new(&mut s.volume, 0.0..=1.0)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0));
                        let r = ui.add_enabled(!s.muted, slider);
                        let mute = ui.selectable_label(s.muted, if s.muted { "Muted" } else { "Mute" });
                        if mute.clicked() {
                            s.muted = !s.muted;
                        }
                        if r.changed() || mute.clicked() {
                            self.update_gain();
                        }
                        // Save once a drag ends rather than on every frame of it.
                        volume_done = r.drag_stopped() || (r.changed() && !r.dragged()) || mute.clicked();
                        ui.end_row();
                    });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let (label, fill) = if self.playing {
                        ("⏹  Stop", egui::Color32::from_rgb(70, 70, 80))
                    } else {
                        ("▶  Play", ACCENT)
                    };
                    let text = egui::RichText::new(label).size(18.0).strong().color(egui::Color32::WHITE);
                    let button = egui::Button::new(text).fill(fill).corner_radius(18).min_size(egui::vec2(120.0, 36.0));
                    if ui.add(button).clicked() {
                        self.playing = !self.playing;
                        self.status = "Stopped".into();
                        changed = true;
                    }
                    let dot = if live {
                        GREEN
                    } else if self.status.starts_with("Error") {
                        ACCENT
                    } else if self.playing {
                        AMBER
                    } else {
                        egui::Color32::GRAY
                    };
                    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 5.0, dot);
                    ui.add(egui::Label::new(&self.status).truncate());
                });
            });

        if changed || volume_done {
            self.settings.save();
        }
        if changed {
            self.sync();
        }
    }
}

/// A little record like the icon's; it spins while audio is flowing.
fn spinning_record(ui: &mut egui::Ui, spinning: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    p.circle_filled(c, 23.0, egui::Color32::from_rgb(20, 20, 22));
    for r in [12.0, 16.0, 20.0] {
        p.circle_stroke(c, r, egui::Stroke::new(1.0, egui::Color32::from_rgb(52, 52, 58)));
    }
    p.circle_filled(c, 8.0, ACCENT);
    // 33⅓ rpm, sped up a little so it reads as motion.
    let angle = if spinning { ui.input(|i| i.time) as f32 * 2.5 } else { 0.0 };
    p.circle_filled(c + 15.0 * egui::vec2(angle.cos(), angle.sin()), 2.0, egui::Color32::from_gray(150));
    p.circle_filled(c, 1.5, egui::Color32::LIGHT_GRAY);
}

/// Segmented LED-style meter, green to amber to red, for a 0..1 level.
fn level_meter(ui: &mut egui::Ui, level: f32) {
    const SEGMENTS: usize = 16;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 16.0), egui::Sense::hover());
    let w = rect.width() / SEGMENTS as f32;
    for i in 0..SEGMENTS {
        let pos = i as f32 / SEGMENTS as f32;
        let color = if pos < 0.65 { GREEN } else if pos < 0.85 { AMBER } else { ACCENT };
        let lit = pos < level;
        let seg = egui::Rect::from_min_size(rect.min + egui::vec2(i as f32 * w, 0.0), egui::vec2(w - 2.0, rect.height()));
        ui.painter().rect_filled(seg, 2, if lit { color } else { color.gamma_multiply(0.15) });
    }
}

fn device_combo(ui: &mut egui::Ui, id: &str, sel: &mut String, list: &[String]) -> bool {
    let shown = if sel.is_empty() { "Windows default" } else { sel.as_str() };
    let before = sel.clone();
    egui::ComboBox::from_id_salt(id)
        .width(240.0)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            ui.selectable_value(sel, WINDOWS_DEFAULT.to_string(), "Windows default");
            for n in list {
                ui.selectable_value(sel, n.clone(), n);
            }
        });
    *sel != before
}

/// Device name; `None` if the device vanished (its Display impl would panic then).
fn name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|d| d.name().to_owned())
}

fn names(devs: impl Iterator<Item = cpal::Device>) -> Vec<String> {
    devs.filter_map(|d| name(&d)).collect()
}

/// The selected device if present, otherwise the Windows default.
fn pick(
    devs: Option<impl Iterator<Item = cpal::Device>>,
    default: Option<cpal::Device>,
    sel: &str,
) -> Option<cpal::Device> {
    if sel.is_empty() {
        return default;
    }
    // shortcut: matched by name, so two identical USB devices are indistinguishable.
    devs.and_then(|mut d| d.find(|d| name(d).as_deref() == Some(sel))).or(default)
}

fn start(inp: &cpal::Device, out: &cpal::Device, gain: Arc<AtomicU32>) -> Result<Running, String> {
    let e = |e: cpal::Error| e.to_string();
    let icfg = inp.default_input_config().map_err(e)?;
    let ich = icfg.channels() as usize;
    let in_rate = icfg.sample_rate();
    let ocfg = out.default_output_config().map_err(e)?;
    let och = ocfg.channels() as usize;
    let target = in_rate as usize * TARGET_MS / 1000;
    let mut converter = Converter::new(ich, ocfg.sample_rate() as f64 / in_rate as f64, target)?;

    let shared = Arc::new(Shared::default());
    // Headroom only: the converter holds the fill near `target`, so this rarely fills up.
    let (prod, mut cons) = HeapRb::<f32>::new(target * 5 * ich).split();

    use cpal::SampleFormat as F;
    let (cfg, s) = (icfg.config(), shared.clone());
    let input = match icfg.sample_format() {
        F::F32 => build_input::<f32>(inp, cfg, prod, s),
        F::I16 => build_input::<i16>(inp, cfg, prod, s),
        F::I32 => build_input::<i32>(inp, cfg, prod, s),
        F::U16 => build_input::<u16>(inp, cfg, prod, s),
        f => return Err(format!("unsupported input format {f}")),
    }
    .map_err(e)?;

    let s = shared.clone();
    let output = out
        .build_output_stream(
            ocfg.config(),
            move |data: &mut [f32], _: &_| {
                converter.fill(data, och, &mut cons);
                let g = f32::from_bits(gain.load(Ordering::Relaxed));
                data.iter_mut().for_each(|x| *x *= g);
                s.out_peak.fetch_max(peak(data).to_bits(), Ordering::Relaxed);
            },
            fail(shared.clone()),
            None,
        )
        .map_err(e)?;

    input.play().map_err(e)?;
    output.play().map_err(e)?;
    Ok(Running {
        _input: input,
        _output: output,
        in_name: name(inp).unwrap_or_default(),
        out_name: name(out).unwrap_or_default(),
        shared,
    })
}

fn fail(shared: Arc<Shared>) -> impl FnMut(cpal::Error) + Send + 'static {
    move |_| shared.failed.store(true, Ordering::Relaxed)
}

/// Input stream for any sample type cpal can convert to f32 (devices that don't capture
/// in f32), pushing into the ring buffer.
fn build_input<T: cpal::SizedSample>(
    dev: &cpal::Device,
    cfg: cpal::StreamConfig,
    mut prod: ringbuf::HeapProd<f32>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, cpal::Error>
where
    f32: cpal::FromSample<T>,
{
    let ich = cfg.channels as usize;
    // Sized for any realistic callback, so the callback never allocates.
    let mut scratch = Vec::<f32>::with_capacity(1 << 16);
    let fail = fail(shared.clone());
    dev.build_input_stream(
        cfg,
        move |data: &[T], _: &_| {
            scratch.clear();
            scratch.extend(data.iter().map(|&x| f32::from_sample_(x)));
            shared.in_peak.fetch_max(peak(&scratch).to_bits(), Ordering::Relaxed);
            // Whole frames only, so channels never get shifted. When full, the newest
            // audio is dropped.
            let n = scratch.len().min(prod.vacant_len() / ich * ich);
            prod.push_slice(&scratch[..n]);
        },
        fail,
        None,
    )
}

/// Buffered input the converter aims to keep: enough to ride out callback jitter.
const TARGET_MS: usize = 20;
/// Output frames per resampler chunk. Small, as it adds to latency.
const CHUNK: usize = 64;

/// The output side of the audio path, run in the output callback:
/// - pulls input frames from the ring buffer
/// - resamples to the output rate (sinc interpolation, via rubato)
/// - nudges that ratio to hold the buffer fill at `target`, so differing input and output
///   clocks never overflow or drain the buffer
/// - maps channels: extra output channels repeat the last input channel, so mono feeds
///   both ears.
struct Converter {
    rs: Async<f32>,
    ich: usize,
    /// Desired and smoothed actual buffer fill, in input frames.
    target: f64,
    fill: f64,
    primed: bool,
    scratch: Vec<f32>,
    /// Resampled frames (interleaved, `ich` channels) still to be played: `pending[pos..]`.
    pending: Vec<f32>,
    pos: usize,
}

impl Converter {
    /// `ratio` is output rate / input rate.
    fn new(ich: usize, ratio: f64, target: usize) -> Result<Self, String> {
        let params = SincInterpolationParameters::new(64, WindowFunction::BlackmanHarris2)
            .oversampling_factor(128)
            .interpolation(SincInterpolationType::Linear);
        let rs = Async::<f32>::new_sinc(ratio, 1.01, &params, CHUNK, ich, FixedAsync::Output)
            .map_err(|e| e.to_string())?;
        let scratch = vec![0.0; rs.input_frames_max() * ich];
        let pending = vec![0.0; rs.output_frames_max() * ich];
        let pos = pending.len();
        Ok(Self { rs, ich, target: target as f64, fill: 0.0, primed: false, scratch, pending, pos })
    }

    /// Fills `out` (`och` channels) from `cons`. `out` arrives silent, so an underrun
    /// leaves silence; the converter then waits for `target` frames before resuming.
    /// Returns false if `out` was not completely filled.
    fn fill(&mut self, out: &mut [f32], och: usize, cons: &mut impl Consumer<Item = f32>) -> bool {
        let ich = self.ich;
        let avail = (cons.occupied_len() / ich) as f64;
        if !self.primed {
            if avail < self.target {
                return false;
            }
            self.primed = true;
            self.fill = avail;
        }
        // Proportional control, smoothed over ~0.5 s of ~10 ms callbacks. Too full means
        // the input is faster: produce slightly fewer output frames per input frame.
        self.fill += (avail - self.fill) * 0.02;
        let err = (self.fill - self.target) / self.target;
        let _ = self.rs.set_resample_ratio_relative((1.0 - 0.002 * err).clamp(0.995, 1.005), true);

        for frame in out.chunks_mut(och) {
            if self.pos == self.pending.len() {
                let need = self.rs.input_frames_next();
                if cons.occupied_len() < need * ich {
                    self.primed = false;
                    return false;
                }
                let input = &mut self.scratch[..need * ich];
                cons.pop_slice(input);
                let input = InterleavedSlice::new(&*input, ich, need).unwrap();
                let frames = self.pending.len() / ich;
                let mut output = InterleavedSlice::new_mut(&mut self.pending[..], ich, frames).unwrap();
                let Ok((_, written)) = self.rs.process_into_buffer(&input, &mut output, None) else {
                    return false;
                };
                // Move the chunk to the end of `pending` so `pos == len` still means empty.
                self.pos = self.pending.len() - written * ich;
                self.pending.copy_within(..written * ich, self.pos);
            }
            for (c, s) in frame.iter_mut().enumerate() {
                *s = self.pending[self.pos + c.min(ich - 1)];
            }
            self.pos += ich;
        }
        true
    }
}

fn peak(data: &[f32]) -> f32 {
    data.iter().fold(0.0, |m, s| m.max(s.abs()))
}

/// Non-negative f32 bit patterns order like the floats, so `fetch_max` on bits works.
fn take_peak(a: &AtomicU32) -> f32 {
    f32::from_bits(a.swap(0, Ordering::Relaxed))
}

/// Peak to 0..1 on a -60..0 dBFS scale.
fn meter(peak: f32) -> f32 {
    ((20.0 * peak.max(1e-6).log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Stored as lines in `%APPDATA%\NymPlayer\settings.txt`: input device name, output
/// device name (empty = Windows default), volume (0..1), muted (0/1).
struct Settings {
    input: String,
    output: String,
    volume: f32,
    muted: bool,
}

impl Settings {
    fn path() -> Option<std::path::PathBuf> {
        Some(std::path::PathBuf::from(std::env::var_os("APPDATA")?).join("NymPlayer").join("settings.txt"))
    }

    fn load() -> Self {
        Self::parse(&Self::path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default())
    }

    fn parse(text: &str) -> Self {
        let mut lines = text.lines();
        Self {
            input: lines.next().unwrap_or_default().to_owned(),
            output: lines.next().unwrap_or_default().to_owned(),
            volume: lines.next().and_then(|v| v.parse().ok()).unwrap_or(1.0f32).clamp(0.0, 1.0),
            muted: lines.next() == Some("1"),
        }
    }

    fn save(&self) {
        if let Some(p) = Self::path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let muted = if self.muted { 1 } else { 0 };
            let _ = std::fs::write(p, format!("{}\n{}\n{}\n{muted}\n", self.input, self.output, self.volume));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(samples: &[f32]) -> ringbuf::HeapCons<f32> {
        let (mut p, c) = HeapRb::new(samples.len()).split();
        p.push_slice(samples);
        c
    }

    /// Runs input through a fresh converter and returns `frames` output frames.
    fn convert(input: &[f32], ich: usize, ratio: f64, och: usize, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * och];
        Converter::new(ich, ratio, 32).unwrap().fill(&mut out, och, &mut buffer(input));
        out
    }

    #[test]
    fn mono_feeds_both_channels() {
        let out = convert(&[0.5; 4000], 1, 1.0, 2, 1000);
        assert!(out[1000..].iter().all(|&x| (x - 0.5).abs() < 0.01), "{:?}", &out[1000..1010]);
    }

    #[test]
    fn stereo_keeps_channels_apart() {
        let out = convert(&[0.5, -0.25].repeat(2000), 2, 1.0, 2, 1000);
        for f in out[1000..].chunks(2) {
            assert!((f[0] - 0.5).abs() < 0.01 && (f[1] + 0.25).abs() < 0.01, "{f:?}");
        }
    }

    #[test]
    fn resamples_48k_to_44k1() {
        let mut cons = buffer(&[0.5; 4800]);
        let mut c = Converter::new(1, 44100.0 / 48000.0, 32).unwrap();
        let mut out = [0.0; 100];
        let mut produced = 0;
        while c.fill(&mut out, 1, &mut cons) {
            produced += out.len();
        }
        // ~4410, minus what the last partial block, the target fill and the filter hold back.
        assert!((4200..=4410).contains(&produced), "{produced}");
    }

    #[test]
    fn drains_overfull_buffer_faster() {
        let consumed = |target| {
            let mut cons = buffer(&[0.5; 20000]);
            let mut out = vec![0.0; 10000];
            Converter::new(1, 1.0, target).unwrap().fill(&mut out, 1, &mut cons);
            20000 - cons.occupied_len()
        };
        // Far above target the ratio is clamped 0.5% low: ~50 extra frames per 10000.
        let (overfull, on_target) = (consumed(32), consumed(20000));
        assert!(overfull > on_target + 30, "{overfull} vs {on_target}");
    }

    #[test]
    fn waits_for_target_fill() {
        let mut out = [0.0; 100];
        assert!(!Converter::new(1, 1.0, 32).unwrap().fill(&mut out, 1, &mut buffer(&[0.5; 10])));
        assert!(out.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn settings_parse_new_and_old_format() {
        let s = Settings::parse("Mic\nSpeakers\n0.25\n1\n");
        assert_eq!((s.input.as_str(), s.output.as_str(), s.volume, s.muted), ("Mic", "Speakers", 0.25, true));
        let old = Settings::parse("Mic\n\n");
        assert_eq!((old.output.as_str(), old.volume, old.muted), ("", 1.0, false));
    }

    #[test]
    fn meter_scale() {
        assert_eq!(meter(1.0), 1.0);
        assert_eq!(meter(0.0), 0.0);
        assert!((meter(0.001) - 0.0).abs() < 1e-5); // -60 dB
    }
}
