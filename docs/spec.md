# GatiMeter — feature specification (clean room)

GatiMeter is a small Windows program that shows network speed and system load
on the taskbar and in a floating window.

## How this was written (clean room)

* This spec was written from what a user can see in such tools (a taskbar
  speed meter is a common kind of program) and from what the owner of this
  project asked for. It describes behaviour, not code.
* No source code, resources, skins, icons, translations, file formats or text
  were taken from TrafficMonitor or any other monitor. Its repository was not
  opened while this project was written. Names, menus, settings keys, file
  layout and the drawing method are GatiMeter's own.
* GatiMeter uses only documented, user-mode Windows APIs. It installs no
  kernel driver and loads none. That is the main reason it exists: the
  WinRing0 driver used by some monitors is flagged by Microsoft Defender
  (`HackTool:Win32/Winring0`, a vulnerable signed driver).

## Values shown

| Item | Source (user mode only) |
|---|---|
| Upload / download speed | `GetIfTable2` byte counters, summed over the chosen adapters |
| CPU usage | PDH `\Processor Information(_Total)\% Processor Utility` (what Task Manager shows); falls back to `GetSystemTimes` |
| Memory usage | `GlobalMemoryStatusEx` |
| GPU usage | PDH `\GPU Engine(*)\Utilization Percentage`, busiest engine |
| Disk activity | PDH `\PhysicalDisk(*)\% Idle Time`, busiest disk (100 − idle) |
| CPU temperature | PDH `\Thermal Zone Information(*)` (ACPI). Approximate; many desktops do not report it |
| GPU temperature | `D3DKMTQueryAdapterInfo` adapter performance data (the same data Task Manager uses) |
| Disk temperature | `IOCTL_STORAGE_QUERY_PROPERTY` / `StorageDeviceTemperatureProperty` (NVMe and most SATA SSDs) |

A value that Windows does not provide is shown as `--`.

Network adapters:

* **Automatic** (default): every connected hardware adapter (Wi-Fi, Ethernet,
  USB tethering). Filter layers, loopback and virtual tunnels are skipped, so
  VPN traffic is not counted twice.
* Or one adapter picked by name from the menu.

## Where it is shown

1. **Taskbar**: a small window placed inside the Windows taskbar, just left of
   the notification-area icons (or at the left end, if chosen). Two rows when
   the taskbar is tall enough, one row otherwise. Text colour follows the
   Windows light/dark taskbar unless a colour is chosen.
2. **Floating window**: a borderless window that can be dragged anywhere.
   Options: always on top, click-through (mouse passes through it), lock
   position, opacity, background colour or transparent background.
3. **Notification-area icon** with a short tooltip; its menu is always
   reachable, so a click-through window can still be turned back.

Hovering the taskbar or floating window shows a detailed tooltip (all values,
today's and this month's traffic). Right-click opens the menu; double-click
opens Task Manager.

## Menu

* Show in taskbar / Show floating window
* Items to show: upload, download, CPU, memory, GPU, disk, CPU temp, GPU temp,
  disk temp
* Network adapter: Automatic, or a named adapter
* Speed units: bytes (KB/s) or bits (Kb/s)
* Labels on/off
* Taskbar position: next to the tray icons, or at the left end
* Floating window: always on top, click-through, lock position, transparent
  background, opacity, background colour
* Font…, text colour (automatic or chosen)
* Update interval: 0.5, 1, 2 or 5 s
* Traffic history…, Open Task Manager, Start with Windows, Open data folder,
  About, Exit

## Traffic history

Bytes sent and received are added up per local calendar day and kept in
`%APPDATA%\GatiMeter\history.csv` (`date,upload_bytes,download_bytes`). The
history window shows today, this month, monthly totals and daily totals.

## Settings

`%APPDATA%\GatiMeter\settings.ini`, plain `key=value` lines. Start with Windows
uses `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (no admin rights).

## Non-goals for 0.1

* No kernel driver, ever.
* No plug-ins, skins or translations yet.
* Windows 11 is the target. On Windows 10 the taskbar window sits over the
  right end of the task-button area instead of making room for itself.
* Only the main taskbar (not the taskbars on other monitors).
