//! The launcher's settings, kept in `%LOCALAPPDATA%\victory-scarlett-play\launcher.json`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lighting {
    /// Ray tracing off, no stand-in light: fastest, dark shadows.
    Fast,
    /// Ray tracing off, a stand-in fill light in the shadows (`VICTORY_AMBIENT`).
    Balanced,
    /// The title's own ray tracing (`VICTORY_BVH_BUILDER=1`): right, much slower.
    RayTraced,
}

/// Which game layer runs the title.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Engine {
    /// The Scarlett build (`victory\victory.exe`): the launcher's own settings, setup and recording.
    Scarlett,
    /// Kemerd/VIctory-NVIDIA-RT (`victory-rt\victory.exe`, its `play` command): DLSS, frame generation, Reflex,
    /// HDR10, ultrawide; its own F1 menu in the game.
    NvidiaRt,
}

/// What `victory play` of the NVIDIA-RT layer is started with (its `PLAY.cmd`'s ten choices).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RtSettings {
    /// `1440`, `1080to1440`, `4k`, `wide` (5120x1440), or `3440x1440` (21:9, `--res 1440` with
    /// `VICTORY_DISPLAY_WIDE`).
    pub res: String,
    pub windowed: bool,
    /// `off`, `dlaa`, `quality`, `balanced`, `performance`.
    pub dlss: String,
    /// 0: ready, switched on in F1; 1: not loaded; 2..=6: on from the start, that many frames shown per rendered one.
    pub framegen: u32,
    /// `boost`, `on`, `off`.
    pub reflex: String,
    pub hdr: bool,
    /// Frames a second at most; 1000: no cap.
    pub fps_cap: u32,
    /// Interface stretched on ultrawide screens instead of kept 16:9 in the middle.
    pub ui_stretch: bool,
    /// The fork's measured fast switches (its "fast" speed); off: the official release's settings.
    pub fast: bool,
    /// The title's own ray tracing on the RTX cores (`VICTORY_RT_NATIVE`, experimental, 5-8 fps).
    pub ray_tracing: bool,
    /// The Series S profile (1280x720 inside: faster, much softer); the layer's author: Series X is the one to use.
    pub series_s: bool,
}

impl Default for RtSettings {
    fn default() -> Self {
        Self {
            res: "1440".into(),
            windowed: false,
            dlss: "quality".into(),
            framegen: 0,
            reflex: "boost".into(),
            hdr: false,
            fps_cap: 1000,
            ui_stretch: false,
            fast: true,
            ray_tracing: false,
            series_s: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The folder holding `game_scarlett_master.exe`.
    pub game_dir: Option<PathBuf>,
    pub engine: Engine,
    pub rt: RtSettings,
    /// The launcher's window is minimised while the game runs and comes back when it ends.
    pub minimize_while_playing: bool,
    /// A preset name from `presets::ALL`, or `None` for the one picked from the hardware.
    pub preset: Option<String>,
    pub lighting: Lighting,
    /// `Some("series-x")` / `Some("series-s")`: the console the game is told it runs on; `None`: the preset's.
    /// Series S renders at a lower resolution with smaller memory budgets (helps the graphics card, not the CPU).
    pub console: Option<String>,
    /// The title's internal render scale: 1.0 (full), 0.75 or 0.5 (`VICTORY_RENDER_SCALE`; less work for the graphics
    /// card, softer picture).
    pub render_scale: f32,
    /// Scarlett's memory hooks (faster; a few small textures can show old contents for a moment).
    pub hooks: bool,
    /// Experimental: keep the last picture at a scene change while streaming catches up (`VICTORY_CUT_HOLD=1`).
    pub cut_hold: bool,
    /// Mouse and keyboard in a GTA V PC layout (`VICTORY_KBM_GTA=1`; F2 or ~ locks the mouse for looking).
    pub kbm: bool,
    pub mouse_sensitivity: f32,
    pub mouse_invert_y: bool,
    /// `None`: not answered yet - reports are on by default (the first-start notice says so); `Some(false)`: off.
    pub reports: Option<bool>,
    pub report_screenshots: bool,
    /// Random id of this install (no personal data): lets reports from one PC be told apart.
    pub install_id: String,
    /// The `victory.exe` build and game folder the first-run setup was done for.
    pub setup_done_for: Option<String>,
    pub check_updates: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            game_dir: None,
            engine: Engine::Scarlett,
            rt: RtSettings::default(),
            minimize_while_playing: true,
            preset: None,
            lighting: Lighting::Fast,
            console: None,
            render_scale: 1.0,
            hooks: true,
            cut_hold: true,
            kbm: true,
            mouse_sensitivity: 1.0,
            mouse_invert_y: false,
            reports: None,
            report_screenshots: true,
            install_id: new_id(),
            setup_done_for: None,
            check_updates: true,
        }
    }
}

/// `%LOCALAPPDATA%\victory-scarlett-play`: settings, caches, saves, logs.
pub fn home() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("victory-scarlett-play")
}

pub fn cache_dir() -> PathBuf {
    home().join("cache")
}

/// The NVIDIA-RT layer's caches: its translator differs from Scarlett's, so they are kept apart.
pub fn rt_cache_dir() -> PathBuf {
    home().join("cache-rt")
}

/// Saves are the title's own and shared by both layers.
pub fn saves_dir() -> PathBuf {
    home().join("saves")
}

pub fn logs_dir() -> PathBuf {
    home().join("logs")
}

fn file() -> PathBuf {
    home().join("launcher.json")
}

impl Config {
    pub fn load() -> Self {
        let c: Config = std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if c.install_id.len() < 16 {
            let c = Config { install_id: new_id(), ..c };
            c.save();
            return c;
        }
        c
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(home());
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let tmp = file().with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, file());
            }
        }
    }
}

/// 128 random bits as hex, from the OS's random source (through the std hasher's random keys and the clock).
fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}
