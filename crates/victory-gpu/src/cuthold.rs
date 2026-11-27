//! (victory-scarlett-play, ported from its Scarlett build) `VICTORY_CUT_HOLD=1`, off by default: at a scene change, the last finished picture stays
//! on screen while the title's streaming catches up, instead of frames of a half-loaded scene (black ground, missing
//! buildings, low-detail textures) being shown.
//!
//! On the console the title's reads are decompressed by hardware and a new scene's data is there within a frame or
//! two of a camera cut. Here the storage workers decompress on the CPU, and right after a cut a burst of reads waits
//! for them. A burst is told from the steady streaming of play by its jump: the reads waiting go from under half the
//! threshold to over it between two frames. From then on frames are not presented (the window keeps the last one; the
//! title runs on unaware) while the reads waiting stay over a quarter of the threshold, for at most
//! `VICTORY_CUT_HOLD_MS` (1200). After a hold, none starts again for `VICTORY_CUT_HOLD_GAP_MS` (2000).
//! `VICTORY_CUT_HOLD_MIN` (48): reads waiting that make a burst.
//!
//! The same for pipelines: draws that ask for pipelines not compiled yet are dropped, so a frame shown then lacks
//! whole objects. A jump of those over `VICTORY_CUT_HOLD_PIPELINES` (16) starts a hold too, and it lasts while a
//! quarter of that many are still asked for. (A first measurement: the reads after a cut were done in 30-70 ms, but
//! a scene went on missing objects for seconds while its pipelines compiled.)
//!
//! `VICTORY_CUT_HOLD=wait`: the title itself waits instead. At such a burst its present call does not return until
//! the reads and the pipelines draws asked for are done (at most `VICTORY_CUT_HOLD_MS`, 3000 in this mode): the
//! title's frame loop, and with it the scene, stand still while the storage workers and the pipeline workers go on,
//! as behind a loading screen. The frame that was drawn while they were missing is not shown.
//!
//! The reads waiting come from the runtime's storage scheduler ([`on_stream_backlog`]); without it nothing is held.

use std::sync::OnceLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static BACKLOG: OnceLock<fn() -> u64> = OnceLock::new();

/// Registers what tells the storage reads submitted and not finished (the runtime's scheduler; once).
pub fn on_stream_backlog(f: fn() -> u64) {
    let _ = BACKLOG.set(f);
}

struct Settings {
    on: bool,
    wait: bool,
    min: u64,
    pipelines: u64,
    most: Duration,
    gap: Duration,
}

fn settings() -> &'static Settings {
    static S: OnceLock<Settings> = OnceLock::new();
    S.get_or_init(|| {
        let num = |name: &str, default: u64| std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default);
        let mode = std::env::var("VICTORY_CUT_HOLD").unwrap_or_default();
        let wait = mode.trim() == "wait";
        let s = Settings {
            on: wait || mode.trim() == "1",
            wait,
            min: num("VICTORY_CUT_HOLD_MIN", 48).max(4),
            pipelines: num("VICTORY_CUT_HOLD_PIPELINES", 16).max(4),
            most: Duration::from_millis(num("VICTORY_CUT_HOLD_MS", if wait { 3000 } else { 1200 }).min(10_000)),
            gap: Duration::from_millis(num("VICTORY_CUT_HOLD_GAP_MS", 2000)),
        };
        if s.on {
            tracing::info!(wait = s.wait, min = s.min, pipelines = s.pipelines, most_ms = s.most.as_millis() as u64, gap_ms = s.gap.as_millis() as u64, "scene-change hold (VICTORY_CUT_HOLD=1)");
        }
        s
    })
}

#[derive(Default)]
struct State {
    /// Reads waiting and pipelines asked for at the frame before.
    last: u64,
    last_asked: u64,
    /// The hold under way: when it began, frames held, reads waiting at its start.
    holding: Option<(Instant, u64, u64)>,
    /// When the last hold ended.
    ended: Option<Instant>,
    holds: u64,
    /// (wait mode) The frame after a wait is not shown: it was drawn while things were missing.
    skip_next: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// (`VICTORY_CUT_HOLD=wait`) Called at the start of the title's present of `frame`: at a burst of reads or of
/// pipelines asked for, returns only once they are done or the time is up, so the title waits.
pub(crate) fn wait(frame: u64) {
    let s = settings();
    if !s.on || !s.wait {
        return;
    }
    let Some(backlog) = BACKLOG.get() else {
        return;
    };
    let (waiting, asked) = (backlog(), crate::pipeline::asked_waiting());
    let began = Instant::now();
    {
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let st = guard.get_or_insert_with(State::default);
        let last = std::mem::replace(&mut st.last, waiting);
        let last_asked = std::mem::replace(&mut st.last_asked, asked);
        if frame < 120 || st.ended.is_some_and(|e| began.duration_since(e) < s.gap) {
            return;
        }
        let reads_burst = waiting >= s.min && last * 2 < s.min;
        let pipelines_burst = asked >= s.pipelines && last_asked * 2 < s.pipelines;
        if !reads_burst && !pipelines_burst {
            return;
        }
    }
    // Not under the lock: the storage and pipeline workers go on meanwhile.
    let (mut reads_now, mut pipelines_now) = (waiting, asked);
    while began.elapsed() < s.most && (reads_now * 4 >= s.min || pipelines_now > 0) {
        std::thread::sleep(Duration::from_millis(5));
        reads_now = backlog();
        pipelines_now = crate::pipeline::asked_waiting();
    }
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let st = guard.get_or_insert_with(State::default);
    st.ended = Some(Instant::now());
    st.holds += 1;
    st.skip_next = true;
    tracing::info!(
        frame,
        waited_ms = began.elapsed().as_millis() as u64,
        reads_at_start = waiting,
        pipelines_at_start = asked,
        reads_now,
        pipelines_now,
        cut_short = began.elapsed() >= s.most,
        holds = st.holds,
        "scene-change wait ended (the title waited)"
    );
}

/// Whether the present of `frame` is to be left out (the last picture stays).
pub(crate) fn hold(frame: u64) -> bool {
    let s = settings();
    if !s.on {
        return false;
    }
    if s.wait {
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let st = guard.get_or_insert_with(State::default);
        return std::mem::take(&mut st.skip_next);
    }
    let Some(backlog) = BACKLOG.get() else {
        return false;
    };
    let waiting = backlog();
    let asked = crate::pipeline::asked_waiting();
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let st = guard.get_or_insert_with(State::default);
    let last = std::mem::replace(&mut st.last, waiting);
    let last_asked = std::mem::replace(&mut st.last_asked, asked);
    let now = Instant::now();
    if let Some((began, frames, at_start)) = st.holding {
        let over = now.duration_since(began) >= s.most;
        if (waiting * 4 >= s.min || asked * 4 >= s.pipelines) && !over {
            st.holding = Some((began, frames + 1, at_start));
            return true;
        }
        st.holding = None;
        st.ended = Some(now);
        st.holds += 1;
        tracing::info!(
            frame,
            held_frames = frames,
            held_ms = now.duration_since(began).as_millis() as u64,
            reads_at_start = at_start,
            reads_now = waiting,
            pipelines_now = asked,
            cut_short = over,
            holds = st.holds,
            "scene-change hold ended"
        );
        return false;
    }
    // Frames before the title's own first scenes are not held (start screens, loading screens).
    if frame < 120 || st.ended.is_some_and(|e| now.duration_since(e) < s.gap) {
        return false;
    }
    let reads_burst = waiting >= s.min && last * 2 < s.min;
    let pipelines_burst = asked >= s.pipelines && last_asked * 2 < s.pipelines;
    if reads_burst || pipelines_burst {
        st.holding = Some((now, 1, waiting));
        return true;
    }
    false
}
