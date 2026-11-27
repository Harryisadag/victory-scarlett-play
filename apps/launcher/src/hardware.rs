//! What this PC has: CPU, memory, GPU, driver, Windows; and whether it can run the game at all.

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Hardware {
    pub cpu: String,
    pub threads: usize,
    pub avx2: bool,
    pub ram_gb: f64,
    pub free_ram_gb: f64,
    /// RAM + page file: what Windows lets all programs commit together.
    pub commit_limit_gb: f64,
    pub gpu: String,
    pub vendor: Vendor,
    pub vram_gb: f64,
    pub driver: String,
    pub shader_model_66: bool,
    pub mesh_shaders: bool,
    pub windows: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    #[default]
    Other,
}

/// A problem found, and whether it stops the game from running.
pub struct Finding {
    pub blocking: bool,
    pub text: String,
}

impl Hardware {
    pub fn detect() -> Self {
        let mut h = Hardware {
            cpu: reg_string(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")
                .map(|s| s.trim().to_owned())
                .unwrap_or_else(|| "unknown CPU".into()),
            threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
            avx2: std::arch::is_x86_feature_detected!("avx2"),
            windows: windows_version(),
            ..Default::default()
        };
        memory(&mut h);
        gpu(&mut h);
        h
    }

    pub fn findings(&self) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut add = |blocking: bool, text: String| out.push(Finding { blocking, text });
        if !self.avx2 {
            add(true, format!("{} has no AVX2. The game layer needs a CPU with AVX2 (Intel 4th gen / AMD Ryzen or newer).", self.cpu));
        }
        if !self.shader_model_66 || !self.mesh_shaders {
            add(
                true,
                format!(
                    "{} lacks DirectX 12 {} - a GPU with shader model 6.6 and mesh shaders is needed (NVIDIA RTX 20 or newer, AMD RX 6000 or newer). Updating the graphics driver can help.",
                    self.gpu,
                    match (self.shader_model_66, self.mesh_shaders) {
                        (false, false) => "shader model 6.6 and mesh shaders",
                        (false, true) => "shader model 6.6",
                        _ => "mesh shaders",
                    }
                ),
            );
        }
        if self.ram_gb < 30.0 {
            add(
                self.commit_limit_gb < 40.0,
                format!(
                    "{:.0} GB of RAM: the game and the layer commit about 31-34 GB. {}",
                    self.ram_gb,
                    if self.commit_limit_gb < 40.0 {
                        "Make the Windows page file larger (System > About > Advanced system settings > Performance > Advanced > Virtual memory: 'System managed' on a drive with 40 GB free)."
                    } else {
                        "It will run from the page file: expect freezes."
                    }
                ),
            );
        } else if self.commit_limit_gb < 48.0 {
            add(
                false,
                format!(
                    "RAM + page file is {:.0} GB. Freezes at scene loads get much worse when it runs short; a 'System managed' page file avoids that.",
                    self.commit_limit_gb
                ),
            );
        }
        if self.vram_gb > 0.0 && self.vram_gb < 7.5 {
            add(false, format!("{:.0} GB of VRAM: untested; Series S mode is used.", self.vram_gb));
        }
        if self.free_ram_gb < 8.0 && self.ram_gb >= 30.0 {
            add(false, format!("Only {:.1} GB of RAM is free right now: close other programs (browsers especially) before playing.", self.free_ram_gb));
        }
        out
    }
}

fn memory(h: &mut Hardware) {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: plain data; the size is set before the call.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a valid out pointer.
    if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
        let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
        h.ram_gb = gb(m.ullTotalPhys);
        h.free_ram_gb = gb(m.ullAvailPhys);
        h.commit_limit_gb = gb(m.ullTotalPageFile);
    }
}

fn gpu(h: &mut Hardware) {
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::core::Interface;

    // SAFETY: plain COM calls on objects created here; out structures of the right size.
    unsafe {
        let Ok(factory) = CreateDXGIFactory2::<IDXGIFactory6>(DXGI_CREATE_FACTORY_FLAGS(0)) else {
            return;
        };
        let Ok(adapter) = factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(0, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) else {
            return;
        };
        if let Ok(desc) = adapter.GetDesc1() {
            let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            h.gpu = String::from_utf16_lossy(&desc.Description[..len]).trim().to_owned();
            h.vendor = match desc.VendorId {
                0x10de => Vendor::Nvidia,
                0x1002 | 0x1022 => Vendor::Amd,
                0x8086 => Vendor::Intel,
                _ => Vendor::Other,
            };
            h.vram_gb = desc.DedicatedVideoMemory as f64 / (1u64 << 30) as f64;
        }
        if let Ok(v) = adapter.CheckInterfaceSupport(&IDXGIDevice::IID) {
            let v = v as u64;
            let (a, b, c, d) = (v >> 48, (v >> 32) & 0xffff, (v >> 16) & 0xffff, v & 0xffff);
            h.driver = if h.vendor == Vendor::Nvidia {
                // NVIDIA's own numbering: the last five digits of the Windows version, as 581.57.
                let n = (c % 10) * 10000 + d;
                format!("{}.{:02}", n / 100, n % 100)
            } else {
                format!("{a}.{b}.{c}.{d}")
            };
        }
        let mut device: Option<ID3D12Device> = None;
        if D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device).is_err() {
            return;
        }
        let Some(device) = device else { return };
        let mut sm = D3D12_FEATURE_DATA_SHADER_MODEL { HighestShaderModel: D3D_SHADER_MODEL_6_6 };
        if device
            .CheckFeatureSupport(D3D12_FEATURE_SHADER_MODEL, (&raw mut sm).cast(), size_of_val(&sm) as u32)
            .is_ok()
        {
            h.shader_model_66 = sm.HighestShaderModel.0 >= D3D_SHADER_MODEL_6_6.0;
        }
        let mut o7 = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
        if device
            .CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS7, (&raw mut o7).cast(), size_of_val(&o7) as u32)
            .is_ok()
        {
            h.mesh_shaders = o7.MeshShaderTier.0 >= D3D12_MESH_SHADER_TIER_1.0;
        }
    }
}

fn windows_version() -> String {
    let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = reg_string(key, "CurrentBuild").unwrap_or_default();
    // Windows 11 still says "Windows 10" in ProductName; the build tells.
    let name = if build.parse::<u32>().unwrap_or(0) >= 22000 { "Windows 11" } else { "Windows 10" };
    let display = reg_string(key, "DisplayVersion").unwrap_or_default();
    format!("{name} {display} (build {build})").trim().to_owned()
}

/// A string value under HKEY_LOCAL_MACHINE (read only).
fn reg_string(key: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
    let value: Vec<u16> = value.encode_utf16().chain([0]).collect();
    let mut buf = [0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: NUL-terminated names; the buffer and its size in bytes.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if r != 0 {
        return None;
    }
    let n = (len as usize / 2).min(buf.len());
    let end = buf[..n].iter().position(|&c| c == 0).unwrap_or(n);
    Some(String::from_utf16_lossy(&buf[..end]))
}
