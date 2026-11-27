//! Updates from the repository's GitHub releases: the newest release's zip, checked against its `.sha256`, replaces
//! the launcher's folder (the running Launcher.exe is renamed aside first; Windows lets a running exe be renamed).

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const REPO: &str = "Harryisadag/victory-scarlett-play";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub zip_url: String,
    pub sha_url: String,
    pub size: u64,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .user_agent(format!("victory-scarlett-play-launcher/{VERSION}"))
        .build()
        .into()
}

/// `1.2.10` > `1.2.9`; a leading `v` is ignored.
pub fn newer(theirs: &str, ours: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.trim_start_matches('v').split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect() };
    parse(theirs) > parse(ours)
}

/// The newest release, if it is newer than this launcher.
pub fn check() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = match agent().get(&url).header("Accept", "application/vnd.github+json").call() {
        Ok(r) => r,
        // No release published yet.
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let v: serde_json::Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or_default().to_owned();
    if tag.is_empty() || !newer(&tag, VERSION) {
        return Ok(None);
    }
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let find = |suffix: &str| {
        assets.iter().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(suffix))).map(|a| {
            (a["browser_download_url"].as_str().unwrap_or_default().to_owned(), a["size"].as_u64().unwrap_or(0))
        })
    };
    let (Some((zip_url, size)), Some((sha_url, _))) = (find(".zip"), find(".zip.sha256")) else {
        return Ok(None);
    };
    Ok(Some(Release {
        version: tag.trim_start_matches('v').to_owned(),
        notes: v["body"].as_str().unwrap_or_default().chars().take(2000).collect(),
        zip_url,
        sha_url,
        size,
    }))
}

/// Downloads and installs `r` over this launcher's folder; `progress` gets 0..1. Returns the new Launcher.exe.
pub fn install(r: &Release, progress: &dyn Fn(f32)) -> Result<PathBuf, String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = me.parent().ok_or("no launcher folder")?.to_path_buf();

    let want = {
        let mut resp = agent().get(&r.sha_url).call().map_err(|e| e.to_string())?;
        let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
        text.split_whitespace().next().unwrap_or_default().to_ascii_lowercase()
    };
    if want.len() != 64 {
        return Err("the release has no valid checksum".into());
    }

    let mut resp = agent().get(&r.zip_url).call().map_err(|e| e.to_string())?;
    let total = r.size.max(1);
    let mut body = resp.body_mut().with_config().limit(2 << 30).reader();
    let mut data = Vec::with_capacity(r.size as usize);
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = body.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        progress((data.len() as f64 / total as f64).min(1.0) as f32 * 0.9);
    }
    let got: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        return Err("the download does not match its checksum; nothing was changed".into());
    }

    // Unpack beside, then move into place.
    let staging = dir.join("update-staging");
    let _ = std::fs::remove_dir_all(&staging);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else { continue };
        // The zip holds one top folder; drop it.
        let rel: PathBuf = rel.components().skip(1).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = staging.join(&rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut f, &mut w).map_err(|e| e.to_string())?;
    }
    progress(0.95);
    let old = dir.join("Launcher.old.exe");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&me, &old).map_err(|e| format!("could not move the running launcher aside: {e}"))?;
    if let Err(e) = move_tree(&staging, &dir) {
        // Put the launcher back so the folder still starts.
        let _ = std::fs::rename(&old, &me);
        return Err(format!("installing the update failed: {e}"));
    }
    let _ = std::fs::remove_dir_all(&staging);
    progress(1.0);
    Ok(dir.join("Launcher.exe"))
}

fn move_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            std::fs::create_dir_all(&dest)?;
            move_tree(&e.path(), &dest)?;
        } else {
            let _ = std::fs::remove_file(&dest);
            std::fs::rename(e.path(), &dest)?;
        }
    }
    Ok(())
}

/// Removes what an update left: the old launcher.
pub fn tidy() {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let _ = std::fs::remove_file(dir.join("Launcher.old.exe"));
        let _ = std::fs::remove_dir_all(dir.join("update-staging"));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions_compare_by_number() {
        assert!(super::newer("v0.1.10", "0.1.9"));
        assert!(super::newer("1.0.0", "0.9.9"));
        assert!(!super::newer("v0.1.0", "0.1.0"));
        assert!(!super::newer("0.0.9", "0.1.0"));
    }
}
