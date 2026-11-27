//! victory-scarlett-play launcher: picks the game folder, checks the PC, runs the first-run setup, plays, updates,
//! and (opt-in) sends reports.
#![windows_subsystem = "windows"]

mod config;
mod game;
mod hardware;
mod jobs;
mod presets;
mod report;
mod update;

use config::{Config, Lighting};
use eframe::egui::{self, Color32, RichText};
use hardware::Hardware;
use std::sync::{Arc, Mutex};

/// `Launcher.exe --setup [--game <folder>] [--no-record]`: the first-run setup without the window (for scripts and
/// tests); progress goes to `logs\setup-cli.log`. Exit code 0 when it finished.
fn setup_cli(args: &[String]) -> ! {
    use std::io::Write;
    let _ = std::fs::create_dir_all(config::logs_dir());
    let mut out = std::fs::File::create(config::logs_dir().join("setup-cli.log")).ok();
    let mut say = |s: &str| {
        if let Some(f) = out.as_mut() {
            let _ = writeln!(f, "{s}");
        }
    };
    let mut cfg = Config::load();
    if let Some(i) = args.iter().position(|a| a == "--game")
        && let Some(dir) = args.get(i + 1)
    {
        let dir = std::path::PathBuf::from(dir);
        if let Err(e) = game::check_game_dir(&dir) {
            say(&e);
            std::process::exit(2);
        }
        cfg.game_dir = Some(dir);
        cfg.save();
    }
    let hw = Hardware::detect();
    for f in hw.findings() {
        say(&format!("{} {}", if f.blocking { "BLOCKING:" } else { "note:" }, f.text));
    }
    let (Some(exe), Some(_)) = (game::victory_exe(), cfg.game_dir.as_ref()) else {
        say("victory.exe or the game folder is missing");
        std::process::exit(2);
    };
    if hw.findings().iter().any(|f| f.blocking) {
        std::process::exit(2);
    }
    let job = jobs::setup(exe.clone(), cfg.clone(), hw, !args.iter().any(|a| a == "--no-record"));
    let mut last = String::new();
    loop {
        let st = job.status.lock().unwrap();
        let line = format!("{} | {} | {:.0}%", st.step, st.detail, st.progress.max(0.0) * 100.0);
        if line != last {
            say(&line);
            last = line;
        }
        if st.done {
            if let Some(e) = &st.error {
                say(&format!("FAILED: {e}"));
                std::process::exit(1);
            }
            break;
        }
        drop(st);
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    cfg.setup_done_for = Some(jobs::setup_key(&exe, &cfg));
    cfg.save();
    say("done");
    std::process::exit(0);
}

fn main() -> eframe::Result<()> {
    update::tidy();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--setup") {
        setup_cli(&args);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([760.0, 720.0]).with_min_inner_size([620.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        &format!("victory-scarlett-play {}", update::VERSION),
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}

enum UpdateState {
    Idle,
    Checking(Arc<Mutex<Option<Result<Option<update::Release>, String>>>>),
    Available(update::Release),
    Installing(Arc<Mutex<(f32, Option<Result<std::path::PathBuf, String>>)>>),
    Message(String),
}

struct App {
    cfg: Config,
    hw: Hardware,
    exe: Option<std::path::PathBuf>,
    job: Option<(JobKind, jobs::Job)>,
    record_opening: bool,
    note: Option<(bool, String)>,
    update: UpdateState,
    reporting: Option<Arc<Mutex<Option<Result<(), String>>>>>,
}

#[derive(Clone, Copy, PartialEq)]
enum JobKind {
    Setup,
    Play,
}

impl App {
    fn new() -> Self {
        let cfg = Config::load();
        let mut app = App {
            hw: Hardware::detect(),
            exe: game::victory_exe(),
            job: None,
            record_opening: true,
            note: None,
            update: UpdateState::Idle,
            reporting: None,
            cfg,
        };
        if app.cfg.check_updates {
            app.start_update_check();
        }
        app
    }

    fn start_update_check(&mut self) {
        let slot = Arc::new(Mutex::new(None));
        let s = slot.clone();
        std::thread::spawn(move || *s.lock().unwrap() = Some(update::check()));
        self.update = UpdateState::Checking(slot);
    }

    fn setup_done(&self) -> bool {
        match &self.exe {
            Some(exe) => self.cfg.setup_done_for.as_deref() == Some(&jobs::setup_key(exe, &self.cfg)),
            None => false,
        }
    }

    fn blocking(&self) -> bool {
        self.hw.findings().iter().any(|f| f.blocking)
    }

    fn busy(&self) -> bool {
        self.job.is_some() || matches!(self.update, UpdateState::Installing(_))
    }

    fn poll(&mut self) {
        // The background job.
        let mut finished = None;
        if let Some((kind, job)) = &self.job {
            let mut st = job.status.lock().unwrap();
            if st.done {
                finished = Some((*kind, st.error.take(), st.session.take(), st.step.clone()));
            }
        }
        if let Some((kind, error, session, step)) = finished {
            self.job = None;
            match (kind, error) {
                (_, Some(e)) if e == "cancelled" => self.note = Some((false, "Stopped.".into())),
                (_, Some(e)) => self.note = Some((true, e)),
                (JobKind::Setup, None) => {
                    if let Some(exe) = &self.exe {
                        self.cfg.setup_done_for = Some(jobs::setup_key(exe, &self.cfg));
                        self.cfg.save();
                    }
                    self.note = Some((false, "Setup finished. Press Play.".into()));
                }
                (JobKind::Play, None) => {
                    if let Some(exe) = &self.exe {
                        // A play run translated again if needed: the caches now fit this build and these settings.
                        self.cfg.setup_done_for = Some(jobs::setup_key(exe, &self.cfg));
                        self.cfg.save();
                    }
                    self.note = Some((false, step));
                    if let Some(session) = session
                        && self.cfg.reports == Some(true)
                        && report::available()
                        && session.minutes >= 1.0
                    {
                        let build = self.exe.as_deref().map(game::build_stamp).unwrap_or_default();
                        let r = report::build(&self.cfg, &self.hw, &session, &build);
                        let slot = Arc::new(Mutex::new(None));
                        let s = slot.clone();
                        std::thread::spawn(move || *s.lock().unwrap() = Some(report::send(&r)));
                        self.reporting = Some(slot);
                    }
                }
            }
        }
        // The update check / install.
        let next = match &self.update {
            UpdateState::Checking(slot) => slot.lock().unwrap().take().map(|r| match r {
                Ok(Some(rel)) => UpdateState::Available(rel),
                Ok(None) => UpdateState::Idle,
                Err(e) => UpdateState::Message(format!("Could not check for updates: {e}")),
            }),
            UpdateState::Installing(slot) => {
                let mut s = slot.lock().unwrap();
                s.1.take().map(|r| match r {
                    Ok(new) => {
                        let _ = std::process::Command::new(new).spawn();
                        std::process::exit(0);
                    }
                    Err(e) => UpdateState::Message(format!("Update failed: {e}")),
                })
            }
            _ => None,
        };
        if let Some(n) = next {
            self.update = n;
        }
        let sent = self.reporting.as_ref().and_then(|slot| slot.lock().unwrap().take());
        if let Some(r) = sent {
            self.note = Some(match r {
                Ok(()) => (false, "Game ended. Report sent - thanks.".into()),
                Err(e) => (true, format!("Game ended. The report could not be sent: {e}")),
            });
            self.reporting = None;
        }
    }

    fn ui_update_banner(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        match &self.update {
            UpdateState::Available(r) => {
                ui.group(|ui| {
                    ui.label(RichText::new(format!("Update available: {} (you have {})", r.version, update::VERSION)).strong());
                    if !r.notes.is_empty() {
                        egui::ScrollArea::vertical().max_height(90.0).show(ui, |ui| ui.label(&r.notes));
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!self.busy(), egui::Button::new(format!("Update now ({:.0} MB)", r.size as f64 / 1e6))).clicked() {
                            action = Some(r.clone());
                        }
                        if self.busy() {
                            ui.label("(after the game or setup has finished)");
                        }
                    });
                });
            }
            UpdateState::Installing(slot) => {
                let p = slot.lock().unwrap().0;
                ui.add(egui::ProgressBar::new(p).text("Downloading the update"));
            }
            UpdateState::Message(m) => {
                ui.colored_label(Color32::from_rgb(200, 150, 40), m);
            }
            _ => {}
        }
        if let Some(r) = action {
            let slot = Arc::new(Mutex::new((0.0f32, None)));
            let s = slot.clone();
            std::thread::spawn(move || {
                let p = s.clone();
                let result = update::install(&r, &move |f| p.lock().unwrap().0 = f);
                s.lock().unwrap().1 = Some(result);
            });
            self.update = UpdateState::Installing(slot);
        }
    }

    fn ui_consent(&mut self, ctx: &egui::Context) {
        if self.cfg.reports.is_some() || !report::available() {
            return;
        }
        egui::Window::new("Help improve it?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.label("After each session the launcher can send a short report to the developer, so the settings for PCs like yours can be improved.");
                ui.add_space(6.0);
                ui.label(RichText::new(report::WHAT).small());
                ui.add_space(6.0);
                ui.checkbox(&mut self.cfg.report_screenshots, "Include up to 4 small screenshots");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Yes, send reports").clicked() {
                        self.cfg.reports = Some(true);
                        self.cfg.save();
                    }
                    if ui.button("No thanks").clicked() {
                        self.cfg.reports = Some(false);
                        self.cfg.save();
                    }
                });
                ui.label(RichText::new("You can change this any time under Settings.").small().weak());
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        if self.job.is_some() || matches!(self.update, UpdateState::Checking(_) | UpdateState::Installing(_)) || self.reporting.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
        self.ui_consent(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("victory-scarlett-play");
                ui.label(RichText::new("The Scarlett build of VIctory. Bring your own dump - no game files are included.").weak());
                ui.add_space(6.0);
                self.ui_update_banner(ui);

                // --- Game folder
                ui.add_space(8.0);
                ui.label(RichText::new("1. Your game folder").strong());
                ui.horizontal(|ui| {
                    let shown = self.cfg.game_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "not chosen".into());
                    ui.label(shown);
                    if ui.add_enabled(!self.busy(), egui::Button::new("Choose...")).clicked()
                        && let Some(dir) = rfd::FileDialog::new().set_title("The folder with game_scarlett_master.exe").pick_folder()
                    {
                        match game::check_game_dir(&dir) {
                            Ok(()) => {
                                self.cfg.game_dir = Some(dir);
                                self.cfg.save();
                                self.note = None;
                            }
                            Err(e) => self.note = Some((true, e)),
                        }
                    }
                });

                // --- Hardware
                ui.add_space(8.0);
                ui.label(RichText::new("2. Your PC").strong());
                let hw = &self.hw;
                egui::Grid::new("hw").num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
                    ui.label("CPU");
                    ui.label(format!("{} ({} threads, AVX2 {})", hw.cpu, hw.threads, if hw.avx2 { "yes" } else { "NO" }));
                    ui.end_row();
                    ui.label("GPU");
                    ui.label(format!("{} - {:.0} GB, driver {}", hw.gpu, hw.vram_gb, hw.driver));
                    ui.end_row();
                    ui.label("Memory");
                    ui.label(format!("{:.0} GB RAM ({:.1} free), RAM + page file {:.0} GB", hw.ram_gb, hw.free_ram_gb, hw.commit_limit_gb));
                    ui.end_row();
                    ui.label("Windows");
                    ui.label(&hw.windows);
                    ui.end_row();
                });
                for f in self.hw.findings() {
                    let color = if f.blocking { Color32::from_rgb(220, 80, 70) } else { Color32::from_rgb(210, 160, 50) };
                    ui.colored_label(color, format!("{} {}", if f.blocking { "✖" } else { "⚠" }, f.text));
                }
                if self.exe.is_none() {
                    ui.colored_label(Color32::from_rgb(220, 80, 70), "✖ victory.exe was not found next to the launcher (expected in the 'victory' folder). Re-extract the release zip.");
                }

                // --- Settings
                ui.add_space(8.0);
                ui.label(RichText::new("3. Settings").strong());
                let busy = self.busy();
                ui.add_enabled_ui(!busy, |ui| {
                    let auto = presets::pick(&self.hw);
                    let current = presets::chosen(&self.cfg, &self.hw);
                    let mut changed = false;
                    egui::ComboBox::from_label("Preset")
                        .selected_text(if self.cfg.preset.is_none() { format!("Automatic: {}", auto.label) } else { current.label.to_owned() })
                        .show_ui(ui, |ui| {
                            changed |= ui.selectable_value(&mut self.cfg.preset, None, format!("Automatic: {}", auto.label)).changed();
                            for p in presets::ALL {
                                changed |= ui
                                    .selectable_value(&mut self.cfg.preset, Some(p.name.to_owned()), format!("{}{}", p.label, if p.tested { "" } else { " (untested)" }))
                                    .changed();
                            }
                        });
                    ui.label(RichText::new(current.about).small().weak());
                    ui.horizontal(|ui| {
                        ui.label("Lighting:");
                        changed |= ui.radio_value(&mut self.cfg.lighting, Lighting::Fast, "Fast").on_hover_text("No ray tracing: fastest, shadows can be very dark").changed();
                        changed |= ui.radio_value(&mut self.cfg.lighting, Lighting::Balanced, "Balanced").on_hover_text("No ray tracing, a fill light in the shadows: same speed, an approximation").changed();
                        changed |= ui.radio_value(&mut self.cfg.lighting, Lighting::RayTraced, "Ray traced").on_hover_text("The game's own ray tracing: right lighting, much slower").changed();
                    });
                    changed |= ui.checkbox(&mut self.cfg.hooks, "Faster memory hooks (10-15 ms a frame less; a few small textures may flicker)").changed();
                    if report::available() {
                        let mut on = self.cfg.reports == Some(true);
                        if ui.checkbox(&mut on, "Send a report after each session (hardware, fps, crash logs)").changed() {
                            self.cfg.reports = Some(on);
                            changed = true;
                        }
                        if on {
                            changed |= ui.checkbox(&mut self.cfg.report_screenshots, "   with up to 4 small screenshots").changed();
                        }
                    }
                    changed |= ui.checkbox(&mut self.cfg.check_updates, "Check for updates at start").changed();
                    if changed {
                        self.cfg.save();
                    }
                });

                // --- Setup and play
                ui.add_space(8.0);
                ui.label(RichText::new("4. Play").strong());
                let ready = self.cfg.game_dir.is_some() && self.exe.is_some() && !self.blocking();
                let setup_done = self.setup_done();
                if !setup_done {
                    ui.label("First-run setup has not been done for this build and these settings. It translates the game's shaders, plays the opening by itself to record the graphics it needs, and compiles them ahead - so faces, skin and the first scenes come out right the first time. About 15 minutes, once.");
                    ui.checkbox(&mut self.record_opening, "Include the opening recording (recommended; uses a separate, temporary save)");
                }
                ui.horizontal(|ui| {
                    let can = ready && !busy;
                    if ui.add_enabled(can, egui::Button::new(RichText::new(if setup_done { "Run setup again" } else { "Run first-run setup" }))).clicked()
                        && let Some(exe) = self.exe.clone()
                    {
                        self.note = None;
                        self.job = Some((JobKind::Setup, jobs::setup(exe, self.cfg.clone(), self.hw.clone(), self.record_opening)));
                    }
                    let play = egui::Button::new(RichText::new("  ▶  Play  ").size(20.0).strong());
                    let r = ui.add_enabled(can, play);
                    let r = if !setup_done { r.on_hover_text("Setup not done: the first session will have missing faces and pop-in") } else { r };
                    if r.clicked()
                        && let Some(exe) = self.exe.clone()
                    {
                        self.note = None;
                        let refresh = !setup_done;
                        self.job = Some((JobKind::Play, jobs::play(exe, self.cfg.clone(), self.hw.clone(), refresh, self.cfg.report_screenshots && self.cfg.reports == Some(true))));
                    }
                });

                if let Some((kind, job)) = &self.job {
                    let st = job.status.lock().unwrap();
                    ui.add_space(6.0);
                    ui.label(RichText::new(&st.step).strong());
                    if st.progress >= 0.0 {
                        ui.add(egui::ProgressBar::new(st.progress).show_percentage());
                    } else {
                        ui.add(egui::Spinner::new());
                    }
                    ui.label(RichText::new(&st.detail).small());
                    drop(st);
                    let label = if *kind == JobKind::Play { "Stop the game" } else { "Cancel setup" };
                    if ui.button(label).clicked() {
                        job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                if let Some((bad, text)) = &self.note {
                    ui.add_space(6.0);
                    if *bad {
                        ui.colored_label(Color32::from_rgb(220, 80, 70), text);
                    } else {
                        ui.label(text);
                    }
                }

                ui.add_space(12.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Open logs folder").clicked() {
                        let _ = std::fs::create_dir_all(config::logs_dir());
                        let _ = std::process::Command::new("explorer").arg(config::logs_dir()).spawn();
                    }
                    if ui.add_enabled(!matches!(self.update, UpdateState::Checking(_) | UpdateState::Installing(_)), egui::Button::new("Check for updates")).clicked() {
                        self.start_update_check();
                    }
                    ui.hyperlink_to("Project page", format!("https://github.com/{}", update::REPO));
                });
                ui.label(RichText::new(format!("Caches and saves: {}", config::home().display())).small().weak());
            });
        });
    }
}
