//! victory-scarlett-play launcher: picks the game folder, checks the PC, runs the first-run setup, plays, updates,
//! and (opt-in) sends reports.
#![windows_subsystem = "windows"]

mod audio;
mod config;
mod game;
mod hardware;
mod jobs;
mod presets;
mod report;
mod rt;
mod saves;
mod update;
mod verify;

use config::{Config, Engine, Lighting};
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
    /// The NVIDIA-RT layer's victory.exe, when it is there.
    rt_exe: Option<std::path::PathBuf>,
    /// "Verify game files" running or done: its summary.
    verify: Option<Arc<Mutex<Option<verify::Report>>>>,
    /// The window was minimised for a game session: restored when it ends.
    minimized_for_game: bool,
    /// Storage figures (bytes): Scarlett caches, NVIDIA-RT caches, logs, saves; refreshed on demand.
    storage: Option<[u64; 4]>,
    /// An import being chosen: where from, and each slot found there with whether it is ticked.
    save_pick: Option<(std::path::PathBuf, Vec<(saves::Slot, bool)>)>,
    /// The last save action's result (error?, text).
    save_note: Option<(bool, String)>,
}

#[derive(Clone, Copy, PartialEq)]
enum JobKind {
    Setup,
    Play,
    Dlss,
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
            rt_exe: rt::exe(),
            verify: None,
            minimized_for_game: false,
            storage: None,
            save_pick: None,
            save_note: None,
            cfg,
        };
        // A saved choice of a layer this folder does not have falls back to Scarlett.
        if app.cfg.engine == Engine::NvidiaRt && app.rt_exe.is_none() {
            app.cfg.engine = Engine::Scarlett;
        }
        app.record_opening = !jobs::recorded(&app.cfg);
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

    /// The victory.exe of the chosen layer.
    fn engine_exe(&self) -> Option<std::path::PathBuf> {
        match self.cfg.engine {
            Engine::Scarlett => self.exe.clone(),
            Engine::NvidiaRt => self.rt_exe.clone(),
        }
    }

    fn setup_done(&self) -> bool {
        // The NVIDIA-RT layer has no setup of its own: it translates and builds as it plays.
        if self.cfg.engine == Engine::NvidiaRt {
            return true;
        }
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
                (JobKind::Dlss, None) => self.note = Some((false, "NVIDIA's DLSS and frame generation files are ready.".into())),
                (JobKind::Setup, None) => {
                    if let Some(exe) = &self.exe {
                        self.cfg.setup_done_for = Some(jobs::setup_key(exe, &self.cfg));
                        self.cfg.save();
                    }
                    self.note = Some((false, "Setup finished. Press Play.".into()));
                }
                (JobKind::Play, None) => {
                    if let Some(exe) = &self.exe
                        && self.cfg.engine == Engine::Scarlett
                    {
                        // A play run translated again if needed: the caches now fit this build and these settings.
                        self.cfg.setup_done_for = Some(jobs::setup_key(exe, &self.cfg));
                        self.cfg.save();
                    }
                    let lost = session.as_ref().is_some_and(|s| s.watch.lost_device);
                    let crashes = session.as_ref().map(|s| s.crash_files.clone()).unwrap_or_default();
                    let mut text = step;
                    if lost {
                        text.push_str(LOST_DEVICE_HELP);
                    }
                    if !crashes.is_empty() {
                        text.push_str(&format!("
Crash files (in the logs folder): {}", crashes.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ")));
                    }
                    self.note = Some((lost || !crashes.is_empty(), text));
                    if let Some(session) = session
                        && self.cfg.reports != Some(false)
                        && report::available()
                        && session.minutes >= 1.0
                    {
                        let build = self.engine_exe().as_deref().map(game::build_stamp).unwrap_or_default();
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
        // Reports are on by default (the player has not turned them off); this notice says so once.
        egui::Window::new("Reports are on")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.label("After each session the launcher sends a short report to the developer, so the settings for PCs like yours can be improved.");
                ui.add_space(6.0);
                ui.label(RichText::new(report::WHAT).small());
                ui.add_space(6.0);
                ui.checkbox(&mut self.cfg.report_screenshots, "Include up to 4 small screenshots");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        self.cfg.reports = Some(true);
                        self.cfg.save();
                    }
                    if ui.button("Turn reports off").clicked() {
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
        if self.minimized_for_game && !matches!(self.job, Some((JobKind::Play, _))) {
            self.minimized_for_game = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if self.job.is_some() || matches!(self.update, UpdateState::Checking(_) | UpdateState::Installing(_)) || self.reporting.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
        self.ui_consent(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("victory-scarlett-play");
                ui.label(RichText::new("Two builds of VIctory: Scarlett and NVIDIA-RT. Bring your own dump - no game files are included.").weak());
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
                if self.engine_exe().is_none() {
                    ui.colored_label(
                        Color32::from_rgb(220, 80, 70),
                        format!(
                            "✖ victory.exe was not found next to the launcher (expected in the '{}' folder). Re-extract the release zip.",
                            if self.cfg.engine == Engine::Scarlett { "victory" } else { "victory-rt" }
                        ),
                    );
                }

                // --- Settings
                ui.add_space(8.0);
                ui.label(RichText::new("3. Settings").strong());
                let busy = self.busy();
                ui.add_enabled_ui(!busy, |ui| {
                    let auto = presets::pick(&self.hw);
                    let current = presets::chosen(&self.cfg, &self.hw);
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        ui.label("Game layer:");
                        changed |= ui
                            .radio_value(&mut self.cfg.engine, Engine::Scarlett, "Scarlett")
                            .on_hover_text("Op-lite/victory-scarlett with this project's fixes: the first-run setup records the opening so faces and scenes come out right; mouse & keyboard in a GTA PC layout")
                            .changed();
                        let rt_there = self.rt_exe.is_some();
                        let r = ui
                            .add_enabled(rt_there, egui::RadioButton::new(self.cfg.engine == Engine::NvidiaRt, "NVIDIA-RT (Kemerd test 9)"))
                            .on_hover_text("Kemerd/VIctory-NVIDIA-RT: a faster frame pipeline (11.8 -> 21.9 fps median on its author's RTX 5090), DLSS, frame generation up to 6x, Reflex, HDR10, 21:9 and 32:9. F1 in the game is its settings menu, F3 its stats. No setup: the first session translates as it goes.")
                            .on_disabled_hover_text("victory-rt\\victory.exe is not in this folder");
                        if r.clicked() && self.cfg.engine != Engine::NvidiaRt {
                            self.cfg.engine = Engine::NvidiaRt;
                            changed = true;
                        }
                    });
                    let scarlett = self.cfg.engine == Engine::Scarlett;
                    if !scarlett {
                        changed |= ui_rt(ui, &mut self.cfg.rt, self.hw.vendor == hardware::Vendor::Nvidia, &self.hw.gpu);
                        changed |= ui
                            .checkbox(&mut self.cfg.rt.scarlett_fixes, "Scarlett's fixes (depth planes in time for tiled lighting, storage workers per CPU core)")
                            .on_hover_text("Ported from the Scarlett build: VICTORY_PLANE_NOW and VICTORY_DSTORAGE_WORKERS=physical. Turn off to compare.")
                            .changed();
                        changed |= ui
                            .checkbox(&mut self.cfg.cut_hold, "Wait at scene changes until the scene has loaded (from Scarlett)")
                            .on_hover_text("At a camera cut the game waits (up to 3 s) for its reads and pipelines instead of showing black ground and missing objects")
                            .changed();
                        changed |= ui
                            .checkbox(&mut self.cfg.kbm, "Mouse look (the pointer is locked in the game window - F2 or ~ frees it and locks it again)")
                            .on_hover_text("The mouse moves the camera through the game's own mouse support. Keys can be changed in the F1 menu. Alt+Tab also frees the pointer.")
                            .changed();
                    }
                    if scarlett {
                    changed |= ui
                        .checkbox(&mut self.cfg.kbm, "Mouse & keyboard (GTA PC layout - press F2 or ~ in game to lock the mouse for looking, Esc to free it)")
                        .on_hover_text("W A S D move, mouse looks, left/right click shoot/aim, Shift sprint, Space jump, F enter vehicle, R reload, Tab weapon wheel, Q cover, E interact, Ctrl crouch, C look back, V camera, P pause, M phone. An Xbox controller works at the same time.")
                        .changed();
                    changed |= ui
                        .checkbox(&mut self.cfg.cut_hold, "Hold the picture at scene changes until the scene has loaded")
                        .on_hover_text("After a camera cut the last picture stays up to 0.8 s while the new scene streams in, instead of showing black ground and missing textures")
                        .changed();
                    changed |= ui.checkbox(&mut self.cfg.hooks, "Faster memory hooks (10-15 ms a frame less; a few small textures may flicker)").changed();
                    }
                    changed |= ui.checkbox(&mut self.cfg.minimize_while_playing, "Minimise the launcher while the game runs").changed();
                    if report::available() {
                        let mut on = self.cfg.reports != Some(false);
                        if ui.checkbox(&mut on, "Send a report after each session (hardware, fps, crash logs)").changed() {
                            self.cfg.reports = Some(on);
                            changed = true;
                        }
                        if on {
                            changed |= ui.checkbox(&mut self.cfg.report_screenshots, "   with up to 4 small screenshots").changed();
                        }
                    }
                    changed |= ui.checkbox(&mut self.cfg.check_updates, "Check for updates at start").changed();
                    // The NVIDIA-RT layer's choices are all shown above; Advanced is Scarlett's.
                    if scarlett {
                    egui::CollapsingHeader::new("Advanced").default_open(false).show(ui, |ui| {
                        if scarlett {
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
                            changed |= ui.radio_value(&mut self.cfg.lighting, Lighting::Fast, "Standard").on_hover_text("The game's lighting without ray tracing (recommended)").changed();
                            changed |= ui.radio_value(&mut self.cfg.lighting, Lighting::Balanced, "Brighter shadows").on_hover_text("Adds a fill light where shadows come out too dark: same speed, an approximation").changed();
                            changed |= ui.radio_value(&mut self.cfg.lighting, Lighting::RayTraced, "Ray traced").on_hover_text("The game's own ray tracing: right lighting, much slower").changed();
                        });
                        if self.cfg.kbm {
                            ui.horizontal(|ui| {
                                ui.label("Mouse sensitivity:");
                                changed |= ui.add(egui::Slider::new(&mut self.cfg.mouse_sensitivity, 0.25..=3.0).step_by(0.05)).changed();
                                changed |= ui.checkbox(&mut self.cfg.mouse_invert_y, "Invert Y").changed();
                            });
                        }
                        ui.horizontal(|ui| {
                            ui.label("Render scale:");
                            changed |= ui.radio_value(&mut self.cfg.render_scale, 1.0, "Full").changed();
                            changed |= ui.radio_value(&mut self.cfg.render_scale, 0.75, "75%").on_hover_text("Less work for the graphics card, a little softer").changed();
                            changed |= ui.radio_value(&mut self.cfg.render_scale, 0.5, "50%").on_hover_text("Much less work for the graphics card, softer; helps weaker cards, not a slow CPU").changed();
                        });
                        }
                        if scarlett {
                        ui.horizontal(|ui| {
                            ui.label("Console mode:");
                            let preset_console = current.console;
                            changed |= ui
                                .radio_value(&mut self.cfg.console, None, format!("Automatic ({})", if preset_console == "series-s" { "Series S" } else { "Series X" }))
                                .changed();
                            changed |= ui
                                .radio_value(&mut self.cfg.console, Some("series-x".into()), "Series X")
                                .on_hover_text("Full resolution (the game renders at 1440p and upscales)")
                                .changed();
                            changed |= ui
                                .radio_value(&mut self.cfg.console, Some("series-s".into()), "Series S")
                                .on_hover_text("Lower resolution and smaller memory budgets: easier on the graphics card and VRAM; does little for a slow CPU")
                                .changed();
                        });
                        }
                    });
                    }
                    if changed {
                        self.cfg.save();
                    }
                });

                // --- Setup and play
                ui.add_space(8.0);
                ui.label(RichText::new("4. Play").strong());
                let ready = self.cfg.game_dir.is_some() && self.engine_exe().is_some() && !self.blocking();
                if self.cfg.engine == Engine::NvidiaRt && !rt::has_dlss() {
                    ui.colored_label(
                        Color32::from_rgb(210, 160, 50),
                        "⚠ NVIDIA's DLSS and frame generation files are not on this PC yet: without them the game uses its own upscaling and no frame generation.",
                    );
                    if ui
                        .add_enabled(!busy && self.rt_exe.is_some(), egui::Button::new("Download them from NVIDIA's GitHub (about 70 MB, once)"))
                        .on_hover_text("NVIDIA's own files under NVIDIA's licences, from github.com/NVIDIA-RTX/Streamline, into %LOCALAPPDATA%\\victory\\nvidia-sdk. Not part of this project.")
                        .clicked()
                        && let Some(exe) = self.rt_exe.clone()
                    {
                        self.note = None;
                        self.job = Some((JobKind::Dlss, jobs::get_dlss(exe)));
                    }
                }
                let setup_done = self.setup_done();
                if !setup_done {
                    if jobs::recorded(&self.cfg) {
                        ui.label("New build or settings: the game's shaders are translated again and the recorded graphics compiled ahead. About 6 minutes. (Play does this by itself too.)");
                        ui.checkbox(&mut self.record_opening, "Also record the opening again (not needed after an update; about 9 minutes more)");
                    } else {
                        // No recording for this lighting yet (first run, or ray tracing switched on): record it.
                        self.record_opening = true;
                        ui.label("First-run setup has not been done yet for this lighting. It translates the game's shaders, plays the opening by itself in a black window to record the graphics it needs (nothing is shown, so the story isn't spoiled - don't touch it), and compiles them ahead, so faces, skin and the first scenes come out right when you play. About 15 minutes, once.");
                        ui.checkbox(&mut self.record_opening, "Include the opening recording (recommended; uses a separate, temporary save)");
                    }
                }
                if self.job.is_none() {
                    let (free_ram, free_commit) = hardware::free_now();
                    match hardware::memory_advice(free_ram, free_commit) {
                        Some((true, text)) => {
                            ui.colored_label(Color32::from_rgb(220, 80, 70), format!("✖ {text}"));
                        }
                        Some((false, text)) => {
                            ui.colored_label(Color32::from_rgb(210, 160, 50), format!("⚠ {text}"));
                        }
                        None => {
                            ui.label(RichText::new(format!("{free_ram:.0} GB of RAM free - good.")).small().weak());
                        }
                    }
                    // Keep the figure current while the window is open.
                    ctx.request_repaint_after(std::time::Duration::from_secs(3));
                }
                ui.horizontal(|ui| {
                    let can = ready && !busy;
                    if self.cfg.engine == Engine::Scarlett
                        && ui.add_enabled(can, egui::Button::new(RichText::new(if setup_done { "Run setup again" } else { "Run first-run setup" }))).clicked()
                        && let Some(exe) = self.exe.clone()
                    {
                        self.note = None;
                        self.job = Some((JobKind::Setup, jobs::setup(exe, self.cfg.clone(), self.hw.clone(), self.record_opening)));
                    }
                    let play = egui::Button::new(RichText::new("  ▶  Play  ").size(20.0).strong());
                    let r = ui.add_enabled(can, play);
                    let r = if setup_done {
                        r
                    } else if jobs::recorded(&self.cfg) {
                        r.on_hover_text("New build or settings: Play first brings the shaders and recorded graphics up to date (a few minutes), then starts the game")
                    } else {
                        r.on_hover_text("Setup not done: the first session will have missing faces and pop-in (run the first-run setup first)")
                    };
                    if r.clicked()
                        && let Some(exe) = self.engine_exe()
                    {
                        self.note = None;
                        // A copy of every save first (the newest 10 are kept): a session that goes wrong cannot cost one.
                        if self.cfg.backup_saves
                            && let Err(e) = saves::backup(&config::saves_dir())
                        {
                            self.save_note = Some((true, format!("The save backup before Play failed: {e}")));
                        }
                        if self.cfg.minimize_while_playing {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                            self.minimized_for_game = true;
                        }
                        let refresh = !setup_done;
                        self.job = Some((JobKind::Play, jobs::play(exe, self.cfg.clone(), self.hw.clone(), refresh, self.cfg.report_screenshots && self.cfg.reports != Some(false))));
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
                    let label = match kind {
                        JobKind::Play => "Stop the game",
                        JobKind::Setup => "Cancel setup",
                        JobKind::Dlss => "Cancel the download",
                    };
                    if ui.button(label).clicked() {
                        job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                let mut tdr_result = None;
                if let Some((bad, text)) = &self.note {
                    ui.add_space(6.0);
                    if *bad {
                        ui.colored_label(Color32::from_rgb(220, 80, 70), text);
                    } else {
                        ui.label(text);
                    }
                    if text.contains(LOST_DEVICE_HELP)
                        && hardware::tdr_delay().is_none_or(|d| d < 10)
                        && ui.button("Give the graphics card 10 s before Windows resets it (asks for administrator rights; restart after)").clicked()
                    {
                        tdr_result = Some(match hardware::set_tdr_delay_10() {
                            Ok(()) => (false, "TdrDelay is 10 s now. Restart Windows for it to take effect.".to_owned()),
                            Err(e) => (true, format!("TdrDelay: {e}")),
                        });
                    }
                }
                if tdr_result.is_some() {
                    self.note = tdr_result;
                }

                ui.add_space(12.0);
                self.ui_saves(ui);
                self.ui_tools(ui);
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

/// Appended to the note after a session that lost the graphics card (Kemerd/VIctory-NVIDIA-RT's advice).
const LOST_DEVICE_HELP: &str = "\nThe graphics card stopped answering. These layers push the card harder and longer than most games: take off any GPU overclock or undervolt (MSI Afterburner, the NVIDIA app's auto-tune) and test at stock clocks; and give Windows more time before it resets the card (the button below sets TdrDelay to 10 s).";

/// The NVIDIA-RT layer's choices (its `PLAY.cmd`'s ten). Returns whether one changed.
fn ui_rt(ui: &mut egui::Ui, rt: &mut config::RtSettings, nvidia: bool, gpu: &str) -> bool {
    let mut changed = false;
    if !nvidia {
        ui.colored_label(Color32::from_rgb(210, 160, 50), "⚠ This layer is made for NVIDIA RTX cards: DLSS, frame generation and Reflex need one.");
    }
    egui::Grid::new("rt").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        ui.label("Resolution");
        egui::ComboBox::from_id_salt("rt-res")
            .selected_text(match rt.res.as_str() {
                "1440" => "2560x1440".to_owned(),
                "1080to1440" => "1920x1080 upscaled to 1440".to_owned(),
                "4k" => "3840x2160".to_owned(),
                "wide" => "5120x1440 (32:9)".to_owned(),
                other => other.to_owned(),
            })
            .show_ui(ui, |ui| {
                for (v, l) in [
                    ("1440", "2560x1440"),
                    ("1080to1440", "1920x1080 upscaled to 1440 (faster)"),
                    ("4k", "3840x2160"),
                    ("3440x1440", "3440x1440 (21:9)"),
                    ("wide", "5120x1440 (32:9)"),
                ] {
                    changed |= ui.selectable_value(&mut rt.res, v.to_owned(), l).changed();
                }
            });
        ui.end_row();
        ui.label("Window");
        ui.horizontal(|ui| {
            changed |= ui.radio_value(&mut rt.windowed, false, "Fullscreen, no border").changed();
            changed |= ui.radio_value(&mut rt.windowed, true, "In a window").changed();
        });
        ui.end_row();
        ui.label("DLSS");
        ui.horizontal(|ui| {
            for (v, l) in [("quality", "Quality"), ("balanced", "Balanced"), ("performance", "Performance"), ("dlaa", "DLAA"), ("off", "Off")] {
                changed |= ui.radio_value(&mut rt.dlss, v.to_owned(), l).changed();
            }
        });
        ui.end_row();
        ui.label("Frame generation");
        let rtx50 = gpu.contains("RTX 50");
        egui::ComboBox::from_id_salt("rt-fg")
            .selected_text(match rt.framegen {
                0 => "Ready - switch on in F1".to_owned(),
                1 => "Not loaded".to_owned(),
                n => format!("{n}x from the start"),
            })
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut rt.framegen, 0, "Ready - switch on in F1").changed();
                changed |= ui.selectable_value(&mut rt.framegen, 2, "2x from the start").changed();
                for n in 3..=6 {
                    let label = if rtx50 { format!("{n}x from the start") } else { format!("{n}x from the start (needs an RTX 50)") };
                    changed |= ui.selectable_value(&mut rt.framegen, n, label).changed();
                }
                changed |= ui.selectable_value(&mut rt.framegen, 1, "Not loaded (saves 1.5-2 GB of VRAM)").changed();
            });
        ui.end_row();
        ui.label("NVIDIA Reflex");
        ui.horizontal(|ui| {
            changed |= ui.radio_value(&mut rt.reflex, "boost".to_owned(), "On + boost").changed();
            changed |= ui.radio_value(&mut rt.reflex, "on".to_owned(), "On").changed();
            changed |= ui.radio_value(&mut rt.reflex, "off".to_owned(), "Off").on_hover_text("Frame generation turns it on anyway").changed();
        });
        ui.end_row();
        ui.label("Frame cap");
        egui::ComboBox::from_id_salt("rt-fps")
            .selected_text(if rt.fps_cap >= 1000 { "None".to_owned() } else { rt.fps_cap.to_string() })
            .show_ui(ui, |ui| {
                for (v, l) in [(1000, "None"), (30, "30"), (60, "60"), (120, "120"), (144, "144")] {
                    changed |= ui.selectable_value(&mut rt.fps_cap, v, l).changed();
                }
            });
        ui.end_row();
        ui.label("Console");
        ui.horizontal(|ui| {
            changed |= ui.radio_value(&mut rt.series_s, false, "Series X (recommended)").changed();
            changed |= ui.radio_value(&mut rt.series_s, true, "Series S").on_hover_text("1280x720 inside: much softer; only for weaker cards").changed();
        });
        ui.end_row();
        ui.label("Lighting");
        ui.add_enabled_ui(!rt.ray_tracing, |ui| {
            ui.horizontal(|ui| {
                changed |= ui
                    .radio_value(&mut rt.light, "both".to_owned(), "Probes + fill light")
                    .on_hover_text("Scarlett's light probes (no violet vegetation, no stale light) plus this layer's fill light in the shadows")
                    .changed();
                changed |= ui
                    .radio_value(&mut rt.light, "probes".to_owned(), "Probes (Scarlett)")
                    .on_hover_text("As the Scarlett build lights it: the game's probes only; shadows darker")
                    .changed();
                changed |= ui
                    .radio_value(&mut rt.light, "fill".to_owned(), "Fill light (original)")
                    .on_hover_text("As VIctory-NVIDIA ships it: a stand-in fill light, the probes left out")
                    .changed();
            });
        });
        ui.end_row();
        ui.label("Speed");
        ui.horizontal(|ui| {
            changed |= ui.radio_value(&mut rt.fast, true, "Fast (recommended)").changed();
            changed |= ui.radio_value(&mut rt.fast, false, "Plain").on_hover_text("The official release's settings").changed();
        });
        ui.end_row();
    });
    changed |= ui.checkbox(&mut rt.hdr, "HDR10 (turn Windows HDR on first)").changed();
    if rt.res == "wide" || rt.res.contains('x') {
        changed |= ui.checkbox(&mut rt.ui_stretch, "Stretch the interface across the ultrawide screen (instead of 16:9 in the middle)").changed();
    }
    changed |= ui
        .checkbox(&mut rt.ray_tracing, "The game's own ray tracing (experimental: 5-8 fps)")
        .on_hover_text("VICTORY_RT_NATIVE: the title's builder calls on the RTX cores")
        .changed();
    ui.label(
        RichText::new("In the game: F1 settings menu (keys can be rebound there), F3 stats. The launcher's resolution, DLSS, frame generation, HDR and window choices win over what F1 saved; every other F1 choice is kept.")
            .small()
            .weak(),
    );
    changed
}

/// Bytes in a folder and everything under it.
fn dir_size(dir: &std::path::Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

impl App {
    /// Verify game files; storage (cache sizes, clearing a cache).
    fn ui_tools(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Tools").default_open(false).show(ui, |ui| {
            let busy = self.busy();
            ui.horizontal(|ui| {
                let running = self.verify.as_ref().is_some_and(|v| v.lock().unwrap().is_none());
                if ui
                    .add_enabled(self.cfg.game_dir.is_some() && !running, egui::Button::new("Verify game files"))
                    .on_hover_text("Checks that every archive and pack is there, not empty, and readable. Reads only: nothing in the game folder is changed.")
                    .clicked()
                    && let Some(dir) = self.cfg.game_dir.clone()
                {
                    let slot = Arc::new(Mutex::new(None));
                    let s = slot.clone();
                    std::thread::spawn(move || *s.lock().unwrap() = Some(verify::check(&dir)));
                    self.verify = Some(slot);
                }
                if running {
                    ui.add(egui::Spinner::new());
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
                }
            });
            if let Some(v) = &self.verify
                && let Some(r) = v.lock().unwrap().as_ref()
            {
                let color = if r.problems.is_empty() { Color32::from_rgb(90, 170, 90) } else { Color32::from_rgb(220, 80, 70) };
                ui.colored_label(color, r.summary());
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Storage").strong());
                if ui.small_button("Measure").clicked() {
                    self.storage = Some([dir_size(&config::cache_dir()), dir_size(&config::rt_cache_dir()), dir_size(&config::logs_dir()), dir_size(&config::saves_dir())]);
                }
            });
            if let Some([scarlett, rt, logs, saves]) = self.storage {
                let gb = |b: u64| format!("{:.2} GB", b as f64 / 1e9);
                let mut clear = None;
                egui::Grid::new("storage").num_columns(3).spacing([12.0, 2.0]).show(ui, |ui| {
                    ui.label("Scarlett shaders and pipelines");
                    ui.label(gb(scarlett));
                    if ui
                        .add_enabled(!busy && scarlett > 0, egui::Button::new("Clear"))
                        .on_hover_text("Built again by the next setup or Play (minutes); the opening recording is lost too")
                        .clicked()
                    {
                        clear = Some(config::cache_dir());
                    }
                    ui.end_row();
                    ui.label("NVIDIA-RT shaders and pipelines");
                    ui.label(gb(rt));
                    if ui.add_enabled(!busy && rt > 0, egui::Button::new("Clear")).on_hover_text("Built again while playing").clicked() {
                        clear = Some(config::rt_cache_dir());
                    }
                    ui.end_row();
                    ui.label("Logs");
                    ui.label(gb(logs));
                    if ui.add_enabled(!busy && logs > 0, egui::Button::new("Clear")).clicked() {
                        clear = Some(config::logs_dir());
                    }
                    ui.end_row();
                    ui.label("Saves (never cleared here)");
                    ui.label(gb(saves));
                    ui.end_row();
                });
                if let Some(dir) = clear {
                    let _ = std::fs::remove_dir_all(&dir);
                    if dir == config::cache_dir() {
                        self.cfg.setup_done_for = None;
                        self.cfg.save();
                    }
                    self.storage = None;
                }
            }
        });
    }
}

impl App {
    /// Saves: what there is, backup, export, import from other builds.
    fn ui_saves(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Saves").default_open(false).show(ui, |ui| {
            let busy = self.busy();
            if busy {
                ui.label(RichText::new("While the game runs, saves are only shown: import and export after it has closed.").small().weak());
            }
            let data = config::saves_dir();
            let mine = saves::ours(&data);
            if mine.is_empty() {
                ui.label("No saves yet.");
            }
            for s in &mine {
                ui.label(format!("• {}", s.label()));
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy && !mine.is_empty(), egui::Button::new("Back up now")).clicked() {
                    self.save_note = Some(match saves::backup(&data) {
                        Ok(Some(f)) => (false, format!("Backed up to {}", f.display())),
                        Ok(None) => (false, "Nothing to back up.".into()),
                        Err(e) => (true, format!("Backup failed: {e}")),
                    });
                }
                if ui.add_enabled(!busy && !mine.is_empty(), egui::Button::new("Export to a zip...")).clicked()
                    && let Some(to) = rfd::FileDialog::new().set_title("Export saves").set_file_name("gta-saves.zip").add_filter("zip", &["zip"]).save_file()
                {
                    self.save_note = Some(match saves::export(&data, &to) {
                        Ok(n) => (false, format!("Exported {n} files to {}", to.display())),
                        Err(e) => (true, format!("Export failed: {e}")),
                    });
                }
                if ui.button("Open backups folder").clicked() {
                    let _ = std::fs::create_dir_all(saves::backups_dir());
                    let _ = std::process::Command::new("explorer").arg(saves::backups_dir()).spawn();
                }
            });
            if ui.checkbox(&mut self.cfg.backup_saves, "Back up every save before each Play (the newest 10 kept)").changed() {
                self.cfg.save();
            }
            ui.add_space(4.0);
            ui.label(RichText::new("Import").strong());
            ui.label(
                RichText::new("An imported save becomes the newest version of its slot; what you had stays on disk under it (and in the backups).")
                    .small()
                    .weak(),
            );
            let mut pick_from: Option<std::path::PathBuf> = None;
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (label, path) in saves::known_sources(&data) {
                        if ui.button(format!("From {label}")).on_hover_text(path.display().to_string()).clicked() {
                            pick_from = Some(path);
                        }
                    }
                    if ui.button("From a folder...").on_hover_text("Another build's data or saves folder, or XCL's gamesave folder").clicked()
                        && let Some(dir) = rfd::FileDialog::new().set_title("A folder with saves").pick_folder()
                    {
                        pick_from = Some(dir);
                    }
                    if ui.button("From a zip...").on_hover_text("A zip this launcher exported or backed up").clicked()
                        && let Some(f) = rfd::FileDialog::new().set_title("A saves zip").add_filter("zip", &["zip"]).pick_file()
                    {
                        pick_from = Some(f);
                    }
                });
            });
            if let Some(path) = pick_from {
                match saves::read_source(&path) {
                    Ok(slots) => {
                        let ticked = slots
                            .into_iter()
                            .map(|s| {
                                let story = s.is_story();
                                (s, story)
                            })
                            .collect();
                        self.save_pick = Some((path, ticked));
                        self.save_note = None;
                    }
                    Err(e) => self.save_note = Some((true, e)),
                }
            }
            let mut done = None;
            if let Some((from, slots)) = &mut self.save_pick {
                ui.group(|ui| {
                    ui.label(format!("Saves in {}:", from.display()));
                    for (slot, on) in slots.iter_mut() {
                        ui.checkbox(on, slot.label());
                    }
                    ui.horizontal(|ui| {
                        let n = slots.iter().filter(|(_, on)| *on).count();
                        if ui.add_enabled(!busy && n > 0, egui::Button::new(format!("Import {n} selected"))).clicked() {
                            // A backup of what is there first.
                            let backed = saves::backup(&data);
                            let mut errors = Vec::new();
                            for (slot, _) in slots.iter().filter(|(_, on)| *on) {
                                if let Err(e) = saves::import(&data, slot) {
                                    errors.push(format!("{}: {e}", slot.name));
                                }
                            }
                            done = Some(if errors.is_empty() {
                                let before = match backed {
                                    Ok(Some(f)) => format!("Your saves from before it are in {}.", f.display()),
                                    _ => String::new(),
                                };
                                (false, format!("Imported {n}. {before}"))
                            } else {
                                (true, format!("Import failed for: {}", errors.join("; ")))
                            });
                        }
                        if ui.button("Cancel").clicked() {
                            done = Some((false, String::new()));
                        }
                    });
                });
            }
            if let Some(note) = done {
                self.save_pick = None;
                self.save_note = (!note.1.is_empty()).then_some(note);
            }
            if let Some((bad, text)) = &self.save_note {
                if *bad {
                    ui.colored_label(Color32::from_rgb(220, 80, 70), text);
                } else {
                    ui.label(RichText::new(text).small());
                }
            }
        });
    }
}
