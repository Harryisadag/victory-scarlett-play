//! Running victory.exe: the game, the shader translation and the pipeline warm-up; reading its log; screenshots.

use crate::config::{self, Config};
use crate::hardware::Hardware;
use crate::presets;
use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// No console window for victory.exe's own console (its output goes to our log).
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// victory.exe: beside the launcher (release zip), or a build in the repository (development).
pub fn victory_exe() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let dir = me.parent()?;
    let mut candidates = vec![dir.join("victory").join("victory.exe"), dir.join("victory.exe")];
    for up in dir.ancestors().take(5) {
        candidates.push(up.join("target").join("diag").join("victory.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Identifies a victory.exe build (size and time written), for knowing when caches must be made again.
pub fn build_stamp(exe: &Path) -> String {
    match std::fs::metadata(exe) {
        Ok(m) => format!(
            "{}-{}",
            m.len(),
            m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
        ),
        Err(_) => "missing".into(),
    }
}

pub fn game_exe(game_dir: &Path) -> PathBuf {
    game_dir.join("game_scarlett_master.exe")
}

/// The game folder is usable: it has the executable and the shader archive.
pub fn check_game_dir(dir: &Path) -> Result<(), String> {
    if !game_exe(dir).is_file() {
        return Err("No game_scarlett_master.exe in this folder. Pick the folder of your own dump that holds it.".into());
    }
    if !dir.join("shaders.rpf").is_file() {
        return Err("This folder has the game executable but no shaders.rpf: the dump looks incomplete.".into());
    }
    Ok(())
}

/// A victory.exe command with the run's settings in its environment (any VICTORY_* the launcher itself was started
/// with is dropped first: only these count).
pub fn command(exe: &Path, cfg: &Config, hw: &Hardware, data_dir: &Path) -> (Command, &'static str) {
    let (env, console) = presets::environment(cfg, hw);
    let mut cmd = Command::new(exe);
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("VICTORY_") {
            cmd.env_remove(&k);
        }
    }
    cmd.envs(env);
    if let Some(game) = &cfg.game_dir {
        cmd.env("VICTORY_GAME_DIR", game);
    }
    cmd.env("VICTORY_CACHE_DIR", config::cache_dir());
    cmd.env("VICTORY_DATA_DIR", data_dir);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    (cmd, console)
}

/// One frame-rate line of the log.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub fps: f64,
    pub draws: f64,
}

/// What is known about a running (or ended) game.
#[derive(Default)]
pub struct Watch {
    pub samples: Vec<Sample>,
    pub tail: std::collections::VecDeque<String>,
    pub lost_device: bool,
    pub panicked: bool,
    pub peak_private_gb: f64,
    pub shots: Vec<Vec<u8>>,
    /// A preload's own progress lines (its steps' counts), kept apart from `tail`: its last step logs thousands of
    /// other lines between two of them.
    pub progress: std::collections::VecDeque<String>,
}

impl Watch {
    pub fn summary(&self) -> (f64, f64, f64) {
        let world: Vec<&Sample> = self.samples.iter().filter(|s| s.draws > 500.0).collect();
        if world.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        let avg = world.iter().map(|s| s.fps).sum::<f64>() / world.len() as f64;
        let mut sorted: Vec<f64> = world.iter().map(|s| s.fps).collect();
        sorted.sort_by(f64::total_cmp);
        let low = sorted[(sorted.len() / 100).min(sorted.len() - 1)];
        let heavy: Vec<f64> = world.iter().filter(|s| s.draws > 3000.0).map(|s| s.fps).collect();
        let heavy = if heavy.is_empty() { 0.0 } else { heavy.iter().sum::<f64>() / heavy.len() as f64 };
        (avg, low, heavy)
    }
}

/// `... victory_gpu::host: 15.7 fps, 63.5 ms/frame (...) 1105 draws/frame ...`
fn parse_sample(line: &str) -> Option<Sample> {
    let at = line.find("victory_gpu::host: ")? + "victory_gpu::host: ".len();
    let rest = &line[at..];
    let (fps, rest) = rest.split_once(" fps, ")?;
    let fps: f64 = fps.trim().parse().ok()?;
    let draws_end = rest.find(" draws/frame")?;
    let draws: f64 = rest[..draws_end].rsplit([' ', ',']).next()?.parse().ok()?;
    Some(Sample { fps, draws })
}

/// Copies a child's output into `log` (whole) and `watch` (samples, the last lines, failures).
pub fn follow(child: &mut Child, log: PathBuf, watch: Arc<Mutex<Watch>>, progress: Option<Arc<Mutex<String>>>) {
    let mut readers: Vec<Box<dyn Read + Send>> = Vec::new();
    if let Some(o) = child.stdout.take() {
        readers.push(Box::new(o));
    }
    if let Some(e) = child.stderr.take() {
        readers.push(Box::new(e));
    }
    let file = Arc::new(Mutex::new(std::fs::File::create(&log).ok()));
    for r in readers {
        let (file, watch, progress) = (file.clone(), watch.clone(), progress.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(r).lines().map_while(Result::ok) {
                let line = strip_ansi(&line);
                if let Some(f) = file.lock().unwrap().as_mut() {
                    use std::io::Write;
                    let _ = writeln!(f, "{line}");
                }
                if let Some(p) = &progress
                    && (line.contains("shaders in the cache") || line.contains("pipelines") && line.contains("made"))
                {
                    *p.lock().unwrap() = line.trim().chars().take(160).collect();
                }
                let mut w = watch.lock().unwrap();
                if let Some(s) = parse_sample(&line) {
                    w.samples.push(s);
                }
                if line.contains("0x887A0005") || line.contains("DEVICE_REMOVED") || line.contains("device removed") {
                    w.lost_device = true;
                }
                if line.contains("panic on thread") || line.contains("host GPU failure") {
                    w.panicked = true;
                }
                if crate::jobs::is_preload_progress(&line) {
                    w.progress.push_back(line.clone());
                    if w.progress.len() > 200 {
                        w.progress.pop_front();
                    }
                }
                w.tail.push_back(line);
                if w.tail.len() > 400 {
                    w.tail.pop_front();
                }
            }
        });
    }
}

/// The line without terminal colour codes (`ESC [ ... m`), which a layer may write even into a pipe.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn spawn_logged(mut cmd: Command, log: PathBuf, watch: Arc<Mutex<Watch>>, progress: Option<Arc<Mutex<String>>>) -> std::io::Result<Child> {
    let _ = std::fs::create_dir_all(config::logs_dir());
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd.spawn()?;
    follow(&mut child, log, watch, progress);
    Ok(child)
}

/// The process's private memory now, in GB.
pub fn private_gb(child: &Child) -> f64 {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    // SAFETY: plain data; a live process handle; the size given.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if GetProcessMemoryInfo(child.as_raw_handle() as _, (&raw mut c).cast(), c.cb) != 0 {
            return c.PrivateUsage as f64 / (1u64 << 30) as f64;
        }
    }
    0.0
}

/// A JPEG of the game's window (by process id), at most 1280 wide; `None` if it has none on screen.
pub fn screenshot(pid: u32) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClientRect, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    };

    struct Find {
        pid: u32,
        hwnd: HWND,
        area: i64,
    }
    unsafe extern "system" fn each(hwnd: HWND, l: LPARAM) -> i32 {
        // SAFETY: `l` is the `Find` below, alive for the enumeration.
        let f = unsafe { &mut *(l as *mut Find) };
        let mut pid = 0;
        // SAFETY: a window handle from the enumeration; an out pointer.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        // SAFETY: as above.
        if pid == f.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
            let mut r: RECT = unsafe { std::mem::zeroed() };
            // SAFETY: as above.
            unsafe { GetWindowRect(hwnd, &mut r) };
            let area = i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
            if area > f.area {
                f.hwnd = hwnd;
                f.area = area;
            }
        }
        1
    }
    let mut f = Find { pid, hwnd: std::ptr::null_mut(), area: 0 };
    // SAFETY: the callback reads `f` only during the call.
    unsafe { EnumWindows(Some(each), (&raw mut f) as LPARAM) };
    if f.hwnd.is_null() || f.area < 200 * 200 {
        return None;
    }
    // Privacy: the copy is of the screen, so only while the game's window is the one in front and not minimised
    // (otherwise another program's window - a browser, a chat - could be in the picture). Only its client area: no
    // title bar.
    // SAFETY: plain window queries on a handle from the enumeration.
    unsafe {
        if GetForegroundWindow() != f.hwnd || IsIconic(f.hwnd) != 0 {
            return None;
        }
    }
    // SAFETY: GDI calls on a screen DC and objects created and released here.
    let (w, h, pixels) = unsafe {
        let mut c: RECT = std::mem::zeroed();
        GetClientRect(f.hwnd, &mut c);
        let mut origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        ClientToScreen(f.hwnd, &mut origin);
        let r = RECT { left: origin.x, top: origin.y, right: origin.x + c.right, bottom: origin.y + c.bottom };
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w < 200 || h < 200 {
            return None;
        }
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp);
        BitBlt(mem, 0, 0, w, h, screen, r.left, r.top, SRCCOPY);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        let mut px = vec![0u8; (w * h * 4) as usize];
        GetDIBits(mem, bmp, 0, h as u32, px.as_mut_ptr().cast(), &mut info, DIB_RGB_COLORS);
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        (w as u32, h as u32, px)
    };
    let mut rgb = image::RgbImage::new(w, h);
    for (i, p) in rgb.pixels_mut().enumerate() {
        let b = &pixels[i * 4..i * 4 + 3];
        *p = image::Rgb([b[2], b[1], b[0]]);
    }
    let scaled = if w > 1280 {
        image::imageops::resize(&rgb, 1280, h * 1280 / w, image::imageops::FilterType::Triangle)
    } else {
        rgb
    };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 70).encode_image(&scaled).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_frame_line() {
        let line = "2026-11-27T01:12:09.410816Z  INFO victory_gpu::host: 9.9 fps, 101.5 ms/frame (0.5 preparing: copies 0.1 ms 0.0 MB, textures 3.4 ms 3.3 MB; 0.0 on the GPU; 0.0 copying back 0.0 MB), 3647 draws/frame, dropped 0 without a host pipeline (397 being made) and 0 indirect";
        let s = parse_sample(line).unwrap();
        assert_eq!(s.fps, 9.9);
        assert_eq!(s.draws, 3647.0);
        assert!(parse_sample("INFO victory_gpu::host: preparing per frame, the submitting thread: 0.0 ms").is_none());
    }

    #[test]
    fn colour_codes_do_not_hide_the_frame_line() {
        let line = "\u{1b}[2m2026-11-27T16:37:31Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m \u{1b}[2mvictory_gpu::host\u{1b}[0m\u{1b}[2m:\u{1b}[0m 21.3 fps, 47.0 ms/frame (38.0 preparing), 3900 draws/frame";
        let s = parse_sample(&strip_ansi(line)).unwrap();
        assert_eq!((s.fps, s.draws), (21.3, 3900.0));
    }

    #[test]
    fn summary_counts_world_frames() {
        let w = Watch {
            samples: vec![Sample { fps: 30.0, draws: 10.0 }, Sample { fps: 20.0, draws: 1000.0 }, Sample { fps: 10.0, draws: 4000.0 }],
            ..Default::default()
        };
        let (avg, low, heavy) = w.summary();
        assert_eq!((avg, low, heavy), (15.0, 10.0, 10.0));
    }
}

/// A running process whose executable is `exe` (other than `except`): the layer started again by itself (F1's
/// "save and restart", a preload's restart). Its id.
pub fn find_process(exe: &Path, except: u32) -> Option<u32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::ProcessStatus::EnumProcesses;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    let want = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let mut ids = vec![0u32; 4096];
    let mut got = 0u32;
    // SAFETY: a buffer and its size in bytes; `got` receives the bytes written.
    if unsafe { EnumProcesses(ids.as_mut_ptr(), (ids.len() * 4) as u32, &mut got) } == 0 {
        return None;
    }
    ids.truncate(got as usize / 4);
    ids.into_iter().filter(|&pid| pid != 0 && pid != except).find(|&pid| {
        // SAFETY: a query-only handle, closed below.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return false;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: the handle above; a buffer and its length in characters.
        let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0;
        // SAFETY: the handle above.
        unsafe { CloseHandle(h) };
        ok && {
            let path = PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]));
            std::fs::canonicalize(&path).unwrap_or(path) == want
        }
    })
}

/// Whether process `pid` is still running.
pub fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: a query-only handle, closed below; an out code.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE as u32
    }
}

/// Ends process `pid` and every process it started (Stop the game, for a game that restarted itself).
pub fn end(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

/// Ends `child` and every process it started (a preload's and a warm-up's pipeline workers, a restart from F1):
/// `Child::kill` alone ends only the first, and a stopped preload went on building in 9 processes of its own.
pub fn kill_tree(child: &mut Child) {
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    let _ = child.kill();
}
