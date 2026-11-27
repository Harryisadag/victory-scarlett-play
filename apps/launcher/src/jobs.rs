//! Work done in the background while the window shows how it goes: the first-run setup and playing.

use crate::config::{self, Config, Engine};
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
    /// Crash files the layer wrote (copied into the logs folder).
    pub crash_files: Vec<PathBuf>,
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
        "{}|{}|{}|{}|{}",
        game::build_stamp(exe),
        presets::translation_key(cfg),
        cfg.preset.as_deref().unwrap_or("auto"),
        cfg.console.as_deref().unwrap_or("preset"),
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
        // `victory warm` says "<done> of <total>, <seconds> s".
        let words: Vec<&str> = last.split_whitespace().collect();
        let fraction = match words.as_slice() {
            [done, "of", total, ..] => match (done.parse::<f32>(), total.trim_end_matches(',').parse::<f32>()) {
                (Ok(d), Ok(t)) if t > 0.0 => (d / t).min(1.0),
                _ => -1.0,
            },
            _ => -1.0,
        };
        say(
            s,
            "Compiling the recorded graphics pipelines ahead",
            &format!("{} s - {}", started.elapsed().as_secs(), last.chars().take(140).collect::<String>()),
            fraction,
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Whether an earlier setup already recorded the opening's pipelines (they stay valid across builds and settings: an
/// update only needs the shaders translated again and the recorded pipelines compiled).
pub fn recorded(cfg: &Config) -> bool {
    let pipelines = config::cache_dir().join("pipelines");
    if !pipelines.join("known-pipelines.bin").is_file() {
        return false;
    }
    // Ray tracing uses programs of its own: a recording made without it does not cover them. The marker names the
    // lighting each recording was made with; a recording from before markers counts for the non-ray-traced modes.
    let marker = pipelines.join(recorded_marker(cfg));
    marker.is_file() || (cfg.lighting != crate::config::Lighting::RayTraced && !pipelines.join(recorded_marker_rt()).is_file() && !any_marker(&pipelines))
}

fn recorded_marker(cfg: &Config) -> &'static str {
    if cfg.lighting == crate::config::Lighting::RayTraced { recorded_marker_rt() } else { "recorded-raster.txt" }
}

fn recorded_marker_rt() -> &'static str {
    "recorded-raytraced.txt"
}

fn any_marker(dir: &Path) -> bool {
    dir.join("recorded-raster.txt").is_file() || dir.join(recorded_marker_rt()).is_file()
}

/// Plays the opening unattended from a fresh start, so every pipeline it uses is recorded, then stops it.
fn record_opening(exe: &Path, cfg: &Config, hw: &Hardware, s: &Arc<Mutex<Status>>, cancel: &AtomicBool) -> Result<(), String> {
    let game_dir = cfg.game_dir.clone().ok_or("no game folder")?;
    let fresh = config::home().join("setup-saves");
    let _ = std::fs::remove_dir_all(&fresh);
    std::fs::create_dir_all(&fresh).map_err(|e| e.to_string())?;
    let (mut cmd, console) = game::command(exe, cfg, hw, &fresh);
    cmd.env("VICTORY_AUTO_INPUT", AUTO_START);
    // Nothing is shown (the window stays black): the opening is not spoiled for someone who has not played it, and
    // its slow, unfinished first-compile frames are not their first look at the game. Everything is still drawn,
    // so every pipeline is recorded; the scripted input counts draws at the title's frame waits, not at presents.
    cmd.env("VICTORY_PRESENT_SKIP", "1");
    cmd.arg("run").arg(game::game_exe(&game_dir)).arg("--console").arg(console).arg("--execute");
    let watch = Arc::new(Mutex::new(Watch::default()));
    let mut child = game::spawn_logged(cmd, config::logs_dir().join("setup-opening.log"), watch.clone(), None)
        .map_err(|e| format!("could not start the game: {e}"))?;
    let started = Instant::now();
    let pid = child.id();
    // Silent: the game's audio session is muted (it opens one a few seconds in, so this is repeated). Windows
    // remembers an app's mute; the marker tells Play to unmute once if this run is cut off before unmuting.
    let marker = config::home().join("recording-muted");
    let _ = std::fs::write(&marker, pid.to_string());
    let mut next_mute = Instant::now();
    let r = loop {
        let t = started.elapsed().as_secs();
        if Instant::now() >= next_mute {
            crate::audio::set_mute(pid, true);
            next_mute = Instant::now() + Duration::from_secs(2);
        }
        if cancel.load(Ordering::Relaxed) {
            break Err("cancelled".to_owned());
        }
        if let Some(code) = child.try_wait().map_err(|e| e.to_string())? {
            // The game ended by itself: whatever it recorded is kept.
            // A recording cut short (the game crashed) covers too little: not counted as done.
            break if t >= RECORD_SECONDS * 2 / 3 {
                Ok(())
            } else {
                Err(format!(
                    "the game stopped after {t} s ({code}) while recording the opening - the recording is not complete. With ray tracing on this can crash; try Standard lighting. Log: {}",
                    config::logs_dir().join("setup-opening.log").display()
                ))
            };
        }
        if t >= RECORD_SECONDS {
            break Ok(());
        }
        let fps = watch.lock().unwrap().samples.last().map(|x| x.fps).unwrap_or(0.0);
        say(
            s,
            "Recording the opening's graphics - a black game window is normal; please don't touch it",
            &format!("{} of {} min · {:.0} fps", t / 60, RECORD_SECONDS / 60, fps),
            t as f32 / RECORD_SECONDS as f32,
        );
        std::thread::sleep(Duration::from_millis(500));
    };
    // Unmuted before it ends, so the mute Windows remembers for victory.exe does not carry over to Play.
    crate::audio::set_mute(pid, false);
    let _ = std::fs::remove_file(&marker);
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
            let _ = std::fs::write(config::cache_dir().join("pipelines").join(recorded_marker(&cfg)), "recorded
");
        }
        warm(&exe, &cfg, &hw, s, cancel)?;
        say(s, "Setup finished", "", 1.0);
        Ok(())
    })
}

/// Plays: brings the caches up to date if needed (Scarlett), starts the game with the chosen layer, watches it until
/// it ends.
pub fn play(exe: PathBuf, cfg: Config, hw: Hardware, refresh: bool, screenshots: bool) -> Job {
    Job::start(move |s, cancel| {
        let game_dir = cfg.game_dir.clone().ok_or("no game folder")?;
        let preset = presets::chosen(&cfg, &hw);
        let rt = cfg.engine == Engine::NvidiaRt;
        let started_at = std::time::SystemTime::now();
        let cmd = if rt {
            let mut cmd = crate::rt::play_command(&exe, &cfg)?;
            // (NVIDIA-RT: `refresh` asks for every shader to be made first, with this start's settings, then the game.)
            if refresh {
                cmd.arg("--preload");
            }
            cmd
        } else {
            if refresh {
                translate(&exe, &cfg, &hw, s, cancel)?;
            }
            warm(&exe, &cfg, &hw, s, cancel)?;
            let (mut cmd, console) = game::command(&exe, &cfg, &hw, &config::saves_dir());
            cmd.arg("run").arg(game::game_exe(&game_dir)).arg("--console").arg(console).arg("--execute");
            cmd
        };
        let watch = Arc::new(Mutex::new(Watch::default()));
        let stamp = started_at.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut log = config::logs_dir().join(format!("game-{stamp}.log"));
        let mut child = game::spawn_logged(cmd, log.clone(), watch.clone(), None).map_err(|e| format!("could not start the game: {e}"))?;
        let started = Instant::now();
        // (NVIDIA-RT) The first start of a new build of the layer stays at about 14 fps for the whole session and
        // runs normally from the second start on (trustno1-coder 10862a6d): it is played 20 s after its first
        // frame, ended and started again, once per build. Done here, not by the layer itself
        // (VICTORY_WARMUP_RESTART=0), so the launcher keeps the game it watches.
        let warm_mark = config::home().join("warmed-rt").join(game::build_stamp(&exe));
        let mut warm_from: Option<Instant> = None;
        let mut warming = rt && !warm_mark.exists();
        // A game the layer started again by itself (F1's "save and restart"): followed by its process id.
        let mut restarted: Option<u32> = None;
        let mut next_shot = Duration::from_secs(4 * 60);
        // Windows remembers an app's mute, and a setup recording that ended without unmuting (the game crashed)
        // leaves victory.exe muted: every Play unmutes the game's audio once it has opened (within its first minute).
        let marker = config::home().join("recording-muted");
        let mut unmute_left = 60;
        let code = loop {
            if let Some(pid) = restarted {
                if cancel.load(Ordering::Relaxed) {
                    game::end(pid);
                }
                if !game::alive(pid) {
                    break None;
                }
                say(s, "Playing", &format!("{:.0} min · the game started itself again (F1) · no frame figures from here", started.elapsed().as_secs() / 60), -1.0);
                std::thread::sleep(Duration::from_millis(1000));
                continue;
            }
            if let Some(code) = child.try_wait().map_err(|e| e.to_string())? {
                // The layer may have started itself again (F1's save and restart): follow that one.
                if rt {
                    let mut again = None;
                    for _ in 0..10 {
                        again = game::find_process(&exe, child.id());
                        if again.is_some() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    if let Some(pid) = again {
                        restarted = Some(pid);
                        continue;
                    }
                }
                break Some(code);
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                break child.wait().ok();
            }
            if warming {
                let drawing = watch.lock().unwrap().samples.iter().any(|x| x.draws > 0.0);
                if warm_from.is_none() && drawing {
                    warm_from = Some(Instant::now());
                }
                if warm_from.is_some_and(|t| t.elapsed() >= Duration::from_secs(20)) {
                    warming = false;
                    say(s, "First start of this version: starting the game again once", "a new version's first run otherwise stays slow", -1.0);
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(dir) = warm_mark.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(&warm_mark, "warmed\r\n");
                    let cmd = crate::rt::play_command(&exe, &cfg)?;
                    log = config::logs_dir().join(format!("game-{stamp}-2.log"));
                    // What the warm-up measured is not the session's.
                    *watch.lock().unwrap() = Watch::default();
                    child = game::spawn_logged(cmd, log.clone(), watch.clone(), None).map_err(|e| format!("could not start the game again: {e}"))?;
                    unmute_left = 60;
                    continue;
                }
            }
            if unmute_left > 0 {
                unmute_left -= 1;
                if crate::audio::set_mute(child.id(), false) > 0 || unmute_left == 0 {
                    unmute_left = 0;
                    let _ = std::fs::remove_file(&marker);
                }
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
            // A preload under way (before the first frame): its step, how far, and the time left.
            if last.is_none()
                && let Some((step, fraction, detail)) = preload_progress(&watch.lock().unwrap().tail)
            {
                say(s, step, &detail, fraction);
                std::thread::sleep(Duration::from_millis(1000));
                continue;
            }
            say(
                s,
                "Playing",
                &match last {
                    Some(l) if warming => format!("first start of this version: it starts again by itself once, 20 s in · now {:.0} fps", l.fps),
                    Some(l) => format!("{:.0} min · now {:.0} fps at {:.0} draws · average {:.1} fps · {:.1} GB", started.elapsed().as_secs() / 60, l.fps, l.draws, avg, mem),
                    // Before the first frame: the layer's last line (a preload's progress, the start's steps).
                    None => format!(
                        "{:.0} s · {}",
                        started.elapsed().as_secs(),
                        watch.lock().unwrap().tail.back().map(|l| l.chars().take(120).collect::<String>()).unwrap_or_else(|| "starting".into())
                    ),
                },
                -1.0,
            );
            std::thread::sleep(Duration::from_millis(1000));
        };
        let _ = std::fs::copy(&log, config::logs_dir().join("game-last.log"));
        let minutes = started.elapsed().as_secs_f64() / 60.0;
        let watch = std::mem::take(&mut *watch.lock().unwrap());
        // The NVIDIA-RT layer leaves crash-<run>.log and .dmp beside its exe: copied to the logs folder.
        let mut crash_files = Vec::new();
        if rt {
            for f in crate::rt::crash_files_since(&exe, started_at) {
                if let Some(name) = f.file_name() {
                    let to = config::logs_dir().join(name);
                    if std::fs::copy(&f, &to).is_ok() {
                        crash_files.push(to);
                    }
                }
            }
        }
        let outcome = if watch.lost_device {
            "graphics card lost (device removed)".to_owned()
        } else if watch.panicked {
            "crashed (panic in the layer)".to_owned()
        } else if !crash_files.is_empty() {
            "crashed (crash report written)".to_owned()
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
        let preset = if rt { "nvidia-rt".to_owned() } else { preset.name.to_owned() };
        st.session = Some(Session { minutes, outcome, watch, preset, crash_files });
        Ok(())
    })
}

/// Fetches NVIDIA's DLSS and frame generation files with the NVIDIA-RT layer's `victory get-dlss`.
/// Fetches AMD's FidelityFX DLLs for FSR 3 frame generation (`crate::fsr`).
pub fn get_fsr() -> Job {
    Job::start(move |s, _cancel| {
        let step = "Downloading AMD's FSR 3 frame generation files from AMD's GitHub (about 30 MB)";
        say(s, step, "", 0.0);
        let st = s.clone();
        crate::fsr::download(&move |f| say(&st, step, "", f))?;
        say(s, "FSR 3 files ready", "", 1.0);
        Ok(())
    })
}

pub fn get_dlss(exe: PathBuf) -> Job {
    Job::start(move |s, cancel| {
        say(s, "Downloading NVIDIA's DLSS files from NVIDIA's GitHub (about 70 MB)", "", -1.0);
        let watch = Arc::new(Mutex::new(Watch::default()));
        let mut child = game::spawn_logged(crate::rt::get_dlss_command(&exe), config::logs_dir().join("get-dlss.log"), watch.clone(), None)
            .map_err(|e| format!("could not start victory.exe: {e}"))?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                return Err("cancelled".into());
            }
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                break;
            }
            let last = watch.lock().unwrap().tail.back().cloned().unwrap_or_default();
            say(s, "Downloading NVIDIA's DLSS files from NVIDIA's GitHub (about 70 MB)", &last.chars().take(140).collect::<String>(), -1.0);
            std::thread::sleep(Duration::from_millis(500));
        }
        if crate::rt::has_dlss() {
            say(s, "DLSS files ready", "", 1.0);
            Ok(())
        } else {
            Err(format!("the DLSS files did not arrive; see {}", config::logs_dir().join("get-dlss.log").display()))
        }
    })
}

/// Shader programs a preload translates (`victory preload`'s first step; 26,084 in v1.0.2.1).
const PRELOAD_SHADERS: f64 = 26_084.0;

/// Where a preload (`victory play --preload`) is, from its last lines: its first step says
/// "<n> shaders in the cache, <s> s", its second "<done> of <total>, <s> s" (pipelines). The step's name, its
/// fraction done, and the detail with the time left (from the step's own pace so far).
fn preload_progress(tail: &std::collections::VecDeque<String>) -> Option<(&'static str, f32, String)> {
    let minutes = |secs: f64| {
        if secs < 90.0 { "under 2 min left".to_owned() } else { format!("about {:.0} min left", (secs / 60.0).round()) }
    };
    for line in tail.iter().rev().take(40) {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            [done, "of", total, secs, "s", ..] => {
                let (Ok(done), Ok(total), Ok(secs)) =
                    (done.parse::<f64>(), total.trim_end_matches(',').parse::<f64>(), secs.parse::<f64>())
                else {
                    continue;
                };
                if total <= 0.0 {
                    continue;
                }
                let rate = if secs > 0.0 { done / secs } else { 0.0 };
                let left = if rate > 0.0 { minutes((total - done) / rate) } else { "working out the time left".to_owned() };
                return Some((
                    "Building shaders ahead (step 2 of 2: pipelines)",
                    (done / total).min(1.0) as f32,
                    format!("{:.0} of {:.0} · {left} · then the game starts", done, total),
                ));
            }
            [n, "shaders", "in", "the", "cache,", ..] => {
                let Ok(n) = n.parse::<f64>() else { continue };
                return Some((
                    "Building shaders ahead (step 1 of 2: translating)",
                    (n / PRELOAD_SHADERS).min(1.0) as f32,
                    format!("{:.0} of about {:.0} shaders · a minute or two, then the pipelines", n, PRELOAD_SHADERS),
                ));
            }
            _ if line.contains("Preloading shaders") => {
                return Some(("Building shaders ahead", -1.0, "starting".to_owned()));
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod preload_tests {
    #[test]
    fn a_preload_says_its_step_and_time_left() {
        let tail: std::collections::VecDeque<String> =
            ["Preloading shaders: every shader of the title is translated", "26000 shaders in the cache, 66 s", "19000 of 88657, 537 s"]
                .into_iter()
                .map(String::from)
                .collect();
        let (step, fraction, detail) = super::preload_progress(&tail).unwrap();
        assert!(step.contains("step 2 of 2"));
        assert!((fraction - 19000.0 / 88657.0).abs() < 1e-4);
        // 69,657 left at 35.4 a second: about 33 min.
        assert!(detail.starts_with("19000 of 88657 · about 33 min left"), "{detail}");
        let first: std::collections::VecDeque<String> = ["7000 shaders in the cache, 57 s".to_owned()].into_iter().collect();
        assert!(super::preload_progress(&first).unwrap().0.contains("step 1 of 2"));
        let none: std::collections::VecDeque<String> = ["INFO victory_gpu::host: host D3D12 device ready".to_owned()].into_iter().collect();
        assert!(super::preload_progress(&none).is_none());
    }
}
