//! Thin wrapper over the Performance Data Helper (PDH) API.
//!
//! Counter paths are added with their English names, so they work on any
//! Windows display language.

use std::ptr::{null, null_mut};

use windows_sys::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW, PdhRemoveCounter,
};

use crate::sys::{from_wide_ptr, wide};

/// Do not clamp values to 100 (CPU utility can go above 100 with turbo).
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
const FMT: u32 = PDH_FMT_DOUBLE | PDH_FMT_NOCAP100;

pub struct Query(PDH_HQUERY);

pub struct Counter(PDH_HCOUNTER);

fn status_ok(s: u32) -> bool {
    s == PDH_CSTATUS_VALID_DATA || s == PDH_CSTATUS_NEW_DATA
}

impl Query {
    pub fn new() -> Option<Query> {
        let mut q: PDH_HQUERY = null_mut();
        let rc = unsafe { PdhOpenQueryW(null(), 0, &mut q) };
        (rc == 0).then_some(Query(q))
    }

    pub fn add(&self, path: &str) -> Option<Counter> {
        let p = wide(path);
        let mut c: PDH_HCOUNTER = null_mut();
        let rc = unsafe { PdhAddEnglishCounterW(self.0, p.as_ptr(), 0, &mut c) };
        (rc == 0).then_some(Counter(c))
    }

    pub fn remove(&self, c: Counter) {
        unsafe { PdhRemoveCounter(c.0) };
    }

    pub fn collect(&self) -> bool {
        unsafe { PdhCollectQueryData(self.0) == 0 }
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.0) };
    }
}

// The handles are plain identifiers owned by one sampler thread.
unsafe impl Send for Query {}
unsafe impl Send for Counter {}

impl Counter {
    /// The formatted value, or `None` until two samples have been collected.
    pub fn value(&self) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let rc = unsafe { PdhGetFormattedCounterValue(self.0, FMT, null_mut(), &mut v) };
        if rc != 0 || !status_ok(v.CStatus) {
            return None;
        }
        let d = unsafe { v.Anonymous.doubleValue };
        d.is_finite().then_some(d)
    }

    /// Every instance of a wildcard counter as (instance name, value).
    pub fn array(&self) -> Vec<(String, f64)> {
        for _ in 0..3 {
            let mut size = 0u32;
            let mut count = 0u32;
            let rc = unsafe {
                PdhGetFormattedCounterArrayW(self.0, FMT, &mut size, &mut count, null_mut())
            };
            if rc != PDH_MORE_DATA || size == 0 {
                return Vec::new();
            }
            // u64 storage keeps the items (pointers + f64) aligned.
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            let rc = unsafe { PdhGetFormattedCounterArrayW(self.0, FMT, &mut size, &mut count, items) };
            if rc == PDH_MORE_DATA {
                continue; // an instance appeared between the two calls
            }
            if rc != 0 {
                return Vec::new();
            }
            let items = unsafe { std::slice::from_raw_parts(items, count as usize) };
            return items
                .iter()
                .filter(|it| status_ok(it.FmtValue.CStatus))
                .filter_map(|it| {
                    let v = unsafe { it.FmtValue.Anonymous.doubleValue };
                    v.is_finite().then(|| (unsafe { from_wide_ptr(it.szName) }, v))
                })
                .collect();
        }
        Vec::new()
    }
}
