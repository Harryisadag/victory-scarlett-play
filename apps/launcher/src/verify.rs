//! "Verify game files": checks a dump folder for completeness, reading metadata only (never writes, moves or
//! deletes). The lists of the title's top-level files and packs follow ScarlettPort's launcher (`launcher/src/
//! verify.rs`, ZigZagBoi1/ScarlettPort, GPL-3.0-or-later).

use std::path::Path;

/// The title's top-level files: the executable, its config and the archives.
const TOP: [&str; 14] = [
    "game_scarlett_master.exe",
    "MicrosoftGame.config",
    "anim.rpf",
    "appdata0.rpf",
    "audio.rpf",
    "common.rpf",
    "data.rpf",
    "levels.rpf",
    "movies.rpf",
    "navmeshes.rpf",
    "packs.rpf",
    "script.rpf",
    "shaders.rpf",
    "textures.rpf",
];

/// The packs under `pack\`: the title crashes at boot without them.
const PACKS: [&str; 9] = [
    "core_bvh4",
    "core_licenses",
    "core_restofworld",
    "core_restofworld001",
    "shared_minigame000",
    "shared_minigame000_bvh4",
    "story000",
    "story000_bvh4",
    "story000_restofworld",
];

pub struct Report {
    /// Problems: each one names a file and what is wrong with it.
    pub problems: Vec<String>,
    pub files: u64,
    pub bytes: u64,
}

impl Report {
    pub fn summary(&self) -> String {
        if self.problems.is_empty() {
            format!("Game files look complete: {} files, {:.1} GB.", self.files, self.bytes as f64 / 1e9)
        } else {
            format!(
                "{} problem(s) in the game folder - copy the dump again from your console for these:\n{}",
                self.problems.len(),
                self.problems.iter().map(|p| format!("  • {p}")).collect::<Vec<_>>().join("\n")
            )
        }
    }
}

pub fn check(dir: &Path) -> Report {
    let mut r = Report { problems: Vec::new(), files: 0, bytes: 0 };
    for name in TOP {
        // Windows names are case-insensitive: metadata finds `MicrosoftGame.Config` too.
        match std::fs::metadata(dir.join(name)) {
            Ok(m) if m.len() == 0 => r.problems.push(format!("{name} is empty")),
            Ok(_) => {}
            Err(_) => r.problems.push(format!("{name} is missing")),
        }
    }
    let pack = dir.join("pack");
    if !pack.is_dir() {
        r.problems.push("the pack folder is missing (the game crashes at boot without it)".into());
    } else {
        for p in PACKS {
            let d = pack.join(p);
            let has_archive = std::fs::read_dir(&d).into_iter().flatten().flatten().any(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("rpf")));
            if !has_archive {
                r.problems.push(format!("pack\\{p} is missing or has no archive"));
            }
        }
    }
    walk(dir, &mut r, 0);
    r
}

/// Counts every file, and names empty ones and links that lead nowhere.
fn walk(dir: &Path, r: &mut Report, depth: usize) {
    if depth > 8 {
        return;
    }
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        // `metadata` follows links: one that fails leads nowhere.
        match std::fs::metadata(&path) {
            Err(_) => r.problems.push(format!("{} cannot be read (a broken link or a damaged file)", path.display())),
            Ok(m) if m.is_dir() => walk(&path, r, depth + 1),
            Ok(m) => {
                r.files += 1;
                r.bytes += m.len();
                if m.len() == 0 && path.extension().is_some_and(|x| x.eq_ignore_ascii_case("rpf")) {
                    r.problems.push(format!("{} is empty", path.display()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_empty_folder_has_every_problem() {
        let dir = std::env::temp_dir().join(format!("vsp-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = super::check(&dir);
        assert_eq!(r.problems.len(), 15);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
