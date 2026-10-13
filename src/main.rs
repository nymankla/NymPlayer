#![windows_subsystem = "windows"]

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Empty selection means "follow the Windows default device".
const WINDOWS_DEFAULT: &str = "";

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 280.0])
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
    sel_in: String,
    sel_out: String,
    playing: bool,
    running: Option<Running>,
    status: String,
    last_sync: Instant,
    in_level: f32,
    out_level: f32,
}

impl App {
    fn new() -> Self {
        let (sel_in, sel_out) = load_settings();
        let mut app = Self {
            host: cpal::default_host(),
            inputs: vec![],
            outputs: vec![],
            sel_in,
            sel_out,
            playing: false,
            running: None,
            status: "Stopped".into(),
            last_sync: Instant::now(),
            in_level: 0.0,
            out_level: 0.0,
        };
        app.sync();
        app
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
        let inp = pick(self.host.input_devices().ok(), self.host.default_input_device(), &self.sel_in);
        let out = pick(self.host.output_devices().ok(), self.host.default_output_device(), &self.sel_out);
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
            (Some(i), Some(o)) => match start(&i, &o) {
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.last_sync.elapsed() > Duration::from_secs(1) {
            self.sync();
        }
        let (in_peak, out_peak) = match &self.running {
            Some(r) => (take_peak(&r.shared.in_peak), take_peak(&r.shared.out_peak)),
            None => (0.0, 0.0),
        };
        // Instant attack, smooth fall-off.
        self.in_level = in_peak.max(self.in_level * 0.85);
        self.out_level = out_peak.max(self.out_level * 0.85);

        let mut changed = false;
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
                        ui.label("Input");
                        changed |= device_combo(ui, "in", &mut self.sel_in, &self.inputs);
                        level_meter(ui, meter(self.in_level));
                        ui.end_row();
                        ui.label("Output");
                        changed |= device_combo(ui, "out", &mut self.sel_out, &self.outputs);
                        level_meter(ui, meter(self.out_level));
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

        if changed {
            save_settings(&self.sel_in, &self.sel_out);
            self.sync();
        }
        let wait = if self.running.is_some() { 33 } else { 1000 };
        ui.ctx().request_repaint_after(Duration::from_millis(wait));
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

fn start(inp: &cpal::Device, out: &cpal::Device) -> Result<Running, String> {
    let e = |e: cpal::Error| e.to_string();
    let icfg = inp.default_input_config().map_err(e)?;
    // shortcut: WASAPI shared-mode capture is f32 in practice; add conversion if a device says otherwise.
    if icfg.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!("unsupported input format {}", icfg.sample_format()));
    }
    let ich = icfg.channels() as usize;
    let in_rate = icfg.sample_rate();
    let ocfg = out.default_output_config().map_err(e)?;
    let och = ocfg.channels() as usize;
    let mut resampler = Resampler::new(ich, in_rate as f64 / ocfg.sample_rate() as f64);

    let shared = Arc::new(Shared::default());
    let ms = |n: usize| in_rate as usize * n / 1000 * ich;
    let prime_len = ms(15);
    // 60 ms cap: if the input clock runs faster than the output, the newest audio is
    // dropped once the buffer is full instead of letting latency grow.
    let (mut prod, mut cons) = HeapRb::<f32>::new(ms(60)).split();

    let s = shared.clone();
    let input = inp
        .build_input_stream(
            icfg.config(),
            move |data: &[f32], _: &_| {
                s.in_peak.fetch_max(peak(data).to_bits(), Ordering::Relaxed);
                // Whole frames only, so channels never get shifted.
                let n = data.len().min(prod.vacant_len() / ich * ich);
                prod.push_slice(&data[..n]);
            },
            fail(shared.clone()),
            None,
        )
        .map_err(e)?;

    let s = shared.clone();
    let mut primed = false;
    let output = out
        .build_output_stream(
            ocfg.config(),
            move |data: &mut [f32], _: &_| {
                primed |= cons.occupied_len() >= prime_len;
                if primed {
                    primed = resampler.fill(data, och, &mut cons);
                }
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

/// Linear-interpolating resampler and channel mapper, run in the output callback.
struct Resampler {
    /// Input frames consumed per output frame (in_rate / out_rate).
    step: f64,
    /// Position between `a` (0.0) and `b` (1.0).
    pos: f64,
    a: Vec<f32>,
    b: Vec<f32>,
}

impl Resampler {
    fn new(ich: usize, step: f64) -> Self {
        Self { step, pos: 1.0, a: vec![0.0; ich], b: vec![0.0; ich] }
    }

    /// Fills `out` (`och` channels) from `cons`. Extra output channels repeat the last
    /// input channel, so mono feeds both ears. `out` arrives silent, so an underrun leaves
    /// silence. Returns false on underrun.
    fn fill(&mut self, out: &mut [f32], och: usize, cons: &mut impl Consumer<Item = f32>) -> bool {
        let ich = self.a.len();
        for frame in out.chunks_mut(och) {
            while self.pos >= 1.0 {
                if cons.occupied_len() < ich {
                    return false;
                }
                std::mem::swap(&mut self.a, &mut self.b);
                cons.pop_slice(&mut self.b);
                self.pos -= 1.0;
            }
            let t = self.pos as f32;
            for (c, s) in frame.iter_mut().enumerate() {
                let i = c.min(ich - 1);
                *s = self.a[i] + (self.b[i] - self.a[i]) * t;
            }
            self.pos += self.step;
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

fn settings_path() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("APPDATA")?).join("NymPlayer").join("settings.txt"))
}

/// Line 1: input device name, line 2: output device name (empty = Windows default).
fn load_settings() -> (String, String) {
    let text = settings_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let mut lines = text.lines().map(str::to_owned);
    (lines.next().unwrap_or_default(), lines.next().unwrap_or_default())
}

fn save_settings(inp: &str, out: &str) {
    if let Some(p) = settings_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, format!("{inp}\n{out}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(samples: &[f32]) -> ringbuf::HeapCons<f32> {
        let (mut p, c) = HeapRb::new(64).split();
        p.push_slice(samples);
        c
    }

    // The resampler starts from one silent frame and holds the newest frame as its
    // interpolation target, so output lags input by one frame.
    #[test]
    fn same_rate_mono_to_stereo_and_underrun() {
        let mut cons = buffer(&[0.1, 0.2]);
        let mut out = [0.0; 6];
        let mut r = Resampler::new(1, 1.0);
        assert!(!r.fill(&mut out, 2, &mut cons));
        assert_eq!(out, [0.0, 0.0, 0.1, 0.1, 0.0, 0.0]);
        // The held frame comes out once more input arrives.
        let mut cons = buffer(&[0.3]);
        let mut out = [0.0; 2];
        assert!(r.fill(&mut out, 2, &mut cons));
        assert_eq!(out, [0.2, 0.2]);
    }

    #[test]
    fn same_rate_stereo_keeps_channels() {
        let mut cons = buffer(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7]);
        let mut out = [0.0; 6];
        assert!(Resampler::new(2, 1.0).fill(&mut out, 2, &mut cons));
        assert_eq!(out, [0.0, 0.0, 0.1, 0.2, 0.3, 0.4]);
        assert_eq!(cons.occupied_len(), 1); // half a frame is never consumed
    }

    #[test]
    fn upsample_2x_interpolates() {
        let mut cons = buffer(&[1.0, 3.0]);
        let mut out = [0.0; 4];
        assert!(Resampler::new(1, 0.5).fill(&mut out, 1, &mut cons));
        assert_eq!(out, [0.0, 0.5, 1.0, 2.0]);
    }

    #[test]
    fn downsample_2x_skips_frames() {
        let mut cons = buffer(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let mut out = [0.0; 3];
        assert!(Resampler::new(1, 2.0).fill(&mut out, 1, &mut cons));
        assert_eq!(out, [0.0, 2.0, 4.0]);
    }

    #[test]
    fn meter_scale() {
        assert_eq!(meter(1.0), 1.0);
        assert_eq!(meter(0.0), 0.0);
        assert!((meter(0.001) - 0.0).abs() < 1e-5); // -60 dB
    }
}
