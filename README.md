# spike-detector

A Windows tray app that watches for anomalous background processes and tells you about them with a notification.
What to do about them is your call: the tray menu lets you end a process, show it in Explorer, or ignore it.
A separate window shows the history of notifications and edits the settings. No administrator rights needed.

## What counts as an anomaly

| Detector      | Condition (defaults)                                                                                                         |
|---------------|------------------------------------------------------------------------------------------------------------------------------|
| `cpu_hog`     | average load ≥ 80% of one core over 5 minutes; the process with the foreground window is left alone                          |
| `idle_busy`   | you've been away from keyboard and mouse for 10 minutes, and a process in your session kept ≥ 25% of a core busy all along   |
| `orphan`      | a dev tool (`node`, `MSBuild`, `dotnet`, …) has run without its parent for over 30 minutes: the terminal is gone, it isn't   |
| `swarm`       | ≥ 25 instances of one dev tool, each alive for over 10 minutes                                                               |
| `mem_leak`    | private bytes grew by ≥ 300 MB over 30 minutes, and the growth is close to linear (R² ≥ 0.8), not a GC sawtooth             |
| `handle_leak` | over 30 minutes, handles grew by ≥ 2000 or threads by ≥ 100, steadily (R² ≥ 0.8)                                             |
| `io_heavy`    | reads + writes average ≥ 10 MB/s over 10 minutes; the foreground process is left alone                                       |

`io_heavy` counts the process's file I/O as Task Manager's "I/O read/write bytes" columns do: reads served from the
file cache are included, socket traffic mostly isn't. True per-process disk counters (`SystemFullProcessInformation`)
return access denied without elevation.

`idle_busy` skips session 0: services live there, and Windows runs its own maintenance exactly when you're idle.
A process that breaks both CPU rules is reported once, as `cpu_hog`. A swarm is one alert per exe name, and its menu
offers to end every instance at once.

The same anomaly is announced at most once an hour. Every threshold lives in the config. **Always ignore X** in an
alert's menu adds X to the config's `ignore` list and leaves the rest of the file, comments included, untouched.

## How it works

Every `poll_interval_secs` (30 s by default) the app makes one `NtQuerySystemInformation(SystemProcessInformation)`
call, which returns CPU time, memory, parent and creation time for **all** processes. That's how Task Manager works
without elevation. No WMI (heavier than the whole app) and no ETW (needs admin).

The call costs about 5 ms of kernel time on a machine with ~400 processes and ~10k threads, because the kernel
reports every thread too. Everything the app does with the result takes about 0.05 ms. Detectors work on
minutes-long windows with history points 30 s apart, so polling faster only adds cost.

```
src/             library shared by both executables, plus the monitor's main.rs
  monitor.rs     the monitor's hidden window, message loop and window procedure
  app.rs         one monitoring pass per tick, tray menu, commands
  sampler.rs     process snapshot (FFI; struct layout checked at compile time)
  tracker.rs     process table, sparse history (a point every 30 s), orphans, descendant tree
  detect/        heuristics: pure logic, no Win32; trend.rs is the least-squares fit both leak detectors share
  alerts.rs      active anomalies, reminders, "hide" and "ignore"
  history.rs     JSON Lines log of notifications
  config.rs      settings
  win/           thin safe wrappers over Win32: tray, menu, window, processes, autostart, shell
ui/              the settings and history window (egui)
```

Kept small on purpose:
- The snapshot buffer exists only during a tick; nothing is held between ticks.
- The timer is coalescable, so Windows can batch our wake-ups with others.
- The process opts itself into EcoQoS.
- A process's command line and path are fetched once, when an anomaly first shows up.
- Files are opened by a short-lived `explorer.exe`, not in-process: `ShellExecute` would load much of the Windows
  shell into the app for good (in a measurement, private memory went from 2 to 5 MB).

A process is identified by (PID, creation time). "End process" re-checks the creation time right before
terminating, so a recycled PID can never get an unrelated process killed.

## Settings and history window

**Settings…** and **History…** in the tray menu open `spike-detector-ui.exe` on the matching tab. It is a separate
program built with egui and lives only while its window is open, so the monitor never loads a GUI toolkit (its
imports have no OpenGL, GDI or COM). The two talk through the same files: the window edits `config.toml`, which the
monitor reloads by itself, and reads `history.jsonl`, which the monitor appends to.

- **History**: every notification, newest first; filter by kind or text (process, details, command line). The most
  frequent processes are listed above the table, the first candidates for a higher threshold or the ignore list.
  Hover a process for its path and command line, right-click to copy the command line or show it in Explorer, or
  **Always ignore** it.
- **Settings**: a form over every setting. Saving writes only the values you changed, so comments and formatting
  in the file stay; invalid values never reach the file. If the file changes elsewhere while the form has unsaved
  edits, the form says so instead of overwriting them silently.

## Install

```
pwsh -File install.ps1
```

Builds a release of both programs, closes running copies, installs them into
`%LOCALAPPDATA%\Programs\spike-detector\` and starts the monitor. Run it again after every change. `-SkipBuild`
installs the existing `target\release` build.

Then tick **Start with Windows** in the tray menu. It writes the per-user `Run` key (no admin), pointing at the
installed exe. If you turn the entry off in Task Manager or Settings > Apps > Startup, the menu shows it as off;
ticking it again turns it back on.

## Icons

`assets/make-icons.ps1` draws the tray icons (teal: all clear, orange: anomalies, grey: paused) at every size from
16 to 256 px, with a simplified shape for the small ones, plus `icon.png` for the window. `build.rs` and
`ui/build.rs` compile them into the executables together with version info from `Cargo.toml`. After editing the script, run `pwsh -File assets/make-icons.ps1` and rebuild.

## Files

`%LOCALAPPDATA%\spike-detector\`
- `config.toml`: commented settings, created on first run and reloaded after each save;
- `history.jsonl`: notification log, one JSON object per line; moved to `history.1.jsonl` past 5 MB.

## Building

```
cargo build --release --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Needs stable Rust and MSVC with the x64 CRT libraries and a Windows SDK. If plain `cargo` fails to link
(`msvcrt.lib` or `ucrt.lib` not found), the newest Visual Studio or Windows SDK on the machine is incomplete;
`install.ps1` works around that by loading the environment of an installation and SDK that have the libraries.

## Prototype limitations

- Elevated processes and other users' processes are visible, but can't be ended without elevation,
  and their command lines aren't readable.
