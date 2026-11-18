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
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_PROPERTY_QUERY,
    STORAGE_TEMPERATURE_DATA_DESCRIPTOR, STORAGE_TEMPERATURE_INFO,
    StorageDeviceTemperatureProperty,
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

const MAX_DRIVES: u32 = 16;
const RESCAN: Duration = Duration::from_secs(60);

/// Remembers which `\\.\PhysicalDriveN` report a temperature, rescanning now and then.
pub struct DiskTemp {
    drives: Vec<u32>,
    scanned: Option<Instant>,
}

impl DiskTemp {
    pub fn new() -> DiskTemp {
        DiskTemp { drives: Vec::new(), scanned: None }
    }

    /// Hottest disk in °C.
    pub fn read(&mut self) -> Option<f32> {
        if self.scanned.is_none_or(|t| t.elapsed() >= RESCAN) {
            self.drives = (0..MAX_DRIVES).filter(|&i| drive_temp(i).is_some()).collect();
            self.scanned = Some(Instant::now());
        }
        self.drives
            .iter()
            .filter_map(|&i| drive_temp(i))
            .fold(None, |best: Option<f32>, t| Some(best.map_or(t, |b| b.max(t))))
    }
}

fn drive_temp(index: u32) -> Option<f32> {
    let path = wide(&format!("\\\\.\\PhysicalDrive{index}"));
    unsafe {
        // Access 0 is enough for this query and needs no admin rights.
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
        q.PropertyId = StorageDeviceTemperatureProperty;
        q.QueryType = PropertyStandardQuery;
        let mut buf = [0u64; 128];
        let mut got = 0u32;
        let ok = DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &q as *const _ as *const c_void,
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            buf.as_mut_ptr() as *mut c_void,
            size_of_val(&buf) as u32,
            &mut got,
            null_mut(),
        );
        CloseHandle(h);
        if ok == 0 || (got as usize) < size_of::<STORAGE_TEMPERATURE_DATA_DESCRIPTOR>() {
            return None;
        }
        let d = &*(buf.as_ptr() as *const STORAGE_TEMPERATURE_DATA_DESCRIPTOR);
        let count = d.InfoCount as usize;
        let max_fit = (got as usize - std::mem::offset_of!(STORAGE_TEMPERATURE_DATA_DESCRIPTOR, TemperatureInfo))
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
