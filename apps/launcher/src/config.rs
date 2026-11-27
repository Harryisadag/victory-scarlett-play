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
    /// `1440`, `1080to1440`, `4k`, `wide` (5120x1440), or `3440x1440` (21:9, `--res ultrawide`).
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
    /// Light with ray tracing off: `both` (Scarlett's probes, `VICTORY_PROBE_RAYS`, plus the layer's fill light),
    /// `probes` (Scarlett's: probes only, fill light off), `fill` (the layer's own: fill light only).
    pub light: String,
    /// Fixes ported from the Scarlett build (engine-rt): depth planes given their twin before use
    /// (`VICTORY_PLANE_NOW`), storage workers per physical core (`VICTORY_DSTORAGE_WORKERS=physical`).
    pub scarlett_fixes: bool,
    /// The title waits at a scene change until the scene has loaded (`VICTORY_CUT_HOLD=wait`). Off by default: in
    /// the open world (Vice) the streaming bursts it watches come every few seconds, and one session waited 142
    /// times, 48 s in all, ten of them the full 3 s (2026-11-27).
    pub cut_hold: bool,
    /// Which frame generation: `auto` (DLSS where the card has it, else AMD FSR 3), `dlss`, `fsr`.
    pub fg_type: String,
    /// Fixes taken from other forks, newest and not yet measured here (engine-rt): the partial texture refresh
    /// (Ahmedx360, `VICTORY_TEXTURE_PARTS`).
    pub partial_textures: bool,
    /// A pipeline's first draw waits up to this long for it instead of being dropped (`VICTORY_PIPELINE_WAIT_FIRST_MS`,
    /// inkVI via gurvancampion): fewer missing objects on a fresh cache, more stutter. 0: off.
    pub first_wait_ms: u32,
    /// Textures are refreshed only once title memory holds still (`VICTORY_TEXTURE_NO_WAIT=0`): victory-sley-fork
    /// measured no more pixelated, noisy skin on characters. Off: the layer's own (=1).
    pub clean_skin: bool,
    /// Indoor floors' reflections held to the room's light, not the sky (`VICTORY_REFLECT_BOUND=0.15`): the
    /// frosted floors indoors. Off: outdoors it may dull cars' and windows' reflections (not tried yet).
    pub indoor_reflections: bool,
    /// Large textures start this many levels down (`VICTORY_TEXTURE_MIP_DROP`, LightYearZh; 0..=3): for 8 GB cards.
    pub mip_drop: u32,
    /// Kemerd's pipelines made from the title's loose pieces (`VICTORY_PIPELINE_PIECES`, part of the fast set): more
    /// draws happen, but some vertex inputs are guessed; suspected of the meshes seen distorted in the plane
    /// (2026-11-27). Only with `fast`.
    pub pieces: bool,
}

impl Default for RtSettings {
    fn default() -> Self {
        Self {
            res: "1440".into(),
            // In a window by default: borderless fullscreen covered the whole screen (player's report, 2026-11-27).
            windowed: true,
            dlss: "quality".into(),
            framegen: 0,
            reflex: "boost".into(),
            hdr: false,
            fps_cap: 1000,
            ui_stretch: false,
            fast: true,
            ray_tracing: false,
            series_s: false,
            light: "both".into(),
            scarlett_fixes: true,
            cut_hold: false,
            fg_type: "auto".into(),
            partial_textures: false,
            first_wait_ms: 0,
            clean_skin: false,
            indoor_reflections: false,
            mip_drop: 0,
            pieces: true,
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
    /// Every save is zipped into `save-backups` before each Play (the newest 10 kept).
    pub backup_saves: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            game_dir: None,
            // NVIDIA-RT first: on the player's RTX 5070 Ti it ran 1.5-2x Scarlett's frame rate at the same draws
            // (2026-11-27). An AMD card is given Scarlett on a first start (main.rs).
            engine: Engine::NvidiaRt,
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
            backup_saves: true,
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
