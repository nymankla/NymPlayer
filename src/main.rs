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
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

mod cast;
mod system;

/// Empty selection means "follow the system default device".
const SYSTEM_DEFAULT: &str = "";
/// Input selection prefix for an output device's loopback (what the PC plays through it),
/// followed by that output device's ID.
const LOOPBACK: &str = "loopback:";

fn main() -> eframe::Result {
    let Some(show_requests) = system::single_instance() else {
        return Ok(()); // another copy is running and has been asked to show itself
    };
    // Only hide at start while tray mode is on; otherwise there'd be no way back.
    let start_hidden = std::env::args().any(|a| a == system::TRAY_FLAG) && system::autostart();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([500.0, 355.0])
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap()),
        ..Default::default()
    };
    eframe::run_native(
        "NymPlayer",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.all_styles_mut(apply_theme);
            if start_hidden {
                // eframe always shows the window after its first frame, so minimize it
                // instead; the minimize-to-tray logic then hides it.
                cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            // Without a tray icon the app still works; it just minimizes normally. The tray
            // is still created when tray mode is off: its channel also carries Show requests
            // from a second copy of the app.
            let tray = Tray::new(&cc.egui_ctx, show_requests).ok();
            Ok(Box::new(App::new(tray)))
        }),
    )
}

/// Requests from outside the egui loop: the tray icon and its menu, or a second copy of
/// the app being started.
enum Cmd {
    Show,
    TogglePlay,
    Quit,
}

struct Tray {
    icon: TrayIcon,
    play_item: MenuItem,
    cmds: mpsc::Receiver<Cmd>,
}

impl Tray {
    fn new(ctx: &egui::Context, show_requests: system::ShowRequests) -> Result<Self, String> {
        let e = |e: &dyn std::fmt::Display| e.to_string();
        let (tx, cmds) = mpsc::channel();
        let ctx = ctx.clone();
        // The handlers run outside egui; wake it so `logic` picks the command up, even
        // while the window is hidden.
        let send = move |cmd| {
            let _ = tx.send(cmd);
            ctx.request_repaint();
        };

        let show = MenuItem::new("Show NymPlayer", true, None);
        let play_item = MenuItem::new("Play", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        for item in [&show as &dyn tray_icon::menu::IsMenuItem, &play_item, &PredefinedMenuItem::separator(), &quit] {
            menu.append(item).map_err(|x| e(&x))?;
        }
        let ids = [show.id().clone(), play_item.id().clone(), quit.id().clone()];
        let s = send.clone();
        MenuEvent::set_event_handler(Some(move |ev: MenuEvent| match ids.iter().position(|id| *id == ev.id) {
            Some(0) => s(Cmd::Show),
            Some(1) => s(Cmd::TogglePlay),
            Some(2) => s(Cmd::Quit),
            _ => {}
        }));
        let s = send.clone();
        TrayIconEvent::set_event_handler(Some(move |ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                s(Cmd::Show);
            }
        }));
        show_requests.watch(move || send(Cmd::Show));

        let icon = system::tray_icon()?;
        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("NymPlayer")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|x| e(&x))?;
        Ok(Self { icon: tray, play_item, cmds })
    }
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
    /// Diagnostics for `NYMPLAYER_TRACE`: underruns, input frames dropped because the
    /// buffer was full, xruns reported by cpal, and the smoothed buffer fill in input frames (f32 bits).
    underruns: AtomicU32,
    dropped: AtomicU32,
    xruns: AtomicU32,
    fill: AtomicU32,
}

struct Running {
    _input: cpal::Stream,
    /// A local output device, or else a cast session.
    _output: Option<cpal::Stream>,
    cast: Option<cast::Session>,
    in_id: String,
    out_id: String,
    in_name: String,
    out_name: String,
    in_rate: u32,
    shared: Arc<Shared>,
}

struct App {
    host: cpal::Host,
    inputs: Vec<Entry>,
    outputs: Vec<Entry>,
    settings: Settings,
    /// Output gain as f32 bits, read by the output callback; 0 when muted.
    gain: Arc<AtomicU32>,
    running: Option<Running>,
    status: String,
    last_sync: Instant,
    in_level: f32,
    out_level: f32,
    tray: Option<Tray>,
    /// Window is hidden in the tray.
    hidden: bool,
    /// Registered to start with Windows (the registry is the source of truth).
    autostart: bool,
    /// Casting to this speaker instead of the local output.
    // shortcut: the cast target isn't saved in settings; save it if people want casting to resume at start.
    cast: Option<cast::Device>,
    discovery: Arc<cast::Discovery>,
}

impl App {
    fn new(tray: Option<Tray>) -> Self {
        let autostart = system::autostart();
        if autostart {
            let _ = system::set_autostart(true); // refresh the path in case the exe moved
        }
        let mut app = Self {
            host: cpal::default_host(),
            inputs: vec![],
            outputs: vec![],
            settings: Settings::load(),
            gain: Arc::default(),
            running: None,
            status: "Stopped".into(),
            // Overdue, so the first sync runs right after the first frame.
            last_sync: Instant::now().checked_sub(Duration::from_secs(2)).unwrap_or_else(Instant::now),
            in_level: 0.0,
            out_level: 0.0,
            tray,
            hidden: false,
            autostart,
            cast: None,
            discovery: Arc::default(),
        };
        app.update_gain();
        app.show_tray_icon(autostart);
        app.refresh_devices();
        // Playback resumes (if it was playing when last closed) on the first `logic` pass
        // after the window is drawn, as opening the streams takes ~250 ms.
        if app.settings.playing {
            app.status = "Starting…".into();
            if let Some(t) = &app.tray {
                t.play_item.set_text("Stop");
            }
        }
        app
    }

    /// Tray mode is tied to "Start with Windows": off means no tray icon and a normal
    /// minimize.
    fn show_tray_icon(&self, on: bool) {
        if let Some(t) = &self.tray {
            let _ = t.icon.set_visible(on);
        }
    }

    fn set_playing(&mut self, on: bool) {
        if on != self.settings.playing {
            self.settings.playing = on;
            self.settings.save();
        }
        self.status = "Stopped".into();
        if let Some(t) = &self.tray {
            t.play_item.set_text(if on { "Stop" } else { "Play" });
        }
        self.sync();
    }

    fn update_gain(&self) {
        let s = &self.settings;
        let gain = if s.muted { 0.0 } else { s.volume };
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Refreshes device lists and (re)starts the streams when the device to use has
    /// changed: unplugged, plugged back in, or a new Windows default.
    /// With `NYMPLAYER_TRACE=<file>` set, appends one line per second:
    /// unix time, buffer fill (ms), underruns, dropped input frames, xruns, failed, status.
    fn trace(&self) {
        let Some(path) = std::env::var_os("NYMPLAYER_TRACE") else { return };
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let line = match &self.running {
            Some(r) => {
                let s = &r.shared;
                let fill = f32::from_bits(s.fill.load(Ordering::Relaxed)) * 1000.0 / r.in_rate as f32;
                let (u, d) = (s.underruns.load(Ordering::Relaxed), s.dropped.load(Ordering::Relaxed));
                let x = s.xruns.load(Ordering::Relaxed);
                format!(
                    "{} {fill:.2} {u} {d} {x} {} {}",
                    t.as_secs(),
                    s.failed.load(Ordering::Relaxed) as u8,
                    self.status
                )
            }
            None => format!("{} - - - - - {}", t.as_secs(), self.status),
        };
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }

    /// The cast icon and its menu of speakers. Returns `Some(target)` when the user picks
    /// a speaker, or `Some(None)` for "Stop casting".
    fn cast_menu(&self, ui: &mut egui::Ui) -> Option<Option<cast::Device>> {
        let icon = cast_icon(ui, self.cast.is_some()).on_hover_text("Cast to a Google speaker (experimental)");
        if icon.clicked() {
            let ctx = ui.ctx().clone();
            self.discovery.search(move || ctx.request_repaint());
        }
        let mut pick = None;
        egui::Popup::menu(&icon).show(|ui| {
            ui.set_min_width(220.0);
            ui.label(egui::RichText::new("Cast to").strong());
            let devices = self.discovery.devices.lock().unwrap();
            let searching = self.discovery.searching.load(Ordering::Relaxed);
            // Scrolls once the list would reach the bottom of the window, leaving room for
            // the rows shown below it and the menu's margin.
            let below = 12.0 + if searching { 26.0 } else { 0.0 } + if self.cast.is_some() { 40.0 } else { 0.0 };
            let max = (ui.ctx().content_rect().bottom() - ui.cursor().top() - below).max(80.0);
            // Solid bar: egui's default floating one is invisible until hovered.
            ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
            egui::ScrollArea::vertical().max_height(max).show(ui, |ui| {
                for (i, d) in devices.iter().enumerate() {
                    // Groups are sorted last; a divider separates them from the speakers.
                    if d.group && (i == 0 || !devices[i - 1].group) {
                        ui.separator();
                    }
                    let on = self.cast.as_ref().is_some_and(|c| c.name == d.name);
                    if ui.selectable_label(on, &d.name).clicked() && !on {
                        pick = Some(Some(d.clone()));
                    }
                }
            });
            if searching {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Searching…");
                });
            } else if devices.is_empty() {
                ui.label("No speakers found");
            }
            if self.cast.is_some() {
                ui.separator();
                if ui.button("Stop casting").clicked() {
                    pick = Some(None);
                }
            }
        });
        pick
    }

    fn refresh_devices(&mut self) {
        self.inputs = self.host.input_devices().map(entries).unwrap_or_default();
        self.outputs = self.host.output_devices().map(entries).unwrap_or_default();
        if system::LOOPBACK {
            self.inputs.extend(self.outputs.iter().map(|e| Entry {
                id: format!("{LOOPBACK}{}", e.id),
                name: e.name.clone(),
                // Meaning first, so it survives when a long name is cut short.
                label: format!("What plays on {}", e.label),
            }));
        }
        let s = &mut self.settings;
        if migrate(&mut s.input, &self.inputs) | migrate(&mut s.output, &self.outputs) {
            s.save();
        }
    }

    fn sync(&mut self) {
        self.last_sync = Instant::now();
        self.refresh_devices();
        // The speaker stopped the cast (or it failed): stop rather than suddenly play locally.
        if let Some(why) = self.running.as_ref().and_then(|r| r.cast.as_ref()?.ended()) {
            self.cast = None;
            self.running = None; // before set_playing, whose sync() would see the ended cast again
            self.set_playing(false);
            self.status = why;
            return;
        }
        if !self.settings.playing {
            self.running = None;
            return;
        }
        let s = &self.settings;
        let inp = match s.input.strip_prefix(LOOPBACK) {
            // No fallback: switching to the microphone instead would be a surprise.
            Some(id) => pick(self.host.output_devices().ok(), None, id),
            None => pick(self.host.input_devices().ok(), self.host.default_input_device(), &s.input),
        };
        let out = pick(self.host.output_devices().ok(), self.host.default_output_device(), &s.output);
        let out_id = match &self.cast {
            Some(c) => Some(c.id()),
            None => out.as_ref().and_then(dev_id),
        };
        let want = (inp.as_ref().and_then(dev_id), out_id);
        if loops_back(&self.settings.input, want.1.as_deref()) {
            self.running = None;
            self.status =
                "Error: can't play what the PC plays back through the same speakers; choose another output or cast"
                    .into();
            return;
        }
        let stale = match &self.running {
            Some(r) => {
                r.shared.failed.load(Ordering::Relaxed) || want != (Some(r.in_id.clone()), Some(r.out_id.clone()))
            }
            None => true,
        };
        if !stale {
            return;
        }
        self.running = None; // release the old devices first
        let out = match (&self.cast, &out) {
            (Some(c), _) => Some(Output::Cast(c)),
            (None, Some(o)) => Some(Output::Device(o)),
            (None, None) => None,
        };
        match (inp, out) {
            (Some(i), Some(o)) => match start(&i, o, self.gain.clone()) {
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
        use egui::ViewportCommand as V;
        while let Some(cmd) = self.tray.as_ref().and_then(|t| t.cmds.try_recv().ok()) {
            match cmd {
                Cmd::Show => {
                    ctx.send_viewport_cmd(V::Visible(true));
                    ctx.send_viewport_cmd(V::Minimized(false));
                    ctx.send_viewport_cmd(V::Focus);
                }
                Cmd::TogglePlay => self.set_playing(!self.settings.playing),
                Cmd::Quit => ctx.send_viewport_cmd(V::Close),
            }
        }
        // Minimizing hides the window to the tray. `hidden` only clears once the window
        // reports it's restored, so a stale "minimized" right after Show can't re-hide it.
        let minimized = ctx.input(|i| i.viewport().minimized);
        if self.hidden {
            self.hidden = minimized != Some(false);
        } else if minimized == Some(true) && self.tray.is_some() && self.autostart {
            ctx.send_viewport_cmd(V::Visible(false));
            self.hidden = true;
        }

        // Not before the first frame is drawn, so the window shows without waiting for audio.
        let first_frame = ctx.cumulative_frame_nr() == 0;
        if !first_frame && self.last_sync.elapsed() > Duration::from_secs(1) {
            self.trace();
            self.sync();
        }
        let wait = if first_frame {
            0
        } else if self.running.is_some() && !self.hidden {
            33
        } else {
            1000
        };
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
        let mut toggle = false;
        let mut cast_change = None;
        let live = self.running.is_some();
        egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).inner_margin(18.0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                spinning_record(ui, live);
                ui.label(egui::RichText::new("NymPlayer").size(24.0).strong().color(egui::Color32::WHITE));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    cast_change = self.cast_menu(ui);
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
                    match &self.cast {
                        Some(c) => {
                            ui.label(egui::RichText::new(format!("Casting to {}", c.name)).color(ACCENT));
                        }
                        None => changed |= device_combo(ui, "out", &mut s.output, &self.outputs),
                    }
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
                let (label, fill) = if self.settings.playing {
                    ("⏹  Stop", egui::Color32::from_rgb(70, 70, 80))
                } else {
                    ("▶  Play", ACCENT)
                };
                let text = egui::RichText::new(label).size(18.0).strong().color(egui::Color32::WHITE);
                let button = egui::Button::new(text).fill(fill).corner_radius(18).min_size(egui::vec2(120.0, 36.0));
                toggle = ui.add(button).clicked();
                let dot = if live {
                    GREEN
                } else if self.status.starts_with("Error") {
                    ACCENT
                } else if self.settings.playing {
                    AMBER
                } else {
                    egui::Color32::GRAY
                };
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 5.0, dot);
                ui.add(egui::Label::new(&self.status).truncate());
            });
            ui.add_space(12.0);
            let mut on = self.autostart;
            if let Some(label) = system::AUTOSTART_LABEL
                && ui.checkbox(&mut on, label).changed()
            {
                match system::set_autostart(on) {
                    Ok(()) => {
                        self.autostart = on;
                        self.show_tray_icon(on);
                    }
                    Err(e) => self.status = format!("Error: {e}"),
                }
            }
        });

        if toggle {
            self.set_playing(!self.settings.playing);
        }
        if let Some(target) = cast_change {
            self.cast = target;
            if self.cast.is_some() && !self.settings.playing {
                self.set_playing(true);
            } else {
                self.sync();
            }
        }

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

/// The usual cast symbol (a screen with waves in its corner), painted since the font
/// has no glyph for it. Accent-coloured while casting.
fn cast_icon(ui: &mut egui::Ui, active: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), egui::Sense::click());
    let color = if active {
        ACCENT
    } else if r.hovered() {
        egui::Color32::WHITE
    } else {
        egui::Color32::GRAY
    };
    let stroke = egui::Stroke::new(2.0, color);
    let p = ui.painter();
    let screen = rect.shrink2(egui::vec2(3.0, 5.0));
    let (min, max) = (screen.min, screen.max);
    // Screen outline, open at the bottom left where the waves are.
    p.line(
        vec![egui::pos2(min.x, min.y + 5.0), min, egui::pos2(max.x, min.y), max, egui::pos2(min.x + 9.0, max.y)],
        stroke,
    );
    let corner = egui::pos2(min.x, max.y);
    p.circle_filled(corner + egui::vec2(1.0, -1.0), 2.0, color);
    for radius in [5.5, 9.5] {
        let arc = (0..=8)
            .map(|i| {
                // Quarter circle from straight up to straight right of the corner.
                let a = std::f32::consts::FRAC_PI_2 * i as f32 / 8.0;
                corner + radius * egui::vec2(a.sin(), -a.cos())
            })
            .collect();
        p.line(arc, stroke);
    }
    r
}

/// Segmented LED-style meter, green to amber to red, for a 0..1 level.
fn level_meter(ui: &mut egui::Ui, level: f32) {
    const SEGMENTS: usize = 16;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 16.0), egui::Sense::hover());
    let w = rect.width() / SEGMENTS as f32;
    for i in 0..SEGMENTS {
        let pos = i as f32 / SEGMENTS as f32;
        let color = if pos < 0.65 {
            GREEN
        } else if pos < 0.85 {
            AMBER
        } else {
            ACCENT
        };
        let lit = pos < level;
        let seg =
            egui::Rect::from_min_size(rect.min + egui::vec2(i as f32 * w, 0.0), egui::vec2(w - 2.0, rect.height()));
        ui.painter().rect_filled(seg, 2, if lit { color } else { color.gamma_multiply(0.15) });
    }
}

fn device_combo(ui: &mut egui::Ui, id: &str, sel: &mut String, list: &[Entry]) -> bool {
    let shown = if sel.is_empty() {
        system::DEFAULT_DEVICE_LABEL
    } else {
        list.iter().find(|e| e.id == *sel).map_or("Saved device (not connected)", |e| e.label.as_str())
    };
    let before = sel.clone();
    egui::ComboBox::from_id_salt(id).width(240.0).truncate().selected_text(shown).show_ui(ui, |ui| {
        // Long device names would otherwise run past the window.
        ui.set_max_width(380.0);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.selectable_value(sel, SYSTEM_DEFAULT.to_string(), system::DEFAULT_DEVICE_LABEL);
        for e in list {
            ui.selectable_value(sel, e.id.clone(), &e.label);
        }
    });
    *sel != before
}

/// Device name; `None` if the device vanished (its Display impl would panic then).
fn name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|d| d.name().to_owned())
}

/// Stable device ID, e.g. `wasapi:{0.0.1.00000000}.{guid}`. Windows keeps it across reboots
/// and re-plugging into the same USB port; cpal persists it via Display/FromStr.
fn dev_id(d: &cpal::Device) -> Option<String> {
    d.id().ok().map(|i| i.to_string())
}

/// A device in a dropdown: its ID (what settings store), name, and a label that tells
/// identical devices apart.
#[derive(Debug, PartialEq)]
struct Entry {
    id: String,
    name: String,
    label: String,
}

fn entries(devs: impl Iterator<Item = cpal::Device>) -> Vec<Entry> {
    label_entries(devs.filter_map(|d| Some((dev_id(&d)?, name(&d)?))))
}

/// Numbers repeated names ("USB Codec", "USB Codec (2)") so identical devices differ.
fn label_entries(devs: impl Iterator<Item = (String, String)>) -> Vec<Entry> {
    let mut out: Vec<Entry> = vec![];
    for (id, name) in devs {
        let n = out.iter().filter(|e| e.name == name).count();
        let label = if n == 0 { name.clone() } else { format!("{name} ({})", n + 1) };
        out.push(Entry { id, name, label });
    }
    out
}

/// Settings from before device IDs hold a device name: switch to that device's ID once
/// it's present. Returns true if `sel` changed.
fn migrate(sel: &mut String, list: &[Entry]) -> bool {
    if sel.is_empty() || list.iter().any(|e| e.id == *sel) {
        return false;
    }
    match list.iter().find(|e| e.name == *sel) {
        Some(e) => {
            *sel = e.id.clone();
            true
        }
        None => false,
    }
}

/// Whether `input` captures the very output it would play to, which would echo endlessly.
/// `out_id` is the output's device ID (a cast target's never matches).
fn loops_back(input: &str, out_id: Option<&str>) -> bool {
    input.strip_prefix(LOOPBACK).is_some_and(|id| out_id == Some(id))
}

/// The selected device (by ID) if present, otherwise the Windows default.
fn pick(
    devs: Option<impl Iterator<Item = cpal::Device>>,
    default: Option<cpal::Device>,
    sel: &str,
) -> Option<cpal::Device> {
    if sel.is_empty() {
        return default;
    }
    devs.and_then(|mut d| d.find(|d| dev_id(d).as_deref() == Some(sel))).or(default)
}

/// Where the audio goes.
enum Output<'a> {
    Device(&'a cpal::Device),
    Cast(&'a cast::Device),
}

fn start(inp: &cpal::Device, out: Output, gain: Arc<AtomicU32>) -> Result<Running, String> {
    let e = |e: cpal::Error| e.to_string();
    // An output device here is a loopback input; it only has an output format.
    let icfg = inp.default_input_config().or_else(|_| inp.default_output_config()).map_err(e)?;
    let ich = icfg.channels() as usize;
    let in_rate = icfg.sample_rate();
    let frames = match out {
        // Headroom only: the converter holds the fill near its target, so this rarely fills up.
        Output::Device(_) => in_rate as usize * TARGET_MS / 1000 * 5,
        // The network side reads in bursts; 2 s rides out a slow Wi-Fi moment.
        Output::Cast(_) => in_rate as usize * 2,
    };
    let shared = Arc::new(Shared::default());
    let (prod, cons) = HeapRb::<f32>::new(frames * ich).split();

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

    let (output, cast, out_id, out_name) = match out {
        Output::Device(o) => {
            let stream = build_output(o, ich, in_rate, cons, gain, shared.clone())?;
            (Some(stream), None, dev_id(o).unwrap_or_default(), name(o).unwrap_or_default())
        }
        Output::Cast(d) => {
            let session = cast::Session::start(d, in_rate, ich, cons, gain, shared.clone())?;
            (None, Some(session), d.id(), format!("{} (Cast)", d.name))
        }
    };
    input.play().map_err(e)?;
    if let Some(o) = &output {
        o.play().map_err(e)?;
    }
    Ok(Running {
        _input: input,
        _output: output,
        cast,
        in_id: dev_id(inp).unwrap_or_default(),
        out_id,
        in_name: name(inp).unwrap_or_default(),
        out_name,
        in_rate,
        shared,
    })
}

/// Local output stream: resamples the input to the device's rate and applies the gain.
fn build_output(
    out: &cpal::Device,
    ich: usize,
    in_rate: u32,
    mut cons: ringbuf::HeapCons<f32>,
    gain: Arc<AtomicU32>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, String> {
    let e = |e: cpal::Error| e.to_string();
    let ocfg = out.default_output_config().map_err(e)?;
    let och = ocfg.channels() as usize;
    let target = in_rate as usize * TARGET_MS / 1000;
    let mut converter = Converter::new(ich, ocfg.sample_rate() as f64 / in_rate as f64, target)?;
    let s = shared.clone();
    out.build_output_stream(
        ocfg.config(),
        move |data: &mut [f32], _: &_| {
            let primed = converter.primed;
            converter.fill(data, och, &mut cons);
            if primed && !converter.primed {
                s.underruns.fetch_add(1, Ordering::Relaxed);
            }
            s.fill.store((converter.fill as f32).to_bits(), Ordering::Relaxed);
            let g = f32::from_bits(gain.load(Ordering::Relaxed));
            data.iter_mut().for_each(|x| *x *= g);
            s.out_peak.fetch_max(peak(data).to_bits(), Ordering::Relaxed);
        },
        fail(shared),
        None,
    )
    .map_err(e)
}

/// Error callback: marks the streams for a restart, except for errors cpal reports while
/// the stream keeps running. A USB input reported an xrun every few seconds after ~5 min,
/// and restarting for each one was far more audible than the glitch itself.
fn fail(shared: Arc<Shared>) -> impl FnMut(cpal::Error) + Send + 'static {
    use cpal::ErrorKind as K;
    move |e| match e.kind() {
        K::Xrun => _ = shared.xruns.fetch_add(1, Ordering::Relaxed),
        K::DeviceChanged | K::RealtimeDenied => {}
        _ => shared.failed.store(true, Ordering::Relaxed),
    }
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
            shared.dropped.fetch_add(((scratch.len() - n) / ich) as u32, Ordering::Relaxed);
        },
        fail,
        None,
    )
}

/// Buffered input the converter aims to keep: enough to ride out callback jitter.
/// Bluetooth outputs pull in uneven bursts; 20 ms underran about every 30 s.
const TARGET_MS: usize = 40;
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
        let rs =
            Async::<f32>::new_sinc(ratio, 1.01, &params, CHUNK, ich, FixedAsync::Output).map_err(|e| e.to_string())?;
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
        let mut avail = (cons.occupied_len() / ich) as f64;
        if !self.primed {
            if avail < self.target {
                return false;
            }
            // Start at the target: a backlog (e.g. from an output that starts late) would
            // otherwise only drain slowly through the gentle drift correction.
            cons.skip((avail - self.target) as usize * ich);
            avail = self.target;
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

/// Stored as lines in `NymPlayer\settings.txt` under `system::config_dir()` (`%APPDATA%` on Windows): input device ID, output
/// device ID (empty = Windows default; older files hold names, see `migrate`), volume (0..1), muted (0/1), playing (0/1).
struct Settings {
    input: String,
    output: String,
    volume: f32,
    muted: bool,
    /// Playing when last closed; playback resumes on the next start.
    playing: bool,
}

impl Settings {
    fn path() -> Option<std::path::PathBuf> {
        Some(system::config_dir()?.join("NymPlayer").join("settings.txt"))
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
            playing: lines.next() == Some("1"),
        }
    }

    fn save(&self) {
        if let Some(p) = Self::path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let (muted, playing) = (self.muted as u8, self.playing as u8);
            let text = format!("{}\n{}\n{}\n{muted}\n{playing}\n", self.input, self.output, self.volume);
            let _ = std::fs::write(p, text);
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

    /// Runs input through a fresh converter and returns `frames` output frames. The
    /// target is the whole input, so priming doesn't trim any of it.
    fn convert(input: &[f32], ich: usize, ratio: f64, och: usize, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * och];
        Converter::new(ich, ratio, input.len() / ich).unwrap().fill(&mut out, och, &mut buffer(input));
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
        let mut c = Converter::new(1, 44100.0 / 48000.0, 4800).unwrap();
        let mut out = [0.0; 100];
        let mut produced = 0;
        while c.fill(&mut out, 1, &mut cons) {
            produced += out.len();
        }
        // ~4410, minus what the last partial block and the filter hold back.
        assert!((4200..=4410).contains(&produced), "{produced}");
    }

    #[test]
    fn drains_overfull_buffer_faster() {
        let consumed = |target| {
            let mut cons = buffer(&[0.5; 20000]);
            let mut out = vec![0.0; 10000];
            // Already running with this fill (priming would trim it to the target).
            let mut c = Converter::new(1, 1.0, target).unwrap();
            (c.primed, c.fill) = (true, 20000.0);
            c.fill(&mut out, 1, &mut cons);
            20000 - cons.occupied_len()
        };
        // Far above target the ratio is clamped 0.5% low: ~50 extra frames per 10000.
        let (overfull, on_target) = (consumed(32), consumed(20000));
        assert!(overfull > on_target + 30, "{overfull} vs {on_target}");
    }

    #[test]
    fn starts_at_target_after_backlog() {
        let mut cons = buffer(&[0.5; 20000]);
        Converter::new(1, 1.0, 32).unwrap().fill(&mut [0.0; 1], 1, &mut cons);
        assert!(cons.occupied_len() <= 32, "{}", cons.occupied_len());
    }

    #[test]
    fn waits_for_target_fill() {
        let mut out = [0.0; 100];
        assert!(!Converter::new(1, 1.0, 32).unwrap().fill(&mut out, 1, &mut buffer(&[0.5; 10])));
        assert!(out.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn loopback_into_the_same_output_is_refused() {
        let spk = "wasapi:{0.0.0.00000000}.{abc}";
        let looped = format!("{LOOPBACK}{spk}");
        assert!(loops_back(&looped, Some(spk)));
        assert!(!loops_back(&looped, Some("wasapi:{0.0.0.00000000}.{other}")));
        assert!(!loops_back(&looped, Some("cast:Bibliotek")));
        assert!(!loops_back(spk, Some(spk))); // a microphone-style input, not loopback
    }

    #[test]
    fn settings_parse_new_and_old_format() {
        let s = Settings::parse("Mic\nSpeakers\n0.25\n1\n1\n");
        assert_eq!((s.input.as_str(), s.output.as_str(), s.volume, s.muted), ("Mic", "Speakers", 0.25, true));
        assert!(s.playing);
        let old = Settings::parse("Mic\n\n");
        assert_eq!((old.output.as_str(), old.volume, old.muted, old.playing), ("", 1.0, false, false));
    }

    #[test]
    fn identical_devices_get_numbered_labels() {
        let pairs = [("a", "USB Codec"), ("b", "Speakers"), ("c", "USB Codec")];
        let list = label_entries(pairs.iter().map(|(i, n)| (i.to_string(), n.to_string())));
        let labels: Vec<_> = list.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["USB Codec", "Speakers", "USB Codec (2)"]);
    }

    #[test]
    fn old_name_settings_migrate_to_ids() {
        let list = label_entries([("id1".into(), "Mic".into())].into_iter());
        let mut sel = "Mic".to_string();
        assert!(migrate(&mut sel, &list));
        assert_eq!(sel, "id1");
        assert!(!migrate(&mut sel, &list)); // already an ID
        let mut gone = "Unplugged".to_string();
        assert!(!migrate(&mut gone, &list)); // kept until the device shows up
        assert!(!migrate(&mut String::new(), &list)); // Windows default
    }

    #[test]
    fn meter_scale() {
        assert_eq!(meter(1.0), 1.0);
        assert_eq!(meter(0.0), 0.0);
        assert!((meter(0.001) - 0.0).abs() < 1e-5); // -60 dB
    }
}
