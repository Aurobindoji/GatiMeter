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
  disk temp. Each has a box (☑ shown, ☐ hidden), listed in the arranged
  order. Clicking an entry, or Space/Enter on it, toggles it without closing
  the menu.
* Arrange items…: a window listing every item with a tick box (shown or
  hidden), Move up / Move down, Reset order, and each shown item's place on
  the taskbar ("Column 2, top"). The order is kept for hidden items too and is
  stored as `order=` in the settings file. The floating window uses the same
  order, top to bottom.
* Network adapter: Automatic, or a named adapter
* Disk temperature: Hottest disk (automatic, the default), or one disk by
  model name (saved as `disk_temp_drive`); the tooltip lists every disk
* Speed units: bytes (KB/s) or bits (Kb/s)
* Labels on/off
* Taskbar position: next to the tray icons, or at the left end
* Floating window: always on top, click-through, lock position, transparent
  background, opacity, background colour
* Font…, text colour (automatic or chosen)
* Update interval: 0.5, 1, 2 or 5 s
* Traffic history…
* SSD write warning: Warn about heavy SSD writes (on/off, on by default since 0.1.6),
  Warn above 1 / 2 / 5 / 10 GB per hour, Ignored programs (tick boxes, since
  0.1.8: the ignored programs first, then the last 5 warned ones, then the
  busiest writers of the last 15 minutes; at most 20, names in lower case),
  SSD write report…
* Open Task Manager, Start with Windows, Open data folder, About, Exit

## Dark and light theme

The menu, the text window (traffic history, SSD write report) and the
Arrange window follow the Windows app theme (Settings > Personalisation >
Colours > app mode), checked each time the menu or a window opens:

* Menus: uxtheme `SetPreferredAppMode` (ForceDark / ForceLight) and
  `FlushMenuThemes`, exported by ordinal 135 and 136 since Windows 10 1903.
  If they are missing, menus stay light.
* Title bars: `DwmSetWindowAttribute(DWMWA_USE_IMMERSIVE_DARK_MODE)`.
* Controls: `SetWindowTheme` with "DarkMode_Explorer" (buttons, list, scroll
  bars) and "DarkMode_ItemsView" (list header); text boxes, labels and the
  list get background #202020 and text #E8E8E8.

Message boxes and the font and colour pickers are drawn by Windows and stay
light. The taskbar readout keeps following the taskbar's own theme.

## SSD write warning

On by default since 0.1.6 (off by default in 0.1.3–0.1.5); a settings file
that already holds `write_watch=` keeps the user's choice. Its purpose is to catch a program that keeps
writing to the SSD while nothing visible happens (for example a driver
service that rewrites its log file again and again).

Sources, all without admin rights:

* Every process's write I/O byte counter, read for all processes at once
  with `NtQuerySystemInformation(SystemProcessInformation)` (the I/O counters
  of the documented `SYSTEM_PROCESS_INFORMATION`). Opening each process with
  `OpenProcess` was rejected: without admin rights it fails for services that
  run as SYSTEM, which are the main suspects. This counter also counts pipes
  and some network traffic.
* What the SSDs wrote: PDH `\PhysicalDisk(*)\Disk Write Bytes/sec`, summed over
  the disks whose `StorageDeviceSeekPenaltyProperty` says no seek penalty. If
  no such disk is found, all disks are summed.

Rule: the readings form 10-second steps; the last 90 steps (15 minutes) are
kept. The SSD bytes of each step are shared out among the steady writers:
processes that wrote something in at least two of every three minutes of the
window, where no single minute holds more than half of their bytes. (Until
0.1.4 the rule was "at least 75 % of the 10-second steps"; that missed a
service that rewrites a whole log file every 30 s, which is idle in most
steps. A user report of driver 621.14 showed exactly that pattern.)

1. Order: smallest "overshoot" first (the bytes by which the process's
   counter ran above the SSD total, summed over the steps), then the bigger
   writer, then the lower process id.
2. A writer takes part only if, in at least 75 % of its steps, its bytes fit
   what is left of that step: at most twice the bytes left plus 1 MB (cache
   flushes that land a moment later, rewrites the cache merged).
3. It then takes `min(its bytes, bytes left)` from each step. The sum is its
   estimated share of the SSD writes.

A process is warned about when the window is full and its share reaches the
limit for that time. This came from a live test: a browser whose counter
(mostly pipe traffic) ran above everything the SSDs wrote was blamed together
with the real writer under the first, simpler rule (counter and SSD total both
above the limit).

Idle and System (process ids 0 and 4) are listed but never warned about. A
process id that now belongs to another program starts from zero. A step longer
than 60 s (sleep, hibernation) starts a new window.

The warning is a notification from the tray icon (shown just for the warning
if the icon is hidden). Clicking it opens the SSD write report. Each warning is
added to `%APPDATA%\GatiMeter\write_alerts.csv`
(`time,process,pid,process_bytes,ssd_share_bytes,ssd_bytes,seconds`).

While a program stays a suspect, the warning repeats every hour (until 0.1.8:
once in 6 hours, far too quiet for a bug that writes GBs per hour). Each line
of the warning gives the program's rate, about how much it has written since
its heavy writing began, and for how long ("NVDisplay.Container.exe: 5.12
GB/h, about 6.40 GB in 1 h 15 min"); from the second warning on the title is
"Still heavy SSD writes". The running total adds, at each repeat, the current
window's share scaled to the time since the previous warning. A program last
warned about more than 2 hours ago starts a new count. Settings: `write_watch`, `write_limit_gb`
(1–100), `write_ignore` (comma-separated program names).

The report window shows the watched disks, what they wrote in the window, the
busiest writers (program, process id, write I/O counter, estimated share
per hour, active minutes out of the window's minutes, Likely), the ignore list and the last 10 warnings.

Hint (0.1.10): only web browsers (brave, chrome, msedge, firefox, opera,
vivaldi, librewolf, waterfox, chromium, thorium) are called "usually normal";
their heavy writes are cache, history and tabs. Every other program, even a
well-known heavy writer such as a torrent client, can go wrong, so the warning
says "worth checking". The balloon adds one hint line; it drops the SSD total,
then later programs, if the text would pass Windows' 255-character limit. The
report's Likely column shows "browser - usually normal", "worth checking"
(other programs at or above the limit) or "-".

`--probe` also prints the SSD drive numbers found and the ten processes that
wrote most since they started.

## Traffic history

Bytes sent and received are added up per local calendar day and kept in
`%APPDATA%\GatiMeter\history.csv` (`date,upload_bytes,download_bytes`). The
history window shows today, this month, monthly totals and daily totals.

## Settings

`%APPDATA%\GatiMeter\settings.ini`, plain `key=value` lines. Keys added later
(such as `write_watch`) default to their documented value when missing. Start with Windows
uses `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (no admin rights).

## Non-goals for 0.1

* No kernel driver, ever.
* No plug-ins, skins or translations yet.
* Windows 11 is the target. On Windows 10 the taskbar window sits over the
  right end of the task-button area instead of making room for itself.
* Only the main taskbar (not the taskbars on other monitors).
