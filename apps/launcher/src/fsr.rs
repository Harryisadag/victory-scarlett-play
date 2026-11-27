//! AMD FSR 3 frame generation for the NVIDIA-RT layer on cards without DLSS frame generation (RTX 20/30, AMD,
//! Intel; trustno1-coder/VIctory-NVIDIA 29d8dbf7). It needs two of AMD's signed DLLs from the FidelityFX SDK. They
//! are AMD's (redistributable in binary form under AMD's licence, which is saved beside them) and are not shipped
//! here: like NVIDIA's files, they are fetched on the player's PC, from AMD's own GitHub, at one pinned version,
//! and checked against the SHA-256 of the copies whose Authenticode signature (Advanced Micro Devices) was verified
//! when this list was made (2026-11-27).

use sha2::{Digest, Sha256};
use std::path::PathBuf;

const BASE: &str = "https://raw.githubusercontent.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/v2.4.0/Kits/FidelityFX";

/// (path in the SDK, file name, SHA-256; `None`: the licence text, kept as it comes)
const FILES: [(&str, &str, Option<&str>); 3] = [
    ("signedbin", "amd_fidelityfx_loader_dx12.dll", Some("e2191d3c0828be45eb8affd4e3ec0378f2293adc345b772a7170edcf3b021ecb")),
    ("signedbin", "amd_fidelityfx_framegeneration_dx12.dll", Some("e6843252f58b0341a0d80f6a20bc0d840643ee95bc48c22cac08745344577bb6")),
    ("docs", "license.md", None),
];

/// `%LOCALAPPDATA%\victory-scarlett-play\fidelityfx`, given to the layer as `VICTORY_FIDELITYFX_DIR`.
pub fn dir() -> PathBuf {
    crate::config::home().join("fidelityfx")
}

/// Both DLLs are there.
pub fn ready() -> bool {
    FILES.iter().filter(|f| f.2.is_some()).all(|(_, name, _)| dir().join(name).is_file())
}

/// Whether this card has DLSS frame generation (an NVIDIA RTX 40 or 50); elsewhere FSR 3's is the one there is.
pub fn dlss_framegen_card(gpu: &str) -> bool {
    let g = gpu.to_ascii_uppercase();
    g.contains("NVIDIA") && (g.contains("RTX 40") || g.contains("RTX 50") || g.contains("RTX PRO") || g.contains(" ADA"))
}

/// Downloads the three files (about 30 MB); `progress` gets 0..1.
pub fn download(progress: &dyn Fn(f32)) -> Result<(), String> {
    use std::io::Read;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(300)))
        .user_agent(format!("victory-scarlett-play-launcher/{}", crate::update::VERSION))
        .build()
        .into();
    let to = dir();
    std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
    for (k, (folder, name, sha)) in FILES.iter().enumerate() {
        let mut resp = agent.get(&format!("{BASE}/{folder}/{name}")).call().map_err(|e| format!("{name}: {e}"))?;
        let mut data = Vec::new();
        resp.body_mut().with_config().limit(64 << 20).reader().read_to_end(&mut data).map_err(|e| format!("{name}: {e}"))?;
        if let Some(want) = sha {
            let got: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
            if &got != want {
                return Err(format!("{name} is not the file expected (SHA-256 {got}); nothing was kept"));
            }
        }
        let file = to.join(if sha.is_some() { (*name).to_owned() } else { "AMD-FidelityFX-license.md".to_owned() });
        let tmp = file.with_extension("part");
        std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
        progress((k + 1) as f32 / FILES.len() as f32);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn cards_with_dlss_frame_generation() {
        assert!(super::dlss_framegen_card("NVIDIA GeForce RTX 5070 Ti"));
        assert!(super::dlss_framegen_card("NVIDIA GeForce RTX 4060 Laptop GPU"));
        assert!(!super::dlss_framegen_card("NVIDIA GeForce RTX 3080"));
        assert!(!super::dlss_framegen_card("AMD Radeon RX 7900 XTX"));
    }
}
