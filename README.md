# GatiMeter

![icon](res/gatimeter-256.png)

Network speed and system load on the Windows taskbar, with **no kernel driver**.

GatiMeter (गति = speed) shows upload/download speed, CPU, memory, GPU and disk
load, and CPU/GPU/disk temperatures, either inside the taskbar next to the tray
icons or in a small floating window.

## Why

Some taskbar monitors read temperatures through the WinRing0 kernel driver.
Microsoft Defender flags that driver (`HackTool:Win32/Winring0`) because it is
a known vulnerable driver. GatiMeter installs and loads no driver at all. Every
value comes from documented user-mode Windows interfaces, so it runs without
admin rights and does not trip the antivirus.

## Values and where they come from

| Item | Source |
|---|---|
| Upload / download | `GetIfTable2` byte counters |
| CPU usage | PDH `% Processor Utility` (what Task Manager shows) |
| Memory | `GlobalMemoryStatusEx` |
| GPU usage | PDH `GPU Engine` counters, busiest engine |
| Disk activity | PDH `PhysicalDisk % Idle Time`, busiest disk |
| GPU temperature | display driver's adapter performance data (`D3DKMTQueryAdapterInfo`) |
| Disk temperature | `IOCTL_STORAGE_QUERY_PROPERTY` (NVMe, most SATA SSDs) |
| CPU temperature | ACPI thermal zone counter: **approximate**; many PCs report a fixed value here |

Readings Windows does not give are shown as `--`. Run `GatiMeter.exe --probe > probe.txt`
to see exactly what your PC reports.

## Use

* Right-click the readout (taskbar or floating window) or the tray icon for the menu.
* Double-click the readout to open Task Manager.
* Hover for details: today's and this month's traffic, memory in GB, temperatures.
* **Arrange items...** (in the menu) sets which items are shown and in what
  order. The taskbar fills column by column, top then bottom, and the window
  shows each item's place ("Column 2, top"). Changes appear at once.
* **Items to show** has a tick box per item. The menu stays open while you
  tick or untick, so several items can be changed in one go.
* Menu: network adapter, bytes or bits, labels, taskbar side,
  floating-window options (always on top, click-through, lock, opacity,
  colours), font, update interval, traffic history, start with Windows.

Settings: `%APPDATA%\GatiMeter\settings.ini`. Daily traffic:
`%APPDATA%\GatiMeter\history.csv`. "Start with Windows" writes to
`HKCU\...\CurrentVersion\Run`.

## Build

Needs Rust (MSVC toolchain) and the Windows SDK (for `rc.exe`).

```
cargo build --release
```

The program is `target\release\GatiMeter.exe` (single file, about 400 KB).
`python tools/make_icon.py` redraws the icon.

## Limits (0.1)

* Built for Windows 11. On Windows 10 the taskbar readout covers the right end
  of the task-button area instead of making room.
* Only the main taskbar, not those on other monitors.
* The exe is not code-signed, so SmartScreen may ask once ("Run anyway").

## Clean room

GatiMeter was written from a feature description ([docs/spec.md](docs/spec.md)),
not from another monitor's source code. No code, resources, skins or text were
copied from TrafficMonitor or any similar tool.

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE).
