//! NVIDIA-RT settings by kind of PC. The launcher picks one from the hardware (the card's video memory, whether it
//! has DLSS frame generation, the CPU's threads), says which, and the player can pick another or change any setting
//! (the preset then reads "Custom"). From what was measured on 2026-11-27: an RTX 5070 Ti + Ryzen 7 3700X ran best
//! at 1440 DLSS Quality Series X with frame generation ready; an RTX 3060 Ti (8 GB) at 4K filled its card and spilled
//! 8-12 GB into the PC's memory; the layer is limited by one CPU thread in dense places.

use crate::config::RtSettings;
use crate::hardware::Hardware;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    HighEnd,
    Mid,
    Vram8,
    CpuLimited,
}

pub const ALL: [Preset; 4] = [Preset::HighEnd, Preset::Mid, Preset::Vram8, Preset::CpuLimited];

impl Preset {
    pub fn name(self) -> &'static str {
        match self {
            Preset::HighEnd => "high-end",
            Preset::Mid => "mid",
            Preset::Vram8 => "8gb",
            Preset::CpuLimited => "cpu-limited",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::HighEnd => "High-end (RTX 4080/5080/5090 class, 16 GB+)",
            Preset::Mid => "Mid (12-16 GB cards, 8-core CPUs)",
            Preset::Vram8 => "8-10 GB cards (RTX 3060 Ti / 3070 / 4060 class)",
            Preset::CpuLimited => "CPU-limited (6-core and older CPUs)",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Preset::HighEnd => "2560x1440, DLSS Quality, Series X, frame generation 2x from the start",
            Preset::Mid => "2560x1440, DLSS Quality, Series X, frame generation ready (F1)",
            Preset::Vram8 => "2560x1440, Series S, DLSS Performance, frame generation not loaded, textures one level down",
            Preset::CpuLimited => "1080p upscaled to 1440, DLSS Quality, frame generation 2x (DLSS on RTX 40/50, else FSR 3), Reflex boost",
        }
    }

    /// Puts the preset's settings into `rt` for a PC with `screen_height` pixels of monitor (0: unknown); the
    /// player's own choices (window, lighting, HDR, mouse...) are kept.
    #[cfg(test)]
    pub fn apply(self, rt: &mut RtSettings) {
        self.apply_for(rt, 0);
    }

    /// As [`Preset::apply`], with the monitor's height: on a 4K monitor the high-end and mid presets output the
    /// game's own 3840x2160 with DLSS Performance (the 2560x1440 output patch leaves the interface laid out for 4K:
    /// too big and cut off, a player's 4K monitor, 2026-11-27; the scene is drawn at 1920x1080 inside either way).
    pub fn apply_for(self, rt: &mut RtSettings, screen_height: u32) {
        rt.ray_tracing = false;
        rt.mip_drop = 0;
        rt.reflex = "boost".into();
        match self {
            Preset::HighEnd => {
                rt.res = if rt.res == "4k" || screen_height >= 2160 { "4k".into() } else { "1440".into() };
                rt.dlss = if rt.res == "4k" { "performance".into() } else { "quality".into() };
                rt.series_s = false;
                rt.framegen = 2;
            }
            Preset::Mid => {
                rt.res = if screen_height >= 2160 { "4k".into() } else { "1440".into() };
                rt.dlss = if rt.res == "4k" { "performance".into() } else { "quality".into() };
                rt.series_s = false;
                rt.framegen = 0;
            }
            Preset::Vram8 => crate::rt::fit_low_vram(rt),
            Preset::CpuLimited => {
                rt.res = "1080to1440".into();
                rt.dlss = "quality".into();
                rt.series_s = false;
                rt.framegen = 2;
                rt.fg_type = "auto".into();
            }
        }
    }
}

/// The preset this PC gets.
pub fn pick(hw: &Hardware) -> Preset {
    let vram = hw.vram_gb;
    if vram > 0.0 && vram < 10.0 {
        return Preset::Vram8;
    }
    // 6 cores / 12 threads or fewer: the layer's preparing thread and the title's own threads crowd each other.
    if hw.threads > 0 && hw.threads <= 12 {
        return Preset::CpuLimited;
    }
    let gpu = hw.gpu.to_ascii_uppercase();
    let top = ["4080", "4090", "5080", "5090"].iter().any(|m| gpu.contains(m));
    if top && vram >= 15.0 && hw.threads >= 16 {
        return Preset::HighEnd;
    }
    Preset::Mid
}

/// Which preset `rt` holds exactly for this monitor, if any (to show "Custom" once the player changed something).
pub fn matching(rt: &RtSettings, screen_height: u32) -> Option<Preset> {
    ALL.into_iter().find(|p| {
        let mut probe = rt.clone();
        p.apply_for(&mut probe, screen_height);
        probe == *rt
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(gpu: &str, vram: f64, threads: usize) -> Hardware {
        Hardware { gpu: gpu.into(), vram_gb: vram, threads, ..Default::default() }
    }

    #[test]
    fn the_pcs_seen_get_their_preset() {
        assert_eq!(pick(&hw("NVIDIA GeForce RTX 5070 Ti", 15.6, 16)), Preset::Mid);
        assert_eq!(pick(&hw("NVIDIA GeForce RTX 3060 Ti", 7.8, 16)), Preset::Vram8);
        assert_eq!(pick(&hw("NVIDIA GeForce RTX 5090", 31.5, 32)), Preset::HighEnd);
        assert_eq!(pick(&hw("NVIDIA GeForce RTX 4070 Ti", 11.7, 12)), Preset::CpuLimited);
    }

    #[test]
    fn a_preset_is_recognised_until_a_setting_changes() {
        let mut rt = RtSettings::default();
        Preset::Vram8.apply(&mut rt);
        assert_eq!(matching(&rt, 1440), Some(Preset::Vram8));
        rt.res = "4k".into();
        assert_eq!(matching(&rt, 1440), None);
        let mut rt = RtSettings::default();
        Preset::Mid.apply(&mut rt);
        assert_eq!(matching(&rt, 1440), Some(Preset::Mid));
        // On a 4K monitor the mid preset outputs 4K with DLSS Performance.
        let mut rt = RtSettings::default();
        Preset::Mid.apply_for(&mut rt, 2160);
        assert_eq!((rt.res.as_str(), rt.dlss.as_str()), ("4k", "performance"));
        assert_eq!(matching(&rt, 2160), Some(Preset::Mid));
    }
}
