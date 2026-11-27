//! Saves: list them, import them from other builds (any VIctory-based layer's data folder, XCL's game saves, a zip
//! this launcher exported), export and back them up. Never while the game runs.
//!
//! The layer keeps each save slot (a "container", e.g. `SARC0015`, the story save; `Player`; the profile settings)
//! as `<data>\<title>\saves\<user>\<scid>\<name>~<fnv64 of name>\v<N>\`, with the blobs as files `0`, `1`, ... and a
//! `manifest` (u32-length-prefixed strings: name, display name, each blob's name). The highest readable version is
//! the one the game loads. An import writes the save as a new, higher version of its slot: what was there stays
//! on disk under it.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const TITLE: &str = "7952987A";
/// The layer's one local user and the title's service config id, as every VIctory-based build names them.
const USER: &str = "0009000000000001";
const SCID: &str = "00000000-0000-0000-0000-00007952987a";
const MANIFEST: &str = "manifest";
/// Automatic backups kept (the oldest go first).
const BACKUPS_KEPT: usize = 10;

/// One save slot's newest version.
#[derive(Clone, Debug)]
pub struct Slot {
    pub name: String,
    pub display: String,
    /// (blob name, bytes)
    pub blobs: Vec<(String, Vec<u8>)>,
    pub modified: Option<std::time::SystemTime>,
}

impl Slot {
    pub fn bytes(&self) -> usize {
        self.blobs.iter().map(|(_, b)| b.len()).sum()
    }

    /// The story saves (`SARC...`) are what "a save" means to a player; `Player` and the profile settings travel
    /// only when asked for.
    pub fn is_story(&self) -> bool {
        self.name.starts_with("SARC")
    }

    pub fn label(&self) -> String {
        let when = self
            .modified
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| format_time(d.as_secs()))
            .unwrap_or_default();
        let kind = match self.name.as_str() {
            n if n.starts_with("SARC") => format!("story save {}", n.trim_start_matches("SARC")),
            "Player" => "player profile".to_owned(),
            "SettingsProfileSettings" => "game settings".to_owned(),
            n => n.to_owned(),
        };
        format!("{kind} · {when} · {:.0} KB", self.bytes() as f64 / 1024.0)
    }
}

/// `<data>\7952987A\saves\<user>\<scid>`: where this launcher's slots are.
pub fn slots_dir(data: &Path) -> PathBuf {
    data.join(TITLE).join("saves").join(USER).join(SCID)
}

/// The folder name of a slot: its name made safe (letters, digits, `-`, `_`; others `%XX`), at most 100, then `~`
/// and the 64-bit FNV-1a of the exact name (the layer's `xgamesave::container_dir`).
fn container_dir_name(name: &str) -> String {
    let hash = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
    let mut readable: String = name
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    readable.truncate(100);
    format!("{readable}~{hash:016x}")
}

fn manifest(name: &str, display: &str, blobs: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in [name, display].into_iter().chain(blobs.iter().map(String::as_str)) {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    out
}

fn parse_manifest(bytes: &[u8]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let (len, tail) = rest.split_first_chunk::<4>()?;
        let len = u32::from_le_bytes(*len) as usize;
        out.push(String::from_utf8(tail.get(..len)?.to_vec()).ok()?);
        rest = &tail[len..];
    }
    (out.len() >= 2).then_some(out)
}

/// `v3` -> 3; anything else (as the layer: `v0`, `v01` are not versions) -> None.
fn version_number(file: &str) -> Option<u64> {
    let n = file.strip_prefix('v')?.parse::<u64>().ok()?;
    (n > 0 && format!("v{n}") == file).then_some(n)
}

/// A slot folder's newest readable version, and the highest version number in it (readable or not).
fn newest(container: &Path) -> (Option<Slot>, u64) {
    let mut versions: Vec<(u64, PathBuf)> = std::fs::read_dir(container)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((version_number(&e.file_name().to_string_lossy())?, e.path())))
        .collect();
    versions.sort();
    let highest = versions.last().map_or(0, |v| v.0);
    for (_, dir) in versions.iter().rev() {
        let Some(names) = std::fs::read(dir.join(MANIFEST)).ok().and_then(|b| parse_manifest(&b)) else { continue };
        let blobs: Option<Vec<(String, Vec<u8>)>> =
            names[2..].iter().enumerate().map(|(i, n)| std::fs::read(dir.join(i.to_string())).ok().map(|b| (n.clone(), b))).collect();
        let Some(blobs) = blobs else { continue };
        let modified = std::fs::metadata(dir.join(MANIFEST)).and_then(|m| m.modified()).ok();
        return (Some(Slot { name: names[0].clone(), display: names[1].clone(), blobs, modified }), highest);
    }
    (None, highest)
}

/// The slots under a VIctory-layout folder: `data` may be a data folder, its title folder, or a slots folder itself.
fn victory_slots(root: &Path) -> Vec<Slot> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = e.path();
            if !path.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.contains('~') {
                if let (Some(slot), _) = newest(&path) {
                    out.push(slot);
                }
            } else if depth < 6 {
                stack.push((path, depth + 1));
            }
        }
    }
    out
}

/// XCL's game saves: `<...>\gamesave\<scid>\<name>\b\<blob name>` and `<name>\display`.
fn xcl_slots(root: &Path) -> Vec<Slot> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = e.path();
            if !path.is_dir() {
                continue;
            }
            let blobs_dir = path.join("b");
            if blobs_dir.is_dir() {
                let name = e.file_name().to_string_lossy().into_owned();
                let display = std::fs::read_to_string(path.join("display")).unwrap_or_else(|_| name.clone());
                // The layer's order: data first, then header (as the title writes them); others after, by name.
                let mut blobs: Vec<(String, Vec<u8>)> = std::fs::read_dir(&blobs_dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|b| Some((b.file_name().to_string_lossy().into_owned(), std::fs::read(b.path()).ok()?)))
                    .collect();
                let rank = |n: &str| match n {
                    "Save Game Data" => 0,
                    "Save Game Header" => 1,
                    _ => 2,
                };
                blobs.sort_by(|a, b| (rank(&a.0), &a.0).cmp(&(rank(&b.0), &b.0)));
                let modified = std::fs::metadata(&blobs_dir).and_then(|m| m.modified()).ok();
                if !blobs.is_empty() {
                    out.push(Slot { name, display: display.trim().to_owned(), blobs, modified });
                }
            } else if depth < 6 {
                stack.push((path, depth + 1));
            }
        }
    }
    out
}

/// A zip this launcher exported (the slots folder's tree inside).
fn zip_slots(file: &Path) -> Result<Vec<Slot>, String> {
    let tmp = std::env::temp_dir().join(format!("vsp-save-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut zip = zip::ZipArchive::new(std::fs::File::open(file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else { continue };
        let out = tmp.join(rel);
        if f.is_dir() {
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut data = Vec::new();
        f.read_to_end(&mut data).map_err(|e| e.to_string())?;
        std::fs::write(&out, data).map_err(|e| e.to_string())?;
    }
    let slots = victory_slots(&tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(slots)
}

/// Whatever `path` is: a zip, an XCL game-save folder, or a VIctory-layout folder.
pub fn read_source(path: &Path) -> Result<Vec<Slot>, String> {
    let mut slots = if path.is_file() {
        zip_slots(path)?
    } else {
        let mut v = victory_slots(path);
        if v.is_empty() {
            v = xcl_slots(path);
        }
        v
    };
    slots.sort_by(|a, b| (!a.is_story(), &a.name).cmp(&(!b.is_story(), &b.name)));
    if slots.is_empty() {
        return Err(format!("no saves found in {}", path.display()));
    }
    Ok(slots)
}

/// Save folders of other builds on this PC: the VIctory forks' default data folder and XCL's.
pub fn known_sources(ours: &Path) -> Vec<(String, PathBuf)> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
    let mut out = Vec::new();
    let victory = local.join("victory").join("data");
    if victory.is_dir() && victory != ours && !victory_slots(&victory).is_empty() {
        out.push(("Other VIctory builds (VIctory-NVIDIA, its forks; %LOCALAPPDATA%\\victory)".to_owned(), victory));
    }
    for e in std::fs::read_dir(local.join("XCL")).into_iter().flatten().flatten() {
        let gs = e.path().join("gamesave");
        if gs.is_dir() && !xcl_slots(&gs).is_empty() {
            out.push(("XCL".to_owned(), gs));
        }
    }
    out
}

/// This launcher's slots.
pub fn ours(data: &Path) -> Vec<Slot> {
    let mut v = victory_slots(&slots_dir(data));
    v.sort_by(|a, b| (!a.is_story(), &a.name).cmp(&(!b.is_story(), &b.name)));
    v
}

/// Writes `slot` as the next version of its slot under `data` (what was there stays, one version down).
pub fn import(data: &Path, slot: &Slot) -> Result<(), String> {
    let container = slots_dir(data).join(container_dir_name(&slot.name));
    let (_, highest) = newest(&container);
    let dir = container.join(format!("v{}", highest + 1));
    let tmp = container.join(format!("importing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    for (i, (_, bytes)) in slot.blobs.iter().enumerate() {
        std::fs::write(tmp.join(i.to_string()), bytes).map_err(|e| e.to_string())?;
    }
    let names: Vec<String> = slot.blobs.iter().map(|(n, _)| n.clone()).collect();
    // The manifest last: a version without one is not read.
    std::fs::write(tmp.join(MANIFEST), manifest(&slot.name, &slot.display, &names)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &dir).map_err(|e| e.to_string())
}

/// The slots folder's whole tree into a zip.
pub fn export(data: &Path, to: &Path) -> Result<usize, String> {
    let root = data.join(TITLE).join("saves");
    if !root.is_dir() {
        return Err("there are no saves yet".into());
    }
    let file = std::fs::File::create(to).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut files = 0;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path.strip_prefix(data).map_err(|e| e.to_string())?;
            let name = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
            zip.start_file(name, opts).map_err(|e| e.to_string())?;
            zip.write_all(&std::fs::read(&path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            files += 1;
        }
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(files)
}

/// `%LOCALAPPDATA%\victory-scarlett-play\save-backups`.
pub fn backups_dir() -> PathBuf {
    crate::config::home().join("save-backups")
}

/// A backup of every save (before each Play); the newest [`BACKUPS_KEPT`] are kept. Nothing to back up: Ok(None).
pub fn backup(data: &Path) -> Result<Option<PathBuf>, String> {
    if ours(data).is_empty() {
        return Ok(None);
    }
    let dir = backups_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let file = dir.join(format!("saves-{}.zip", format_time(now).replace([' ', ':'], "-")));
    export(data, &file)?;
    let mut old: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "zip") && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("saves-")))
        .collect();
    old.sort();
    for gone in old.iter().rev().skip(BACKUPS_KEPT) {
        let _ = std::fs::remove_file(gone);
    }
    Ok(Some(file))
}

/// `YYYY-MM-DD HH:MM` (UTC) of seconds since 1970.
fn format_time(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", rest / 3600, rest % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vsp-saves-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn folder_names_match_the_layer() {
        // As the layer names them (seen on disk).
        assert_eq!(container_dir_name("Player"), "Player~333dc56ddffd8ea0");
        assert_eq!(container_dir_name("SARC0015"), "SARC0015~f3ad35e1a05a7da0");
        assert_eq!(container_dir_name("SettingsProfileSettings"), "SettingsProfileSettings~0b4e25fca06268d0");
    }

    #[test]
    fn import_adds_a_version_and_keeps_the_old_one() {
        let data = tmp("import");
        let slot = |b: u8| Slot {
            name: "SARC0015".into(),
            display: "SARC0015".into(),
            blobs: vec![("Save Game Data".into(), vec![b; 10]), ("Save Game Header".into(), vec![b; 3])],
            modified: None,
        };
        import(&data, &slot(1)).unwrap();
        import(&data, &slot(2)).unwrap();
        let c = slots_dir(&data).join("SARC0015~f3ad35e1a05a7da0");
        assert!(c.join("v1").join("manifest").is_file() && c.join("v2").join("manifest").is_file());
        let got = ours(&data);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].blobs[0].1, vec![2; 10]);
        assert_eq!(got[0].blobs[1].0, "Save Game Header");
        // Export, read back from the zip.
        let z = data.join("out.zip");
        assert_eq!(export(&data, &z).unwrap(), 6);
        let back = read_source(&z).unwrap();
        assert_eq!(back[0].blobs[0].1, vec![2; 10]);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn xcl_saves_are_read_in_the_layers_blob_order() {
        let root = tmp("xcl");
        let slot = root.join(SCID).join("SARC0001");
        std::fs::create_dir_all(slot.join("b")).unwrap();
        std::fs::write(slot.join("b").join("Save Game Header"), [1u8; 4]).unwrap();
        std::fs::write(slot.join("b").join("Save Game Data"), [2u8; 8]).unwrap();
        std::fs::write(slot.join("display"), "SARC0001").unwrap();
        let s = read_source(&root).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].blobs[0].0.as_str(), s[0].blobs[1].0.as_str()), ("Save Game Data", "Save Game Header"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
