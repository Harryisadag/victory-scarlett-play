//! Reports after a session: hardware, performance, the end of the log on a failure, a few small screenshots.
//! Sent to the report relay (`relay/`), which forwards them to the developer's private Discord channel. On by
//! default; a notice on the first start says so, and the player can turn them off there or in Settings, after which
//! nothing is sent.

use crate::config::Config;
use crate::hardware::Hardware;
use crate::jobs::Session;
use base64::Engine;

/// The relay's address (`relay/README.md`). Set at build time with `VSP_RELAY`; empty: reports are off.
pub const RELAY: &str = match option_env!("VSP_RELAY") {
    Some(r) => r,
    None => "",
};

pub fn available() -> bool {
    !RELAY.is_empty()
}

/// What a report holds, shown to the player before they agree.
pub const WHAT: &str = "Your CPU, GPU, graphics driver, RAM and Windows version; the frame rate and memory use of each \
session; the end of the game's log when it crashes; and (if you allow it) up to 4 small screenshots of the game window, taken only while the game is the window in front. \
No names, accounts, files or IP-based tracking; an anonymous random id tells reports from one PC apart.";

pub fn build(cfg: &Config, hw: &Hardware, s: &Session, build: &str) -> serde_json::Value {
    let (avg, low, heavy) = s.watch.summary();
    let failed = s.watch.lost_device || s.watch.panicked || !s.outcome.starts_with("closed");
    let log_tail = if failed {
        s.watch.tail.iter().rev().take(250).collect::<Vec<_>>().into_iter().rev().cloned().collect::<Vec<_>>().join("\n")
    } else {
        String::new()
    };
    let shots: Vec<String> = if cfg.report_screenshots {
        s.watch.shots.iter().map(|j| base64::engine::general_purpose::STANDARD.encode(j)).collect()
    } else {
        Vec::new()
    };
    serde_json::json!({
        "kind": "victory-scarlett-play-report",
        "install_id": cfg.install_id,
        "launcher_version": crate::update::VERSION,
        "build": build,
        "engine": format!("{:?}", cfg.engine),
        "preset": if cfg.engine == crate::config::Engine::NvidiaRt {
            // The relay shows 60 characters.
            format!("NV-RT {} dlss:{} fg:{} rfx:{}{}{}{}", cfg.rt.res, cfg.rt.dlss, cfg.rt.framegen, cfg.rt.reflex,
                if cfg.rt.fast { " fast" } else { " plain" }, if cfg.rt.ray_tracing { " RT" } else { "" }, if cfg.rt.hdr { " HDR" } else { "" })
        } else {
            format!("{} / {:?}{}", s.preset, cfg.lighting, if cfg.hooks { " / hooks" } else { "" })
        },
        "crash_files": s.crash_files.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "tdr_delay": crate::hardware::tdr_delay(),
        "outcome": s.outcome,
        "hardware": {
            "cpu": hw.cpu, "threads": hw.threads, "ram_gb": hw.ram_gb, "commit_limit_gb": hw.commit_limit_gb,
            "gpu": hw.gpu, "vram_gb": hw.vram_gb, "driver": hw.driver, "windows": hw.windows,
        },
        "performance": {
            "minutes": s.minutes, "fps_avg": avg, "fps_low": low, "fps_heavy": heavy,
            "peak_private_gb": s.watch.peak_private_gb,
        },
        "log_tail": log_tail,
        "screenshots": shots,
    })
}

pub fn send(report: &serde_json::Value) -> Result<(), String> {
    if !available() {
        return Err("reports are not set up in this build".into());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .user_agent(format!("victory-scarlett-play-launcher/{}", crate::update::VERSION))
        .build()
        .into();
    agent.post(&format!("{}/report", RELAY.trim_end_matches('/'))).send_json(report).map_err(|e| e.to_string())?;
    Ok(())
}
