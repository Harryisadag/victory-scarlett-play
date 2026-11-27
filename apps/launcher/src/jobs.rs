//! Work done in the background while the window shows how it goes: the first-run setup and playing.

use crate::config::{self, Config};
use crate::game::{self, Watch};
use crate::hardware::Hardware;
use crate::presets;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the setup lets the opening play by itself, recording what it draws with.
pub const RECORD_SECONDS: u64 = 9 * 60;
/// Shaders the game has (25,689 in v1.0.2.1): for the translation's progress bar.
const SHADERS: f64 = 25_700.0;
/// Scarlett's -AutoStart: presses through the start screens of a fresh start (no save) into the opening.
const AUTO_START: &str = "d>=15,rep(A,1,d<10),w3,d>=20,w1,rep(A,1,d>=35),w1.5,Down,w7,Down,w0.6,A,w1.5,A,w0.7,Down,w0.5,A,w0.7,Down,w0.5,A";

#[derive(Default)]
pub struct Status {
    pub step: String,
    pub detail: String,
    /// 0..1, or negative for "busy, no measure".
    pub progress: f32,
    pub done: bool,
    pub error: Option<String>,
    /// Set when a game session ended: what to report.
    pub session: Option<Session>,
}

pub struct Session {
    pub minutes: f64,
    pub outcome: String,
    pub watch: Watch,
    pub preset: String,
}

pub struct Job {
    pub status: Arc<Mutex<Status>>,
    pub cancel: Arc<AtomicBool>,
}

impl Job {
    fn start(f: impl FnOnce(&Arc<Mutex<Status>>, &AtomicBool) -> Result<(), String> + Send + 'static) -> Job {
        let status = Arc::new(Mutex::new(Status::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (s, c) = (status.clone(), cancel.clone());
        std::thread::spawn(move || {
            let r = f(&s, &c);
            let mut st = s.lock().unwrap();
            st.done = true;
            if let Err(e) = r {
                st.error = Some(e);
            }
        });
        Job { status, cancel }
    }
}

fn say(s: &Arc<Mutex<Status>>, step: &str, detail: &str, progress: f32) {
    let mut st = s.lock().unwrap();
    st.step = step.to_owned();
    st.detail = detail.to_owned();
    st.progress = progress;
}

/// What the setup was done for: this build, these translation settings, this game folder.
pub fn setup_key(exe: &std::path::Path, cfg: &Config) -> String {
    format!(
        "{}|{}|{}",
        game::build_stamp(exe),
        presets::translation_key(cfg),
        cfg.game_dir.as_deref().map(|p| p.display().to_string()).unwrap_or_default()
    )
}

/// Translates every shader of the game ahead (`victory shaders`).
fn translate(exe: &Path, cfg: &Config, hw: &Hardware, s: &Arc<Mutex<Status>>, cancel: &AtomicBool) -> Result<(), String> {
    let game_dir = cfg.game_dir.clone().ok_or("no game folder")?;
    let (mut cmd, _) = game::command(exe, cfg, hw, &config::saves_dir());
    cmd.arg("shaders").arg(game_dir.join("shaders.rpf"));
    let watch = Arc::new(Mutex::new(Watch::default()));
    let line = Arc::new(Mutex::new(String::new()));
    let mut child = game::spawn_logged(cmd, config::logs_dir().join("translate.log"), watch, Some(line.clone()))
        .map_err(|e| format!("could not start victory.exe: {e}"))?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            return Err("cancelled".into());
        }
        if let Some(code) = child.try_wait().map_err(|e| e.to_string())? {
            if !code.success() {
                return Err(format!("translating the shaders failed ({code}); see {}", config::logs_dir().join("translate.log").display()));
            }
            return Ok(());
        }
        let l = line.lock().unwrap().clone();
        let n: f64 = l.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
        say(s, "Translating the game's shaders", &l, (n / SHADERS).min(1.0) as f32);
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Compiles the pipelines earlier runs recorded, ahead (`victory warm --used`; only new ones).
fn warm(exe: &Path, cfg: &Config, hw: &Hardware, s: &Arc<Mutex<Status>>, cancel: &AtomicBool) -> Result<(), String> {
    if !config::cache_dir().join("pipelines").join("known-pipelines.bin").is_file() {
        return Ok(());
    }
    let (mut cmd, _) = game::command(exe, cfg, hw, &config::saves_dir());
    cmd.arg("warm").arg("--used");
    let watch = Arc::new(Mutex::new(Watch::default()));
    let line = Arc::new(Mutex::new(String::new()));
    let mut child = game::spawn_logged(cmd, config::logs_dir().join("warm.log"), watch.clone(), Some(line.clone()))
        .map_err(|e| format!("could not start victory.exe: {e}"))?;
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            return Err("cancelled".into());
        }
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            // A failure here costs only speed on the first visits: not an error.
            return Ok(());
        }
        let last = watch.lock().unwrap().tail.back().cloned().unwrap_or_default();
        say(
            s,
            "Compiling the recorded graphics pipelines ahead",
            &format!("{} s - {}", started.elapsed().as_secs(), last.chars().take(140).collect::<String>()),
            -1.0,
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Plays the opening unattended from a fresh start, so every pipeline it uses is recorded, then stops it.
fn record_opening(exe: &Path, cfg: &Config, hw: &Hardware, s: &Arc<Mutex<Status>>, cancel: &AtomicBool) -> Result<(), String> {
    let game_dir = cfg.game_dir.clone().ok_or("no game folder")?;
    let fresh = config::home().join("setup-saves");
    let _ = std::fs::remove_dir_all(&fresh);
    std::fs::create_dir_all(&fresh).map_err(|e| e.to_string())?;
    let (mut cmd, console) = game::command(exe, cfg, hw, &fresh);
    cmd.env("VICTORY_AUTO_INPUT", AUTO_START);
    cmd.arg("run").arg(game::game_exe(&game_dir)).arg("--console").arg(console).arg("--execute");
    let watch = Arc::new(Mutex::new(Watch::default()));
    let mut child = game::spawn_logged(cmd, config::logs_dir().join("setup-opening.log"), watch.clone(), None)
        .map_err(|e| format!("could not start the game: {e}"))?;
    let started = Instant::now();
    let r = loop {
        let t = started.elapsed().as_secs();
        if cancel.load(Ordering::Relaxed) {
            break Err("cancelled".to_owned());
        }
        if let Some(code) = child.try_wait().map_err(|e| e.to_string())? {
            // The game ended by itself: whatever it recorded is kept.
            break if t > 60 { Ok(()) } else { Err(format!("the game stopped after {t} s ({code}); see the log in {}", config::logs_dir().display())) };
        }
        if t >= RECORD_SECONDS {
            break Ok(());
        }
        let fps = watch.lock().unwrap().samples.last().map(|x| x.fps).unwrap_or(0.0);
        say(
            s,
            "Playing the opening by itself to record it - please don't touch the game window",
            &format!("{} of {} min · {:.0} fps", t / 60, RECORD_SECONDS / 60, fps),
            t as f32 / RECORD_SECONDS as f32,
        );
        std::thread::sleep(Duration::from_millis(500));
    };
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&fresh);
    r
}

/// The first-run setup: translate, record the opening, compile ahead.
pub fn setup(exe: PathBuf, cfg: Config, hw: Hardware, record: bool) -> Job {
    Job::start(move |s, cancel| {
        translate(&exe, &cfg, &hw, s, cancel)?;
        if record {
            record_opening(&exe, &cfg, &hw, s, cancel)?;
        }
        warm(&exe, &cfg, &hw, s, cancel)?;
        say(s, "Setup finished", "", 1.0);
        Ok(())
    })
}

/// Plays: brings the caches up to date if needed, starts the game, watches it until it ends.
pub fn play(exe: PathBuf, cfg: Config, hw: Hardware, refresh: bool, screenshots: bool) -> Job {
    Job::start(move |s, cancel| {
        if refresh {
            translate(&exe, &cfg, &hw, s, cancel)?;
        }
        warm(&exe, &cfg, &hw, s, cancel)?;
        let game_dir = cfg.game_dir.clone().ok_or("no game folder")?;
        let preset = presets::chosen(&cfg, &hw);
        let (mut cmd, console) = game::command(&exe, &cfg, &hw, &config::saves_dir());
        cmd.arg("run").arg(game::game_exe(&game_dir)).arg("--console").arg(console).arg("--execute");
        let watch = Arc::new(Mutex::new(Watch::default()));
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let log = config::logs_dir().join(format!("game-{stamp}.log"));
        let mut child = game::spawn_logged(cmd, log.clone(), watch.clone(), None).map_err(|e| format!("could not start the game: {e}"))?;
        let started = Instant::now();
        let mut next_shot = Duration::from_secs(4 * 60);
        let code = loop {
            if let Some(code) = child.try_wait().map_err(|e| e.to_string())? {
                break Some(code);
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                break child.wait().ok();
            }
            let mem = game::private_gb(&child);
            let (avg, _, _) = {
                let mut w = watch.lock().unwrap();
                w.peak_private_gb = w.peak_private_gb.max(mem);
                w.summary()
            };
            if screenshots && started.elapsed() >= next_shot {
                next_shot += Duration::from_secs(6 * 60);
                if let Some(jpg) = game::screenshot(child.id()) {
                    let mut w = watch.lock().unwrap();
                    if w.shots.len() >= 4 {
                        w.shots.remove(1);
                    }
                    w.shots.push(jpg);
                }
            }
            let last = watch.lock().unwrap().samples.last().copied();
            say(
                s,
                "Playing",
                &match last {
                    Some(l) => format!("{:.0} min · now {:.0} fps at {:.0} draws · average {:.1} fps · {:.1} GB", started.elapsed().as_secs() / 60, l.fps, l.draws, avg, mem),
                    None => format!("{:.0} s · starting", started.elapsed().as_secs()),
                },
                -1.0,
            );
            std::thread::sleep(Duration::from_millis(1000));
        };
        let _ = std::fs::copy(&log, config::logs_dir().join("game-last.log"));
        let minutes = started.elapsed().as_secs_f64() / 60.0;
        let watch = std::mem::take(&mut *watch.lock().unwrap());
        let outcome = if watch.lost_device {
            "graphics card lost (device removed)".to_owned()
        } else if watch.panicked {
            "crashed (panic in the layer)".to_owned()
        } else {
            match code {
                Some(c) if c.success() => "closed normally".to_owned(),
                Some(c) => format!("ended with {c}"),
                None => "ended".to_owned(),
            }
        };
        let mut st = s.lock().unwrap();
        st.step = format!("Game ended: {outcome}");
        st.detail = String::new();
        st.session = Some(Session { minutes, outcome, watch, preset: preset.name.to_owned() });
        Ok(())
    })
}
