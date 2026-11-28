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
session; a summary of the game's log (its settings, frame rate over time, warnings and errors; your Windows user name \
is blanked out), and the end of the log when it crashes; and (if you allow it) up to 4 small screenshots of the game window, taken only while the game is the window in front. \
No names, accounts, files or IP-based tracking; an anonymous random id tells reports from one PC apart.";

pub fn build(cfg: &Config, hw: &Hardware, s: &Session, build: &str) -> serde_json::Value {
    let (avg, low, heavy) = s.watch.summary();
    let failed = s.watch.lost_device || s.watch.panicked || !s.outcome.starts_with("closed");
    let log_tail = digest(&std::fs::read_to_string(&s.log).unwrap_or_default(), failed);
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
        "pc_preset": crate::pcpresets::matching(&cfg.rt, 0).map_or("custom", |p| p.name()),
        "preset": if cfg.engine == crate::config::Engine::NvidiaRt {
            // The relay shows 60 characters.
            format!("[{}] NV-RT {} dlss:{} fg:{} rfx:{}{}{}{}", crate::pcpresets::matching(&cfg.rt, 0).map_or("custom", |p| p.name()), cfg.rt.res, cfg.rt.dlss, cfg.rt.framegen, cfg.rt.reflex,
                if cfg.rt.fast { " fast" } else { " plain" }, if cfg.rt.ray_tracing { " RT" } else { "" }, if cfg.rt.hdr { " HDR" } else { "" })
        } else {
            format!("{} / {:?}{}", s.preset, cfg.lighting, if cfg.hooks { " / hooks" } else { "" })
        },
        "crash_files": s.crash_files.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "tdr_delay": crate::hardware::tdr_delay(),
        "outcome": s.outcome,
        "hint": s.hint,
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

/// The report's view of a session's log, at most `MOST` bytes (the relay keeps 60 KB): the settings and start lines,
/// the frame rate every 30 s, every warning and error grouped with how often it came, the last of the layer's
/// statistics lines, and the end of the log when the session went wrong. Windows user names in paths are blanked.
fn digest(log: &str, failed: bool) -> String {
    const MOST: usize = 56 * 1024;
    let clean = |l: &str| -> String {
        let l: String = l.chars().take(400).collect();
        blank_user(&l)
    };
    let lines: Vec<&str> = log.lines().collect();
    let mut out = String::new();
    let mut part = |title: &str, body: Vec<String>| {
        if !body.is_empty() {
            out.push_str(&format!("===== {title}\n"));
            for l in body {
                out.push_str(&l);
                out.push('\n');
            }
        }
    };
    // Settings and the start: the first lines that are not the per-second statistics.
    let start: Vec<String> = lines.iter().filter(|l| !l.contains(" INFO victory_gpu::host::lean")).take(60).map(|l| clean(l)).collect();
    part("start", start);
    // The frame rate every 30 s (the layer writes it every second).
    let fps: Vec<String> = lines
        .iter()
        .filter(|l| l.contains(" fps, ") && l.contains("ms/frame"))
        .step_by(30)
        .map(|l| clean(l.split_once("victory_gpu::host: ").map_or(l, |(_, r)| r)).chars().take(200).collect())
        .collect();
    part("frame rate every 30 s", fps);
    // Warnings and errors, grouped by their text with the numbers taken out.
    let mut seen: Vec<(String, usize, String)> = Vec::new();
    for l in lines.iter().filter(|l| l.contains(" WARN ") || l.contains(" ERROR ") || l.contains("panicked") || l.contains("unhandled exception")) {
        let text = l.split_once(" WARN ").or_else(|| l.split_once(" ERROR ")).map_or(*l, |(_, r)| r);
        let key: String = text.chars().filter(|c| !c.is_ascii_digit()).take(140).collect();
        if let Some(e) = seen.iter_mut().find(|(k, _, _)| *k == key) {
            e.1 += 1;
        } else if seen.len() < 60 {
            seen.push((key, 1, clean(l)));
        }
    }
    part("warnings and errors (count, first one)", seen.into_iter().map(|(_, n, first)| format!("x{n}  {first}")).collect());
    // The last of the statistics lines that say most about a session.
    let keys = [
        "pipeline library saved",
        "shaders from the cache and translated",
        "texture queue's room",
        "landing per frame",
        "preparing per frame, the submitting thread",
        "the card's clock per frame",
        "DLSS Frame Generation:",
        "frame times",
    ];
    let last: Vec<String> = keys.iter().filter_map(|k| lines.iter().rev().find(|l| l.contains(k)).map(|l| clean(l))).collect();
    part("last statistics", last);
    if failed {
        let n = lines.len().saturating_sub(150);
        part("end of the log", lines[n..].iter().map(|l| clean(l)).collect());
    }
    if out.len() > MOST {
        let mut cut = MOST;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

/// A Windows user folder's name blanked: `C:\Users\<name>` (either slash, any case) becomes `C:\Users\<user>`.
fn blank_user(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while let Some(at) = lower[i..].find("users").map(|k| k + i) {
        let after = at + "users".len();
        let sep = line[after..].chars().next();
        let before_ok = at > 0 && matches!(&line[at - 1..at], "\\" | "/");
        if before_ok && matches!(sep, Some('\\' | '/')) {
            let name_start = after + 1;
            let name_end = line[name_start..].find(['\\', '/', ' ', '"', '\'']).map_or(line.len(), |k| name_start + k);
            out.push_str(&line[i..name_start]);
            out.push_str("<user>");
            i = name_end;
        } else {
            out.push_str(&line[i..after]);
            i = after;
        }
    }
    out.push_str(&line[i..]);
    out
}

#[cfg(test)]
mod digest_tests {
    #[test]
    fn user_names_are_blanked() {
        assert_eq!(super::blank_user(r"file=C:\Users\Frizky\AppData\Local\x.log"), r"file=C:\Users\<user>\AppData\Local\x.log");
        assert_eq!(super::blank_user("c:/users/bob/x and users said"), "c:/users/<user>/x and users said");
    }

    #[test]
    fn a_digest_groups_warnings_and_keeps_the_settings() {
        let log = "# console=series-x switches: VICTORY_X=1\n\
                   2026 WARN victory_gpu::pipeline: shader does not translate hash=1\n\
                   2026 WARN victory_gpu::pipeline: shader does not translate hash=2\n\
                   2026 INFO victory_gpu::host: 12.0 fps, 83.0 ms/frame (40 preparing), 5000 draws/frame\n";
        let d = super::digest(log, false);
        assert!(d.contains("console=series-x"), "{d}");
        assert!(d.contains("x2  2026 WARN"), "{d}");
        assert!(d.contains("12.0 fps"), "{d}");
        assert!(!d.contains("end of the log"));
    }
}
