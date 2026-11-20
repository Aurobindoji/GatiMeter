//! Temperatures from user-mode Windows interfaces only. No kernel driver.
//!
//! * GPU: the adapter performance data that the display driver reports to
//!   Windows (`D3DKMTQueryAdapterInfo`, the source Task Manager uses).
//! * Disks: `IOCTL_STORAGE_QUERY_PROPERTY` with `StorageDeviceTemperatureProperty`.
//! * CPU: read in `sampler.rs` from the ACPI thermal-zone performance counter.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_PROPERTY_ID,
    STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY, STORAGE_TEMPERATURE_DATA_DESCRIPTOR,
    STORAGE_TEMPERATURE_INFO, StorageDeviceProperty, StorageDeviceTemperatureProperty,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

use crate::sys::wide;

// ------------------------------------------------------------------- GPU

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AdapterInfo {
    h_adapter: u32,
    luid_low: u32,
    luid_high: i32,
    num_sources: u32,
    precise_present_regions: i32,
}

#[repr(C)]
struct EnumAdapters2 {
    num_adapters: u32,
    adapters: *mut AdapterInfo,
}

#[repr(C)]
struct QueryAdapterInfo {
    h_adapter: u32,
    kind: u32,
    data: *mut c_void,
    size: u32,
}

#[repr(C)]
#[derive(Default)]
struct AdapterPerfData {
    physical_adapter_index: u32,
    memory_frequency: u64,
    max_memory_frequency: u64,
    max_memory_frequency_oc: u64,
    memory_bandwidth: u64,
    pcie_bandwidth: u64,
    fan_rpm: u32,
    power: u32,
    /// Tenths of a degree Celsius.
    temperature: u32,
    power_state_override: u8,
}

#[repr(C)]
struct CloseAdapter {
    h_adapter: u32,
}

/// KMTQAITYPE_ADAPTERPERFDATA
const QUERY_ADAPTER_PERF_DATA: u32 = 62;

type EnumFn = unsafe extern "system" fn(*mut EnumAdapters2) -> i32;
type QueryFn = unsafe extern "system" fn(*const QueryAdapterInfo) -> i32;
type CloseFn = unsafe extern "system" fn(*const CloseAdapter) -> i32;

pub struct GpuTemp {
    api: Option<(EnumFn, QueryFn, CloseFn)>,
}

impl GpuTemp {
    pub fn new() -> GpuTemp {
        let api = unsafe {
            let name = wide("gdi32.dll");
            let lib = LoadLibraryExW(name.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
            if lib.is_null() {
                None
            } else {
                let e = GetProcAddress(lib, c"D3DKMTEnumAdapters2".as_ptr() as *const u8);
                let q = GetProcAddress(lib, c"D3DKMTQueryAdapterInfo".as_ptr() as *const u8);
                let c = GetProcAddress(lib, c"D3DKMTCloseAdapter".as_ptr() as *const u8);
                match (e, q, c) {
                    (Some(e), Some(q), Some(c)) => Some((
                        std::mem::transmute::<unsafe extern "system" fn() -> isize, EnumFn>(e),
                        std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryFn>(q),
                        std::mem::transmute::<unsafe extern "system" fn() -> isize, CloseFn>(c),
                    )),
                    _ => None,
                }
            }
        };
        GpuTemp { api }
    }

    /// Hottest GPU in °C, if any display driver reports a temperature.
    pub fn read(&self) -> Option<f32> {
        let (enum_adapters, query, close) = self.api?;
        unsafe {
            let mut e = EnumAdapters2 { num_adapters: 0, adapters: null_mut() };
            if enum_adapters(&mut e) != 0 || e.num_adapters == 0 {
                return None;
            }
            let mut list = vec![AdapterInfo::default(); e.num_adapters as usize];
            e.adapters = list.as_mut_ptr();
            if enum_adapters(&mut e) != 0 {
                return None;
            }
            let mut best: Option<f32> = None;
            for a in list.iter().take(e.num_adapters as usize) {
                let mut perf = AdapterPerfData::default();
                let q = QueryAdapterInfo {
                    h_adapter: a.h_adapter,
                    kind: QUERY_ADAPTER_PERF_DATA,
                    data: &mut perf as *mut _ as *mut c_void,
                    size: size_of::<AdapterPerfData>() as u32,
                };
                if query(&q) == 0 && perf.temperature > 0 {
                    let t = perf.temperature as f32 / 10.0;
                    if (1.0..=150.0).contains(&t) {
                        best = Some(best.map_or(t, |b| b.max(t)));
                    }
                }
                close(&CloseAdapter { h_adapter: a.h_adapter });
            }
            best
        }
    }
}

// ------------------------------------------------------------------ disks

pub const MAX_DRIVES: u32 = 16;
const RESCAN: Duration = Duration::from_secs(60);

/// Remembers which `\\.\PhysicalDriveN` report a temperature (and their
/// names), rescanning now and then.
pub struct DiskTemp {
    drives: Vec<(u32, String)>,
    scanned: Option<Instant>,
}

impl DiskTemp {
    pub fn new() -> DiskTemp {
        DiskTemp { drives: Vec::new(), scanned: None }
    }

    /// Every disk that reports a temperature: (name, °C), in drive order.
    pub fn read(&mut self) -> Vec<(String, f32)> {
        if self.scanned.is_none_or(|t| t.elapsed() >= RESCAN) {
            let found: Vec<u32> = (0..MAX_DRIVES).filter(|&i| drive_temp(i).is_some()).collect();
            let models: Vec<(u32, String)> =
                found.iter().map(|&i| (i, drive_model(i).unwrap_or_default())).collect();
            self.drives = unique_names(&models);
            self.scanned = Some(Instant::now());
        }
        self.drives
            .iter()
            .filter_map(|(i, n)| drive_temp(*i).map(|t| (n.clone(), t)))
            .collect()
    }
}

/// The temperature to show: the named disk, or the hottest when `choice` is empty.
pub fn pick_disk_temp(all: &[(String, f32)], choice: &str) -> Option<f32> {
    if choice.is_empty() {
        all.iter().map(|(_, t)| *t).fold(None, |b: Option<f32>, t| Some(b.map_or(t, |b| b.max(t))))
    } else {
        all.iter().find(|(n, _)| n == choice).map(|(_, t)| *t)
    }
}

/// Model names that can tell the disks apart. Two disks of the same model
/// (or one without a name) get "(Disk N)" added.
fn unique_names(models: &[(u32, String)]) -> Vec<(u32, String)> {
    models
        .iter()
        .map(|(i, m)| {
            let twin = models.iter().filter(|(_, o)| o == m).count() > 1;
            let name = if m.is_empty() {
                format!("Disk {i}")
            } else if twin {
                format!("{m} (Disk {i})")
            } else {
                m.clone()
            };
            (*i, name)
        })
        .collect()
}

/// Product name of `\\.\PhysicalDrive{index}`, e.g. "CT1000T500SSD8".
fn drive_model(index: u32) -> Option<String> {
    let mut buf = [0u64; 128];
    let got = query_drive_property(index, StorageDeviceProperty, &mut buf)?;
    if got < size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, got) };
    let d = unsafe { &*(buf.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };
    let text = |off: u32| -> String {
        let off = off as usize;
        if off == 0 || off >= bytes.len() {
            return String::new();
        }
        let end = bytes[off..].iter().position(|&b| b == 0).map_or(bytes.len(), |p| off + p);
        String::from_utf8_lossy(&bytes[off..end]).trim().to_string()
    };
    let product = text(d.ProductIdOffset);
    let vendor = text(d.VendorIdOffset);
    let name = if product.is_empty() {
        vendor
    } else if vendor.is_empty() || product.starts_with(&vendor) {
        product
    } else {
        format!("{vendor} {product}")
    };
    (!name.is_empty()).then_some(name)
}

/// Runs `IOCTL_STORAGE_QUERY_PROPERTY` for `prop` on `\\.\PhysicalDrive{index}`
/// and returns how many bytes it wrote into `out`.
pub fn query_drive_property(index: u32, prop: STORAGE_PROPERTY_ID, out: &mut [u64]) -> Option<usize> {
    let path = wide(&format!("\\\\.\\PhysicalDrive{index}"));
    unsafe {
        // Access 0 is enough for property queries and needs no admin rights.
        let h = CreateFileW(
            path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut q: STORAGE_PROPERTY_QUERY = zeroed();
        q.PropertyId = prop;
        q.QueryType = PropertyStandardQuery;
        let mut got = 0u32;
        let ok = DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &q as *const _ as *const c_void,
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            out.as_mut_ptr() as *mut c_void,
            size_of_val(out) as u32,
            &mut got,
            null_mut(),
        );
        CloseHandle(h);
        (ok != 0).then_some(got as usize)
    }
}

fn drive_temp(index: u32) -> Option<f32> {
    let mut buf = [0u64; 128];
    let got = query_drive_property(index, StorageDeviceTemperatureProperty, &mut buf)?;
    if got < size_of::<STORAGE_TEMPERATURE_DATA_DESCRIPTOR>() {
        return None;
    }
    unsafe {
        let d = &*(buf.as_ptr() as *const STORAGE_TEMPERATURE_DATA_DESCRIPTOR);
        let count = d.InfoCount as usize;
        let max_fit = (got - std::mem::offset_of!(STORAGE_TEMPERATURE_DATA_DESCRIPTOR, TemperatureInfo))
            / size_of::<STORAGE_TEMPERATURE_INFO>();
        let infos = std::slice::from_raw_parts(d.TemperatureInfo.as_ptr(), count.min(max_fit));
        // Entry 0 is the device's overall (composite) temperature.
        infos
            .first()
            .map(|i| i.Temperature)
            .filter(|t| (1..=125).contains(t))
            .map(|t| t as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_names_and_choice() {
        let n = unique_names(&[(0, "T500".into()), (1, "ORICO".into()), (2, "ORICO".into()), (3, String::new())]);
        let names: Vec<&str> = n.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(names, ["T500", "ORICO (Disk 1)", "ORICO (Disk 2)", "Disk 3"]);
        let all = vec![("T500".to_string(), 39.0), ("ORICO".to_string(), 45.0)];
        assert_eq!(pick_disk_temp(&all, ""), Some(45.0));
        assert_eq!(pick_disk_temp(&all, "T500"), Some(39.0));
        assert_eq!(pick_disk_temp(&all, "gone"), None);
        assert_eq!(pick_disk_temp(&[], ""), None);
    }
}
