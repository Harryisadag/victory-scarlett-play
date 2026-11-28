//! Checks before Play and plain-language hints after a session that went wrong (ideas from dazacode/vilauncher
//! 1.1.0, written here: its "check the game's program before Play" and its messages for known failures).

use std::path::Path;

/// Free bytes on the drive that holds `path` (0 if Windows will not say).
pub fn free_bytes(path: &Path) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        match probe.parent() {
            Some(p) => probe = p.to_path_buf(),
            None => return 0,
        }
    }
    let wide: Vec<u16> = probe.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    // SAFETY: a NUL-terminated path and a pointer to a u64.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    if ok == 0 { 0 } else { free }
}

/// Below this the shader cache cannot be written and the game stalls on a black screen (a player's full C:
/// drive, 2026-11-27: "There is not enough space on the disk" while translating shaders).
pub const TOO_FULL_GB: f64 = 8.0;
/// Below this Play goes on, with a warning.
pub const LOW_GB: f64 = 15.0;

/// What to say about free space on the cache's drive: `Err` stops Play, `Ok(Some)` warns.
pub fn space(cache: &Path) -> Result<Option<String>, String> {
    let free = free_bytes(cache) as f64 / 1e9;
    if free <= 0.0 {
        return Ok(None);
    }
    let drive = cache.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
    if free < TOO_FULL_GB {
        return Err(format!(
            "Your {drive} drive has only {free:.1} GB free. The game's shader cache needs room to save shaders, and with a \
             full drive the game stalls on a black screen. Free up at least 20 GB on {drive}, then press Play again."
        ));
    }
    Ok((free < LOW_GB).then(|| format!("Low disk space: {free:.1} GB free on {drive}. Keep about 20 GB free for the shader cache.")))
}

/// Whether `exe` looks like a 64-bit Windows program (an `MZ` header whose PE header names x64), else what is wrong.
pub fn game_program(exe: &Path) -> Result<(), String> {
    let name = exe.display();
    let bytes = std::fs::read(exe).map_err(|e| format!("Cannot read the game's program {name}: {e}"))?;
    let ok = (|| {
        if bytes.get(..2)? != b"MZ" {
            return None;
        }
        let pe = u32::from_le_bytes(bytes.get(0x3c..0x40)?.try_into().ok()?) as usize;
        if bytes.get(pe..pe + 4)? != b"PE\0\0" {
            return None;
        }
        let machine = u16::from_le_bytes(bytes.get(pe + 4..pe + 6)?.try_into().ok()?);
        Some(machine == 0x8664)
    })();
    match ok {
        Some(true) => Ok(()),
        Some(false) => Err(format!("{name} is not a 64-bit x64 program. Pick the folder of your own Xbox Series dump.")),
        None => Err(format!("{name} is not a valid Windows program (damaged or incomplete copy?). Copy the game folder again.")),
    }
}

/// A plain-language hint for a session that went wrong, from its log and how it ended.
pub fn hint(log_text: &str, lost_device: bool) -> Option<&'static str> {
    let has = |s: &str| log_text.contains(s);
    if has("not enough space on the disk") || has("(os error 112)") {
        Some("Your drive ran out of space while saving shaders. Free up at least 20 GB on the drive with your user folder, then play again.")
    } else if has("0x887E0003") || has("D3D12 SDK version configuration") {
        Some("Windows refused the Direct3D 12 version. Update Windows, or delete the D3D12 folder next to victory.exe and start again.")
    } else if has("0x8007000E") || has("E_OUTOFMEMORY") || has("out of memory") {
        Some("The PC ran out of memory. Close other programs, check the page file is on, or pick the 8 GB preset.")
    } else if lost_device || has("0x887A0005") || has("DEVICE_REMOVED") || has("device removed") {
        Some("The graphics driver reset. Remove any GPU overclock, update the NVIDIA driver, and try again (Tools: GPU timeout to 10 s).")
    } else if has("shader model 6.6") && has("refus") {
        Some("The graphics driver lacks shader model 6.6. Update the NVIDIA driver.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hints_name_the_cause() {
        let disk = "WARN shader does not translate error=There is not enough space on the disk. (os error 112)";
        assert!(super::hint(disk, false).unwrap().contains("space"));
        assert!(super::hint("all fine", true).unwrap().contains("driver"));
        assert!(super::hint("all fine", false).is_none());
    }

    #[test]
    fn a_text_file_is_not_a_program() {
        let dir = std::env::temp_dir().join("vsp-checks-test");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("not-a-game.exe");
        std::fs::write(&f, b"hello").unwrap();
        assert!(super::game_program(&f).is_err());
    }
}
