//! The settings the game is started with: Scarlett's measured set (`scripts/play-scarlett.ps1`), then what the
//! preset, the lighting choice and the hooks change.

use crate::config::{Config, Lighting};
use crate::hardware::{Hardware, Vendor};

pub struct Preset {
    pub name: &'static str,
    pub label: &'static str,
    pub about: &'static str,
    /// `--console` for victory.exe.
    pub console: &'static str,
    pub env: &'static [(&'static str, &'static str)],
    pub tested: bool,
}

pub const ALL: &[Preset] = &[
    Preset {
        name: "nvidia-16",
        label: "NVIDIA, 12 GB or more",
        about: "Scarlett's own settings (Series X mode). Tested: RTX 5070 Ti 16 GB, RTX 4080 SUPER.",
        console: "series-x",
        env: &[],
        tested: true,
    },
    Preset {
        name: "low-vram",
        label: "Less than 12 GB of VRAM",
        about: "Series S mode: the game renders smaller and keeps less in memory. Untested on such cards.",
        console: "series-s",
        env: &[("VICTORY_POOL_CAP", "1")],
        tested: false,
    },
    Preset {
        name: "amd",
        label: "AMD Radeon",
        about: "The settings of the AMD base this build comes from (avatarmanz, RX 7900 XTX): recording on the game's own threads, the card kept busy. Untested here.",
        console: "series-x",
        env: &[("VICTORY_THIN", "1"), ("VICTORY_CARD_AWAKE", "1")],
        tested: false,
    },
];

pub fn by_name(name: &str) -> Option<&'static Preset> {
    ALL.iter().find(|p| p.name == name)
}

/// The preset this hardware gets unless the player picks another.
pub fn pick(hw: &Hardware) -> &'static Preset {
    if hw.vram_gb > 0.0 && hw.vram_gb < 11.5 {
        return by_name("low-vram").unwrap();
    }
    if hw.vendor == Vendor::Amd {
        return by_name("amd").unwrap();
    }
    by_name("nvidia-16").unwrap()
}

pub fn chosen<'a>(cfg: &Config, hw: &Hardware) -> &'a Preset {
    cfg.preset.as_deref().and_then(by_name).unwrap_or_else(|| pick(hw))
}

/// Scarlett's standing set (scripts/play-scarlett.ps1 at bc264a4, with the pipeline book).
const BASE: &[(&str, &str)] = &[
    ("VICTORY_JOIN_COPIES", "1"),
    ("VICTORY_REFRESH_ONCE", "1"),
    ("VICTORY_CARD_REHASH_RUNS", "1"),
    ("VICTORY_BACK_OWN_VIEW", "1"),
    ("VICTORY_WIDE_MERGE", "1"),
    ("VICTORY_MIRROR_FAST", "1"),
    ("VICTORY_FRAME_LOG", "1"),
    ("VICTORY_CARD_METER", "1"),
    ("VICTORY_PIPELINE_WAIT_MS", "500"),
    ("VICTORY_ASYNC_SUBMIT", "1"),
    ("VICTORY_QUEUE2", "1"),
    ("VICTORY_INDIRECT_WINDOW", "1"),
    ("VICTORY_TEXTURE_NO_WAIT", "1"),
    ("VICTORY_COPY_PLAIN", "1"),
    ("VICTORY_WIDE_TEXTURES", "1"),
    ("VICTORY_INDIRECT_GATHER", "1"),
    ("VICTORY_LATE_WAIT_LISTS", "1024"),
    ("VICTORY_CARD_FILLS_BATCH", "0"),
    ("VICTORY_GATHER_DISPATCH", "0"),
    ("VICTORY_RELAY", "2"),
    ("VICTORY_WIDE_BACK", "1"),
    ("VICTORY_RELAY_COPY", "1"),
    ("VICTORY_CARD_STEPS", "2"),
    ("VICTORY_INDIRECT_BATCH", "2"),
    ("VICTORY_MAP_THREADS", "1"),
    ("VICTORY_CARD_AWAKE", "0"),
    ("VICTORY_SHOW_AFTER", "1"),
    ("VICTORY_JOIN_PARTS", "8"),
    ("VICTORY_UPLOAD_AHEAD", "1"),
    ("VICTORY_CARD_BUFFERS_MIN_KB", "64"),
    ("VICTORY_LEVELS_ON_DEMAND", "0"),
    ("VICTORY_POOL_DISCARD", "1"),
    ("VICTORY_FILL_TITLE", "1"),
    ("VICTORY_PARTIAL_DELTA", "1"),
    ("VICTORY_BAKE_DELTA", "1"),
    ("VICTORY_MAP_LOCK_FINE", "1"),
    ("VICTORY_POOL_CAP", "1"),
    ("VICTORY_TABLE_MEMO", "1"),
    ("VICTORY_LIST_ALIAS_EXACT", "1"),
    ("VICTORY_LIST_HOLD", "1"),
    ("VICTORY_BAKE_TEXTURES_MS", "10000"),
    ("VICTORY_PLANE_NOW", "1"),
    ("VICTORY_PIPELINE_BOOK", "1"),
    // Scarlett bc264a4: the freeze fix (batch page-mapping copies' views mapped only when touched: 5-second periods
    // with a freeze 10-12 -> 3), static-sampler pipelines compiled each run instead of loaded from the library
    // (wrong light or faces in some runs), table slots a program can read only, texture-queue marks.
    ("VICTORY_SOFT_VIEWS", "copies"),
    // Only the colour grading (the one program Scarlett saw sample through another run's samplers) is compiled every
    // run; =1 compiled all ~1,150 pipelines with static samplers every launch: minutes of late scenes on an 8-core CPU.
    ("VICTORY_SAMPLERS_MADE", "CompositeNMB"),
    ("VICTORY_TABLE_NAMED_ONLY", "1"),
    ("VICTORY_QUEUE_MARKS", "1"),
    // megafork 382c129d: the title's watchdog ends the process on purpose when its main thread stalls ~30 s (a long
    // pipeline compile, the scene-change wait, a slow load); patched out (only where the exact bytes of v1.0.2.1 match).
    ("VICTORY_NO_WATCHDOG", "1"),
    // One storage worker per physical core instead of per logical processor (16 on a Ryzen 7 3700X): SMT twins add
    // little to reading and unpacking and crowd the submitting thread and the copies, the limit on such CPUs (the
    // switch's author on an 8c/16t PC: copying back 2.4-3.5 ms/MB with 8 against 5.7 with 16; a session here spent
    // 25-34 ms a cutscene frame copying back).
    ("VICTORY_DSTORAGE_WORKERS", "physical"),
];

/// Everything that changes the shader translation (a change needs the shaders translated again).
pub fn translation_key(cfg: &Config) -> String {
    let rays = cfg.lighting == Lighting::RayTraced;
    format!("probe={}|rays={}", if rays { "" } else { "1" }, if rays { "1" } else { "" })
}

/// The environment and `--console` for a run of the game.
pub fn environment(cfg: &Config, hw: &Hardware) -> (Vec<(String, String)>, &'static str) {
    let preset = chosen(cfg, hw);
    let mut env: Vec<(String, String)> = BASE.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let mut set = |k: &str, v: &str| {
        if let Some(e) = env.iter_mut().find(|(n, _)| n == k) {
            e.1 = v.to_owned();
        } else {
            env.push((k.to_owned(), v.to_owned()));
        }
    };
    for (k, v) in preset.env {
        set(k, v);
    }
    match cfg.lighting {
        Lighting::Fast => set("VICTORY_PROBE_RAYS", "1"),
        Lighting::Balanced => {
            set("VICTORY_PROBE_RAYS", "1");
            set("VICTORY_AMBIENT", "0.8");
        }
        Lighting::RayTraced => set("VICTORY_BVH_BUILDER", "1"),
    }
    if (cfg.render_scale - 1.0).abs() > 0.01 {
        set("VICTORY_RENDER_SCALE", &format!("{:.2}", cfg.render_scale));
    }
    if cfg.kbm {
        set("VICTORY_KBM_GTA", "1");
        set("VICTORY_MOUSE_SENS", &format!("{:.2}", cfg.mouse_sensitivity.clamp(0.1, 5.0)));
        if cfg.mouse_invert_y {
            set("VICTORY_MOUSE_INVERT_Y", "1");
        }
    }
    if cfg.cut_hold {
        // The title itself waits at a scene change until its reads and pipelines are there (at most 3 s).
        set("VICTORY_CUT_HOLD", "wait");
    }
    if cfg.hooks {
        set("VICTORY_GPU_ONLY", "1");
        set("VICTORY_KNOWN_WRITES", "1");
    }
    let console = match cfg.console.as_deref() {
        Some("series-x") => "series-x",
        Some("series-s") => "series-s",
        _ => preset.console,
    };
    (env, console)
}
