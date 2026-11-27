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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The folder holding `game_scarlett_master.exe`.
    pub game_dir: Option<PathBuf>,
    /// A preset name from `presets::ALL`, or `None` for the one picked from the hardware.
    pub preset: Option<String>,
    pub lighting: Lighting,
    /// `Some("series-x")` / `Some("series-s")`: the console the game is told it runs on; `None`: the preset's.
    /// Series S renders at a lower resolution with smaller memory budgets (helps the graphics card, not the CPU).
    pub console: Option<String>,
    /// Scarlett's memory hooks (faster; a few small textures can show old contents for a moment).
    pub hooks: bool,
    /// Experimental: keep the last picture at a scene change while streaming catches up (`VICTORY_CUT_HOLD=1`).
    pub cut_hold: bool,
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
            preset: None,
            lighting: Lighting::Fast,
            console: None,
            hooks: false,
            cut_hold: false,
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
