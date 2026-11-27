//! Muting one process's sound through Windows' per-app audio sessions (the mixer's per-app volume): the setup's
//! opening recording is silent. Windows remembers an app's mute, so whatever is muted here is unmuted again before
//! the process ends ([`set_mute`] with `false`), and Play unmutes the game once if a recording was cut off.

use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::core::Interface;

/// Mutes (or unmutes) every audio session of process `pid` on every active output device. Returns how many
/// sessions were found (0: it has opened no audio yet).
pub fn set_mute(pid: u32, mute: bool) -> usize {
    // SAFETY: COM calls on objects created here, on this thread; COM is initialised for it (an already
    // initialised thread is fine either way).
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(devices) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else {
            return 0;
        };
        let Ok(list) = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else {
            return 0;
        };
        let mut found = 0;
        for d in 0..list.GetCount().unwrap_or(0) {
            let Ok(device) = list.Item(d) else { continue };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let Ok(sessions) = manager.GetSessionEnumerator() else { continue };
            for s in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(session) = sessions.GetSession(s) else { continue };
                let Ok(control) = session.cast::<IAudioSessionControl2>() else { continue };
                if control.GetProcessId().ok() != Some(pid) {
                    continue;
                }
                if let Ok(volume) = session.cast::<ISimpleAudioVolume>() {
                    let _ = volume.SetMute(mute, std::ptr::null());
                    found += 1;
                }
            }
        }
        found
    }
}
