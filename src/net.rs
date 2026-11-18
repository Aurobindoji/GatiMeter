//! Network speed from the interface byte counters (`GetIfTable2`).

use std::ptr::null_mut;
use std::time::Instant;

use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};

use crate::sys::from_wide;

const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_OPER_STATUS_UP: i32 = 1;
// Bits of MIB_IF_ROW2.InterfaceAndOperStatusFlags.
const FLAG_HARDWARE: u8 = 0x01;
const FLAG_FILTER: u8 = 0x02;

#[derive(Clone, Debug, Default)]
pub struct NetReading {
    pub up_bps: f64,
    pub down_bps: f64,
    /// Bytes since the previous reading (for the daily totals).
    pub delta_up: u64,
    pub delta_down: u64,
    /// Connected adapters that can be picked from the menu, sorted.
    pub adapters: Vec<String>,
    /// Adapters that were added up for this reading.
    pub measured: Vec<String>,
}

struct Prev {
    ids: Vec<u64>,
    rx: u64,
    tx: u64,
    at: Instant,
}

#[derive(Default)]
pub struct NetSampler {
    prev: Option<Prev>,
}

impl NetSampler {
    /// `wanted` is an adapter name, or empty for every connected hardware adapter.
    pub fn sample(&mut self, wanted: &str) -> NetReading {
        let mut out = NetReading::default();
        let mut ids = Vec::new();
        let (mut rx, mut tx) = (0u64, 0u64);
        unsafe {
            let mut table: *mut MIB_IF_TABLE2 = null_mut();
            if GetIfTable2(&mut table) != 0 || table.is_null() {
                self.prev = None;
                return out;
            }
            let rows =
                std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
            for row in rows {
                let flags = row.InterfaceAndOperStatusFlags._bitfield;
                if flags & FLAG_FILTER != 0 || row.Type == IF_TYPE_SOFTWARE_LOOPBACK {
                    continue;
                }
                let up = row.OperStatus == IF_OPER_STATUS_UP;
                let alias = from_wide(&row.Alias);
                // Idle virtual adapters (Wi-Fi Direct and the like) never carried a byte.
                let used = row.InOctets > 0 || row.OutOctets > 0;
                if up && used && !alias.is_empty() && !out.adapters.contains(&alias) {
                    out.adapters.push(alias.clone());
                }
                let take = if wanted.is_empty() {
                    up && flags & FLAG_HARDWARE != 0
                } else {
                    alias == wanted
                };
                if take {
                    ids.push(row.InterfaceLuid.Value);
                    rx = rx.wrapping_add(row.InOctets);
                    tx = tx.wrapping_add(row.OutOctets);
                    if !out.measured.contains(&alias) {
                        out.measured.push(alias);
                    }
                }
            }
            FreeMibTable(table as *const _);
        }
        out.adapters.sort_by_key(|a| a.to_lowercase());
        ids.sort_unstable();

        let now = Instant::now();
        if let Some(p) = &self.prev {
            // Only compare like with like: a change in the adapter set resets the baseline.
            if p.ids == ids {
                let d_rx = rx.saturating_sub(p.rx);
                let d_tx = tx.saturating_sub(p.tx);
                let dt = now.duration_since(p.at).as_secs_f64();
                out.delta_down = d_rx;
                out.delta_up = d_tx;
                if dt > 0.05 {
                    out.down_bps = d_rx as f64 / dt;
                    out.up_bps = d_tx as f64 / dt;
                }
            }
        }
        self.prev = Some(Prev { ids, rx, tx, at: now });
        out
    }
}
