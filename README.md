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

## SSD write warning (optional)

Off unless you turn it on: **SSD write warning > Warn about heavy SSD writes**.
It looks for a program that keeps writing to the SSDs, such as a driver
that rewrites its log file over and over, and shows a notification:

* Every 10 seconds it reads every program's write counter (one
  `NtQuerySystemInformation` call, with no admin rights, and services running
  as SYSTEM included) and what the SSDs really wrote (PDH
  `PhysicalDisk Disk Write Bytes/sec`, only disks without seek penalty).
* Every 10-second step, what the SSDs wrote is shared out among the programs
  that write steadily (in at least 3 of every 4 steps), the most believable
  first. A program whose counter runs above everything the SSDs wrote (a
  browser's pipe traffic, for example) goes last and usually gets nothing.
* It warns when a program's estimated share stays at or above the chosen limit
  (1, 2, 5 or 10 GB per hour; 2 by default) for 15 minutes. One big file copy
  does not count.
* The same program is warned about at most once in 6 hours. Programs listed in
  `write_ignore=` in settings.ini (comma separated, e.g. `steam.exe`) are never
  warned about.
* **SSD write report...** shows the last 15 minutes: what the SSDs wrote, the
  busiest writers, and recent warnings (`%APPDATA%\GatiMeter\write_alerts.csv`).

Limit: without admin rights Windows does not say which file a program writes,
and a program's write counter also includes pipes and network traffic. So the
report names the program, not the file. Resource Monitor (`resmon`, Disk tab)
shows the file. GatiMeter only reads and warns; it never stops a program or
deletes anything.

## Files

Settings: `%APPDATA%\GatiMeter\settings.ini`. Daily traffic:
`%APPDATA%\GatiMeter\history.csv`. "Start with Windows" writes to
`HKCU\...\CurrentVersion\Run`.

## Build

Needs Rust (MSVC toolchain) and the Windows SDK (for `rc.exe`).

```
cargo build --release
```

The program is `target\release\GatiMeter.exe` (single file, about 500 KB).
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
