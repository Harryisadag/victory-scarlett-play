//! The NVIDIA-RT game layer (Kemerd/VIctory-NVIDIA-RT, branch `engine-rt` of this repository): its `victory play`
//! command does what its `PLAY.cmd` does, from the choices in `Config::rt`. NVIDIA's DLSS and frame generation
//! files are NVIDIA's: `victory get-dlss` fetches them from NVIDIA's own GitHub into `%LOCALAPPDATA%\victory`;
//! they are never shipped here.

use crate::config::{self, Config, RtSettings};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The layer's `victory.exe`: `victory-rt\` beside the launcher (release zip), or `VSP_RT_EXE` (development).
pub fn exe() -> Option<PathBuf> {
    let named = std::env::var_os("VSP_RT_EXE").map(PathBuf::from);
    let beside = std::env::current_exe().ok().and_then(|me| me.parent().map(|d| d.join("victory-rt").join("victory.exe")));
    named.into_iter().chain(beside).find(|p| p.is_file())
}

/// `%LOCALAPPDATA%\victory`: where the layer keeps its F1 menu's settings, its logs and NVIDIA's files.
fn victory_home() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("victory")
}

/// NVIDIA's files are there (`victory get-dlss` has run).
pub fn has_dlss() -> bool {
    victory_home().join("nvidia-sdk").join("Streamline").join("bin").join("x64").join("sl.interposer.dll").is_file()
}

/// `victory get-dlss` (about 70 MB from github.com/NVIDIA-RTX/Streamline).
pub fn get_dlss_command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.arg("get-dlss");
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd
}

/// The F1 menu's own copies of what the launcher sets: dropped before a start so the launcher's choices win (every
/// other F1 choice is kept), as `PLAY.cmd` does.
const LAUNCHER_OWNS: &[&str] = &[
    "VICTORY_MENU_RES=",
    "VICTORY_MENU_DLSS=",
    "VICTORY_MENU_FRAMEGEN=",
    "VICTORY_MENU_FPS=",
    "VICTORY_MENU_CONSOLE=",
    "VICTORY_HDR=",
    "VICTORY_DISPLAY_WIDE=",
    "VICTORY_RT_NATIVE=",
    "VICTORY_UI_LETTERBOX=",
    "VICTORY_FULLSCREEN=",
    "VICTORY_REFLEX=",
];

fn drop_menu_overrides() {
    let file = victory_home().join("victory-settings.txt");
    let Ok(text) = std::fs::read_to_string(&file) else { return };
    let kept: Vec<&str> = text.lines().filter(|l| !LAUNCHER_OWNS.iter().any(|k| l.trim_start().to_ascii_uppercase().starts_with(k))).collect();
    if kept.len() != text.lines().count() {
        let _ = std::fs::write(&file, kept.join("\r\n") + "\r\n");
    }
}

/// The fork's "fast" speed (`PLAY.cmd`'s `:fast`): the set that measured fastest.
const FAST: &[(&str, &str)] = &[
    ("VICTORY_PIPELINE_WAIT_ONCE", "1"),
    ("VICTORY_PIPELINE_WAIT_BACKLOG", "64"),
    ("VICTORY_PIPELINE_SLOW_WORKERS", "8"),
    ("VICTORY_TEXTURE_LOOK_ONCE", "1"),
    ("VICTORY_TEXTURE_READ_AHEAD", "1"),
    ("VICTORY_STAGING_NO_ZERO", "1"),
    ("VICTORY_SEND_COPIED_FIND", "1"),
    ("VICTORY_FILES_BUFFERED", "1"),
    ("VICTORY_MEMORY_FRESH", "1"),
    ("VICTORY_MEMORY_ARENAS_AHEAD", "1"),
    ("VICTORY_MEMORY_QUIET", "1"),
    ("VICTORY_MEMORY_LAND_SENT", "1"),
    ("VICTORY_MEMORY_ASK_RUNS", "1"),
    ("VICTORY_MEMORY_LEAN", "1"),
    ("VICTORY_MEMORY_DIRTY", "1"),
    ("VICTORY_MEMORY_BUFFER_FAULTS", "1"),
    ("VICTORY_MEMORY_FAULT_PAGES", "40000"),
    ("VICTORY_REFRESH_PROBE_US", "300"),
    ("VICTORY_DECLARE_ACROSS_LISTS", "1"),
    ("VICTORY_REFRESH_PROBE_TEXTURES", "1"),
    ("VICTORY_REFRESH_SPLIT", "1"),
    ("VICTORY_WAIT_SPLIT", "1"),
    ("VICTORY_DSTORAGE_STATS", "1"),
    ("VICTORY_PIPELINE_PIECES", "1"),
    ("VICTORY_PIPELINE_PIECES_REFUSE_UNTRANSLATED", "1"),
    ("VICTORY_PIPELINE_PIECES_ONLY", "graphics"),
    ("VICTORY_REFRESH_SET", "1"),
    ("VICTORY_TITLE_WRITE_RANGE", "1"),
    ("VICTORY_UNDER_PARTS", "1"),
    ("VICTORY_SEND_THREAD", "1"),
];

/// `victory play`'s arguments for these choices (`series_s`: the console mode the launcher picked).
/// `mouse`: mouse look (the pointer captured in the window, F2 or ~ frees it). `cut_hold`: the title waits at a
/// scene change until the scene has loaded (`VICTORY_CUT_HOLD=wait`, from the Scarlett build).
pub fn args(rt: &RtSettings, series_s: bool, mouse: bool, cut_hold: bool) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut set = |k: &str, v: &str| {
        a.push("--set".into());
        a.push(format!("{k}={v}"));
    };
    let (res, wide) = match rt.res.as_str() {
        "1080to1440" | "4k" | "wide" | "1440" => (rt.res.as_str(), None),
        // Kemerd 65e88194: 21:9 is a picture size of its own.
        "3440x1440" => ("ultrawide", None),
        other if other.contains('x') => ("1440", Some(other.to_owned())),
        _ => ("1440", None),
    };
    if let Some(w) = &wide {
        set("VICTORY_DISPLAY_WIDE", w);
    }
    set("VICTORY_HDR", if rt.hdr { "1" } else { "0" });
    if rt.ui_stretch {
        set("VICTORY_UI_LETTERBOX", "0");
    }
    if rt.reflex != "off" {
        set("VICTORY_REFLEX", &rt.reflex);
    }
    set("VICTORY_FULLSCREEN", if rt.windowed { "0" } else { "1" });
    if rt.ray_tracing {
        set("VICTORY_RT_NATIVE", "1");
    } else {
        // Scarlett's light probes (engine-rt 3c15c86b, from Op-lite/victory-scarlett 183ac80f): the probes trace
        // against empty structures instead of keeping stale memory (violet vegetation, wrong indoor light).
        if rt.light != "fill" {
            set("VICTORY_PROBE_RAYS", "1");
        }
        // An empty value turns the layer's fill light off.
        if rt.light == "probes" {
            set("VICTORY_AMBIENT", "");
        }
    }
    // The title reads the mouse device only while the pointer is captured (superboynick's capture, trustno1
    // 06afe633: off unless asked for); the toggle key is this repository's (engine-rt).
    if mouse {
        set("VICTORY_MOUSE_CAPTURE", "1");
        set("VICTORY_MOUSE_TOGGLE", "1");
    }
    // The official release's loading fix, always (as PLAY.cmd).
    set("VICTORY_PIPELINE_SAMPLER_TABLE", "1");
    // The title's own watchdog ends the process when its main thread stands 30 s (a cold shader cache's first
    // menu, a long pipeline compile): patched out, only where the bytes of v1.0.2.1 match (engine-rt, from the
    // Scarlett build / megafork 382c129d).
    set("VICTORY_NO_WATCHDOG", "1");
    if rt.scarlett_fixes {
        set("VICTORY_PLANE_NOW", "1");
        set("VICTORY_DSTORAGE_WORKERS", "physical");
    }
    if cut_hold {
        set("VICTORY_CUT_HOLD", "wait");
    }
    if rt.fast {
        for (k, v) in FAST {
            set(k, v);
        }
    }
    let mut out = vec!["--res".to_owned(), res.to_owned(), "--dlss".to_owned(), rt.dlss.clone(), "--fps".to_owned(), rt.fps_cap.clamp(1, 1000).to_string()];
    // Which frame generation (trustno1 29d8dbf7): DLSS's, AMD FSR 3's, or auto (DLSS where the card has it).
    if matches!(rt.fg_type.as_str(), "auto" | "dlss" | "fsr") {
        out.push("--fg-type".into());
        out.push(rt.fg_type.clone());
    }
    if series_s {
        out.push("--series-s".into());
    }
    match rt.framegen {
        0 => {}
        1 => out.push("--no-framegen-ready".into()),
        n => {
            out.push("--framegen".into());
            out.push(n.clamp(2, 6).to_string());
        }
    }
    out.extend(a);
    out
}

/// A `victory play` command for the game in `cfg` (any VICTORY_* the launcher was started with is dropped: the layer
/// sets its own; only the paths are given).
pub fn play_command(exe: &Path, cfg: &Config) -> Result<Command, String> {
    let game_dir = cfg.game_dir.as_ref().ok_or("no game folder")?;
    drop_menu_overrides();
    let mut cmd = Command::new(exe);
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("VICTORY_") {
            cmd.env_remove(&k);
        }
    }
    cmd.env("VICTORY_GAME_DIR", game_dir);
    cmd.env("VICTORY_CACHE_DIR", config::rt_cache_dir());
    cmd.env("VICTORY_DATA_DIR", config::saves_dir());
    // Plain log lines: the colour codes it writes otherwise hid the frame-rate lines from the launcher.
    cmd.env("NO_COLOR", "1");
    // AMD's FidelityFX DLLs for FSR 3 frame generation, where the launcher fetched them.
    cmd.env("VICTORY_FIDELITYFX_DIR", crate::fsr::dir());
    cmd.arg("play").arg(crate::game::game_exe(game_dir)).args(args(&cfg.rt, cfg.rt.series_s, cfg.kbm, cfg.rt.cut_hold));
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    Ok(cmd)
}

/// The crash files the layer leaves beside its exe (`crash-<run>.log`, `crash-<run>.dmp`) written after `since`.
pub fn crash_files_since(exe: &Path, since: std::time::SystemTime) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else { return Vec::new() };
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| { let n = n.to_string_lossy(); (n.starts_with("crash-") || n.contains("-crash-")) && (n.ends_with(".log") || n.ends_with(".dmp")) }))
        .filter(|p| std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t >= since))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_play_cmd() {
        let a = args(&RtSettings::default(), false, true, true);
        assert!(a.contains(&"VICTORY_NO_WATCHDOG=1".to_owned()) && a.contains(&"VICTORY_PLANE_NOW=1".to_owned()) && a.contains(&"VICTORY_CUT_HOLD=wait".to_owned()));
        assert!(a.contains(&"VICTORY_MOUSE_CAPTURE=1".to_owned()) && a.contains(&"VICTORY_MOUSE_TOGGLE=1".to_owned()));
        assert_eq!(&a[..6], ["--res", "1440", "--dlss", "quality", "--fps", "1000"]);
        assert!(a.contains(&"VICTORY_REFLEX=boost".to_owned()));
        assert!(a.contains(&"VICTORY_FULLSCREEN=0".to_owned()));
        assert!(a.contains(&"VICTORY_SEND_THREAD=1".to_owned()));
        assert!(!a.iter().any(|x| x == "--framegen" || x == "--no-framegen-ready" || x == "--series-s"));
        assert!(a.contains(&"VICTORY_PROBE_RAYS=1".to_owned()) && !a.iter().any(|x| x.starts_with("VICTORY_AMBIENT")));
        let fill = args(&RtSettings { light: "fill".into(), ..Default::default() }, false, false, false);
        assert!(!fill.iter().any(|x| x.starts_with("VICTORY_PROBE_RAYS")));
        let probes = args(&RtSettings { light: "probes".into(), ..Default::default() }, false, false, false);
        assert!(probes.contains(&"VICTORY_AMBIENT=".to_owned()));
    }

    #[test]
    fn ultrawide_and_framegen() {
        let rt = RtSettings { res: "3440x1440".into(), framegen: 4, reflex: "off".into(), fast: false, ..Default::default() };
        let a = args(&RtSettings { scarlett_fixes: false, ..rt }, true, false, false);
        assert!(!a.iter().any(|x| x.starts_with("VICTORY_PLANE_NOW") || x.starts_with("VICTORY_CUT_HOLD")));
        assert!(!a.iter().any(|x| x.starts_with("VICTORY_MOUSE")));
        assert_eq!(&a[..2], ["--res", "ultrawide"]);
        assert!(a.windows(2).any(|w| w == ["--framegen", "4"]));
        assert!(a.contains(&"--series-s".to_owned()));
        assert!(!a.iter().any(|x| x.starts_with("VICTORY_REFLEX") || x.starts_with("VICTORY_SEND_THREAD")));
    }
}
