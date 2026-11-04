# M0 Linux checks: WebKitGTK

Run by rodrigo (partner driver), with Opus 5.5 as orchestrator, under D-033 and gate condition G6. The method is D-037; measuring on battery is D-038. This report covers the Linux/WebKitGTK half of three M0 spikes: ui-shell layout B (M0-06b), stream-perf (M0-07) and browser-screencast (M0-08). The macOS reports remain the reference for what was built. This file only records how the same code behaves on Linux.

**Verdict.** Memory figures are MiB, and the harness was reviewed read-only by Codex (see "Harness review").

| Check | Budget | Linux result | |
|---|---|---|---|
| ui-shell: opaque Linux mode | translucency off; opacity and blur controls disabled, with an explanation | Opaque in every window (verified automatically). Controls disabled, with a Linux note (checked hands-on). One Advanced hint still says macOS (L1) | **pass**, minor text fix |
| ui-shell: frames at 60 Hz | ≥ 97% of the refresh rate in every run | `performance` profile: 59.9–60.0 fps in every section. `balanced` on battery: main batch 58.7–60.0 fps, but 3 sections of the extra runs fell to 54.2–58.0 fps (section 2.3) | **pass** on `performance`; **fail** on `balanced` (battery) |
| ui-shell: frames at 144 Hz | same | Smooth, but rendering is capped at 60 fps (idle rAF 100–126/s), with the 60 fps preference on or off (L10); 120 Hz not measured separately | **fail** |
| ui-shell: TTI, 1 window | 400 ms (contract §5: painted and input handled) | median 472 ms, worst 487 ms; 10/10 over. On `performance`: 452 ms (section 2.4) | **fail** |
| ui-shell: TTI, 3 windows | 400 ms | median 495 ms, worst 562 ms; 10/10 over. On `performance`: 446 ms | **fail** |
| ui-shell: memory, 1 window | 150 MB (PLAN §4, D-028) | PSS 273–277 MB at 30 s, plus GPU 168 MB | **fail** (see L2 on the metric) |
| ui-shell: memory, 3 windows | 220 MB (D-028) | PSS 425–429 MB at 30 s, plus GPU 344 MB | **fail** (see L2) |
| stream-perf: 60 fps at 10k tok/s | 60 fps | 60.0 fps in all 7 runs (1 thread, 10 threads, 5 min); latency p99 40–42 ms | **pass** |
| stream-perf: memory while streaming | reported (macOS: 1.0–1.5 GB footprint) | WebContent PSS 231 → 417 MB, no transient spike observed at 30 s sampling; the daemon keeps 227 MB after close (L8) | reported |
| screencast: fps | ≥ 30 fps | 60.0 at 1× (10 runs) and at 2× | **pass** |
| screencast: click → canvas | p50 and p95 < 80 ms | 1×: p50 19, p95 21 ms; 2×: 35 / 43 ms | **pass** |

## 1. Machine and conditions

| | |
|---|---|
| Machine | Intel Core Ultra 7 258V (8 cores), Intel Arc 140V (`xe`), 30 GB RAM |
| OS | CachyOS, kernel 7.3.0-4-cachyos (7.3.1-1-cachyos for the 2026-11-04 runs) |
| Session | GNOME Shell 51.1 on Wayland |
| WebKitGTK | webkit2gtk-4.1 2.54.2, Mesa 26.2.5 |
| Toolchain | cargo 1.99.1, node 26.12.1, pnpm 12.15.0 |
| Monitors | eDP-1 (built-in) 2560×1440 at 59.961 Hz; HDMI-1 (LG ULTRAGEAR+) 2560×1440 at 143.991 Hz. Until 2026-11-03 the ui-shell windows opened on eDP-1, so those frame numbers are at 60 Hz. The 144 Hz runs used HDMI-1 alone (section 2.3) |
| Power | **On battery** (no charger available, D-038), `balanced` power profile; each log records the battery level at start and end (61% → 54% for the ui-shell batch) |
| Environment variables | no `WEBKIT_*` set, except where a row says `WEBKIT_DISABLE_COMPOSITING_MODE=1` |

Battery and the balanced profile slow the CPU compared with mains power. The TTI numbers in particular should be re-checked on mains before anyone treats the size of the gap as final (section 5).

On 2026-10-29 the monitors reported a different mode (both 1920×1080; eDP-1 at 119.93 Hz, HDMI-1 at 60 Hz), and on 2026-11-01 they were back to the table above. Every log line records the modes that were current at that moment (`host_line()`). On 2026-11-04 they were first mirrored, then HDMI-1 alone.

The orchestrator runs in a Debian distrobox. Every build and measurement ran on the host through `distrobox-host-exec`, as `systemd-run --user` units, so a dropped session can't kill a batch. GUI measurements are serialised with `flock /tmp/atrium-gui-measure.lock`. After each command a sweep checks for leftover spike processes: none were found in any run.

## 2. ui-shell layout B (M0-06b)

Build: `pnpm tauri build --no-bundle` in `spikes/ui-shell`, release binary `src-tauri/target/release/atrium-ui-shell`. Logs: `.orchestration/m0-linux/runs/` on rodrigo's machine; the commands below run in `spikes/ui-shell`.

### 2.1 Opaque Linux mode

```
python3 scripts/measure.py roundtrip
python3 scripts/measure.py cold --runs 1 --opacity background --windows 3
python3 scripts/measure.py cold --runs 1 --opacity whole --windows 3
```

- With opacity requested as `background` or `whole` and 3 windows, every window reports chrome, content and native alpha 1 and vibrancy false. The harness refuses any other result on Linux (`validate_report(…, translucency=False)`, tested in `scripts/test_measure.py`, `OpaqueLinuxMode`).
- Pop-out round trip: 7/7 checks pass (move to pop-out, edit right before close survives, polite and forced bring-back, native window list).
- **Controls and their explanation, checked hands-on by rodrigo on 2026-11-04 in the running app (Settings → Appearance):**
  - "Window opacity" and "Blur behind the window" are disabled.
  - Below them, the app shows the note "This platform draws opaque windows; opacity and blur need a compositor that supports them." (`Settings.tsx`, shown when `opacityLocked`). That covers the PLAN's "disabled, with an explanation".
- **Finding L1: one hint is macOS text.** In Advanced appearance, the always-disabled "Blur strength" hint reads "macOS sets the blur radius; not adjustable", which is wrong on Linux. The note's "need a compositor that supports them" is also a little misleading: GNOME can composite transparent windows, but Atrium doesn't implement translucency on Linux. Both are wording fixes for M1. An earlier version of this report said there was no explanation at all; it had missed the note.

### 2.2 Frames at 60 Hz

```
python3 scripts/measure.py frames --runs 3 --windows 1 --hz 59.961
python3 scripts/measure.py frames --runs 3 --windows 3 --hz 59.961
env WEBKIT_DISABLE_COMPOSITING_MODE=1 python3 scripts/measure.py frames --runs 3 --windows 1 --hz 59.961
```

A section passes when every run reaches 97% of the refresh rate (58.2 fps here). p99 above two frame intervals (33.4 ms) would be flagged.

| Windows | idle | scroll (reading) | scroll (fling) | resize | worst p99 | worst max |
|---|---|---|---|---|---|---|
| 1 | 60.0 | 59.9 | 59.8 | 59.8 | 19 ms | 27 ms |
| 3 | 60.0 | 60.0 | 59.8 | 58.7 | 26 ms (resize) | 34 ms |
| 1, compositing off | 60.0 | 60.0 | 60.0 | 60.0 | 18 ms | 19 ms |

Values are the worst run's fps out of 3. **All pass.** Turning off accelerated compositing doesn't hurt frame pacing on this machine; it is slightly smoother.

### 2.3 Frames at 144 Hz, and extra 60 Hz runs

**144 Hz** (2026-11-04). Setup: HDMI-1 alone (LG ULTRAGEAR+, 2560×1440 at 143.991 Hz, laptop panel off), `balanced` profile, on battery (41% → 39%). The `linux-display` log line confirms every window was on that monitor at 143.991 Hz.

```
python3 scripts/measure.py frames --runs 3 --windows 1 --hz 143.991
env ATRIUM_WEBKIT_NATIVE_FPS=1 python3 scripts/measure.py frames --runs 3 --windows 1 --hz 143.991
env ATRIUM_WEBKIT_NATIVE_FPS=1 python3 scripts/measure.py frames --runs 3 --windows 3 --hz 143.991
```

| worst run of 3, fps | idle | scroll (reading) | scroll (fling) | resize | worst p99 |
|---|---|---|---|---|---|
| 1 window, default | 120.9 | 59.7 | 60.3 | 60.0 | 20 ms |
| 1 window, 60 fps preference off | 126.0 | 60.0 | 60.5 | 60.0 | 19 ms |
| 3 windows, 60 fps preference off | 102.0 | 60.0 | 60.2 | 60.0 | 19 ms |

**Fail against 144 Hz.** WebKitGTK renders every changing frame at 60 fps on the 144 Hz monitor, whether `PreferPageRenderingUpdatesNear60FPS` is on or off. Only when nothing changes does rAF run faster, and irregularly: 100–126 callbacks per second, with the p95 interval still at 17 ms. The 60 fps pacing itself is clean: p99 19–20 ms and max ≤ 33 ms in every section. A stream-perf single run on the same monitor (`linux-single-json-slim-b8-ext144.json`) behaves the same: 60.1 fps, latency p99 42 ms, all events applied. So the 60 fps cap comes from somewhere else in WebKitGTK 2.54's rendering path, not that preference. It wasn't investigated further (L10). 120 Hz wasn't measured separately.

Getting a window onto the fast monitor took three attempts:
1. GNOME opens new windows on the primary monitor, wherever the pointer is.
2. With the screens mirrored, GTK reports the 60 Hz panel (`0x419d @59.960 Hz`), and the app stays at 60 fps even with the preference off.
3. Only HDMI-1 as the only display worked.

The ui-shell spike has a Linux-only diagnostic switch, off by default: `ATRIUM_WEBKIT_NATIVE_FPS=1` turns the preference off in every webview (`src-tauri/src/linux.rs`). Each window also logs the monitor it landed on and the preference's state, and `measure.py frames` copies those lines into the run log.

**Extra 60 Hz runs.** Altogether 25 more frame runs ran at 60 Hz:
- 15 on eDP-1 alone, during the earlier attempts;
- 4 runs × 3 with the screens mirrored, on `balanced`;
- 2 runs × 3 mirrored, on `performance`.

Idle stayed at 60 fps throughout. In `balanced` on battery, **three sections fell under the 58.2 fps threshold**:
- resize, 1 window: 54.2 fps, p99 38 ms;
- resize, 3 windows: 58.0 fps, p99 31 ms;
- scroll fling, 3 windows: 57.7 fps, p99 34 ms.

Every other section reached 58.4–60.0 fps. With the `performance` profile (1 and 3 windows), every section reached 59.9–60.0 fps with p99 ≤ 19 ms. By the rule (every run at ≥ 97%), 60 Hz therefore **passes on `performance` and fails on `balanced` on battery**. The dips happen only under `balanced`, which is consistent with CPU power saving. These runs don't prove that cause.

### 2.4 Time to interactive

```
python3 scripts/measure.py cold --runs 10 --warmup 1 --windows 1
python3 scripts/measure.py cold --runs 10 --warmup 1 --windows 3
```

These are warm launches: one discarded warm-up launch, then 10 recorded ones, opacity off. TTI is measured from process launch to the later of first paint and the handled keystroke (contract §5).

| ms (median of 10) | 1 window | 3 windows | macOS (M0-06b) |
|---|---|---|---|
| exec → main | 30 | 31 | |
| main → JS | 212 | 226 | |
| JS → mounted | 143 | 137 | about 72 |
| input ready | 388 | 411 | |
| first contentful paint | 436 | 458 | |
| **TTI** (= first paint) | **472** (min 451, worst 487) | **495** (min 460, worst 562) | 378–384 |
| all windows ready | 484 | 783 (worst 876) | |

**Fail: 10/10 launches over 400 ms in both configurations.** The keystroke is handled (input ready) at about 388 ms, just under the budget, but the first paint comes about 85 ms later. On macOS that step takes about 60 ms. Mounting the SolidJS tree also takes about twice as long as on macOS. Startup before JS (main → JS, 212 ms) is the largest single block. Fresh-launch TTI after dropping the page cache was not measured (it needs root; open item in D-037).

**Is it the battery?** No charger was available, so on 2026-11-04 the cold runs were repeated with the `performance` power profile (still on battery), against `balanced` controls measured right before:

| TTI median (worst), ms | balanced | performance |
|---|---|---|
| 1 window | 464 (476) | 452 (471) |
| 3 windows | 459 (497) | 446 (463) |

The screens were mirrored for these runs, which explains why the balanced control is a little faster than in the table above. The CPU profile gains only 12–13 ms, and all 40 launches stay over 400 ms. Mains power might add a little more, but the profile result shows the gap isn't a power-saving artefact. TTI fails on Linux because of the app's own startup on WebKitGTK.

### 2.5 Memory

```
python3 scripts/measure.py rss --windows 1          # 3 launches
python3 scripts/measure.py rss --windows 3          # 3 launches
python3 scripts/measure.py rss --windows 1 --trace 300
python3 scripts/measure.py rss --windows 3 --trace 300
env WEBKIT_DISABLE_COMPOSITING_MODE=1 python3 scripts/measure.py rss --windows 1
```

**Metric.** On Linux the harness reports PSS (`/proc/<pid>/smaps_rollup`) summed over the app and all its descendants: WebKitWebProcess, WebKitNetworkProcess, the `bwrap` sandbox helpers and `glycin-svg`. PSS splits shared pages between the processes that map them, so a sum over the tree counts each page once. USS and RSS are reported alongside. GPU memory comes from DRM fdinfo (`drm-total-*` and `drm-resident-*` per client, deduplicated) and is reported apart from PSS, never added to it. Code: `spikes/linuxproc.py`, tests in `spikes/test_linuxproc.py`.

| MB | PSS at 10 s | PSS at 30 s | USS at 30 s | RSS at 30 s | GPU (DRM) | Budget |
|---|---|---|---|---|---|---|
| 1 window (4 launches) | 282.8–287.6 | 272.8–277.1 | 221.2–225.4 | 479–484 | 168 | 150 |
| 1 window, compositing off | 285.5 | 275.2 | 223.7 | 481 | 153 | 150 |
| 3 windows (4 launches) | 434.1–437.8 | 424.9–428.9 | 314.3–318.5 | 921–925 | 344 | 220 |

The 4 launches include the start of each 300 s trace. GPU values come from the launches made after the fdinfo fix (section 4, L5); the earlier launches' GPU readings are invalid and aren't used.

300 s traces at 1 Hz: 1 window: min 277.1, median 281.5, max 296.4 MB; 3 windows: min 428.9, median 428.9, max 456.9 MB. The maximum is the first sample in both. After about 10 s, PSS settles and doesn't grow. There is no late growth like the macOS GPU-process jump of about 40 MB.

**Where the 1-window PSS goes** (one launch, `pss-breakdown.py`, PSS by mapping type):

| Process | anon | shared libraries | files |
|---|---|---|---|
| atrium-ui-shell | 43.5 | 34.6 | 5.4 |
| WebKitWebProcess | 72.2 | 85.5 | 2.7 |
| WebKitNetworkProcess | 17.4 | 15.4 | 0 |
| glycin-svg, bwrap | 0.9 | 3.0 | 2.2 |
| **Total** | **133.9** | **138.4** | **10.3** |

GPU buffers are not mapped into the processes (0 in smaps). They show only in DRM fdinfo: 168 MB resident.

**Fail as measured, but the metrics don't match (L2).** macOS budgets `phys_footprint`. That counts a process's dirty private memory, including compressed memory and the graphics memory attributed to it, and leaves out clean shared-library pages from the dyld shared cache. Linux PSS works the other way on both points:
- It includes a proportional share of every library mapped (WebKitGTK, GTK, Mesa, ICU): 138 MB here, about half the total.
- It leaves out GPU memory.

The anonymous memory alone, 134 MB for one window, is in the same range as macOS footprint (133–140 MB). Anonymous memory plus GPU is about 300 MB. Nobody has decided which Linux number the PLAN §4 budget applies to. Each pop-out adds about 75 MB of PSS and about 88 MB of GPU memory, against about 35 MB of footprint on macOS (D-028). This needs a decision: see section 5.

## 3. stream-perf (M0-07)

Build: `pnpm tauri build --no-bundle` in `spikes/stream-perf`. On Linux, `bench.py` reads the process tree from `/proc` (`linuxproc`), and every `*_footprint_mb` field holds **PSS**, not `phys_footprint`. Reports: `spikes/stream-perf/measurements/linux-*.json`. These runs were on 2026-11-01, on battery (93% → 87%), on the 60 Hz panel.

```
python3 scripts/bench.py single --secs 60 --encoding json-slim --batch-ms 8 --label linux-single-json-slim-b8-r{1,2,3}
python3 scripts/bench.py multi  --secs 60 --encoding json-slim --batch-ms 8 --label linux-multi-json-slim-b8-r{1,2,3}
python3 scripts/bench.py memory --secs 300 --encoding json-slim --batch-ms 8 --label linux-memory-json-slim-b8
```

### 3.1 1 thread × 10k tokens/s (3 runs of 60 s)

| | Linux, 3 runs | macOS (M0-07 §3.1, median) |
|---|---|---|
| tokens/s | 9,993 / 9,996 / 9,997 | |
| events received = applied | all (599,835 / 600,021 / 600,030), no backpressure waits | |
| fps | **60.0 / 60.0 / 60.0** | 60.0 |
| frame interval p99 / max (ms) | 19 / 27, 18 / 37, 18 / 23 | 19 / 24 |
| dropped frames | 1, 1, 0 | 0 |
| latency (agent → next rAF) p50 / p95 / p99 / max (ms) | 30 / 39 / 41–42 / 58–89 | 30 / 39 / 75 / 226 |
| apply per frame (JS + Solid) p99 / max (ms) | 4 / 39 (r1) | 2 / 12 |
| CPU (% of a core): app / WebContent / daemon / agents | 22–24 / 41–47 / 52–62 / 3 | daemon 65–86 |

### 3.2 10 threads × 1k tokens/s, one visible, synthetic typing (3 runs of 60 s)

| | Linux, 3 runs | macOS (M0-07 §3.2, median) |
|---|---|---|
| total tokens/s | 9,935 / 9,893 / 9,933 | 9,868 |
| fps / dropped frames | **60.0** / 0 in every run | 60.0 / 1 |
| frame interval p99 / max (ms) | 18–19 / 21–22 | 19 / 33 |
| latency (visible thread) p50 / p99 / max (ms) | 29–30 / 40–41 / 46–67 | 30 / 41 / 89 |
| keystrokes sent = inserted | 476 / 478 / 476 | 479 |
| timer lateness while streaming p50 / p99 / max (ms) | 2.1 / 4.0–4.6 / 4.3–6.1 | 2.3 / 5.0 / 5.9 |
| keystroke → paint, from the deadline, streaming p50 / p95 / p99 / max (ms) | 27.4–27.9 / 35.4 / 36.7–37.0 / 37.7–38.0 | 26.7–27.1 / 34.8–35.3 / 36.1–37.2 / 37.6–38.2 |
| same, idle reference p50 / p99 (ms) | 22.6–28.0 / 32.6–35.3 | 24.5–27.8 / 32.9–34.1 |
| CPU: app / WebContent / daemon / agents | 21 / 31–32 / 100–105 / 15 | 18 / 16 / 85 / 25 (earlier build) |

A fourth single run, `linux-single-json-slim-b8-144.json`, was meant for 144 Hz but ran on the 60 Hz panel (section 2.3). Its results are the same as the three above: 60.0 fps, 0 dropped frames, latency p99 41 ms.

**Pass.** Linux matches macOS on every streaming and typing figure. Its latency tails are, if anything, tighter. The input-latency caveat from M0-07 still applies: the typing is synthetic and doesn't prove "no dropped input". The daemon is the busiest process at a full core with 10 threads, the same as on macOS.

### 3.3 Memory over 5 minutes (1 thread, 10k tokens/s, 300 s)

3.0 M tokens (12.0 M characters) streamed; 79,853 rows; all 3,000,015 events applied; 60.0 fps with 22 slow frames (max 37 ms) over 5 minutes; latency p99 41 ms.

| MB (PSS) | while streaming (t = 30 → 275 s) | after the stream, +0 / +15 / +60 s | macOS footprint (M0-07 §3.3) |
|---|---|---|---|
| WebContent | 231 → 417, growing steadily | 355 / 329 / 327 | 1,050–1,390 streaming; 357 / 248 / 248 after |
| app process (Rust) | 80 | 115 / 111 / 111 | 37–38 |
| other WebKit (network, sandbox, SVG) | 31 | 31 | 81–94 streaming, 26 after |
| daemon + agents | 7 + up to 13 | **372 / 227 / 227** | 487 at close, **8.9** 15 s later |
| total | 351 → 548 | 872 / 698 / 696 | 321 / 320 after 15 / 60 s |
| GPU (DRM, apart) | 368–379 | 370 | (in footprint) |

Two differences from macOS:
- **No transient spike in WebContent was observed** at the 30 s sampling interval (the same interval as on macOS). A shorter peak between samples would be missed. On macOS, footprint is four to five times the retained size while streaming. On Linux, WebContent grows steadily to about 417 MB, and settles at about 330 MB PSS afterwards, against 248 MB of footprint on macOS. The two metrics differ (section 2.5), so this is no evidence that one platform retains more.
- **The daemon keeps its close-time peak (L8).** At session close the daemon folds the 3 M-event history (CR-03; M0-05's known behaviour). macOS returns that memory within 15 s. On Linux, 227 MB is still resident 60 s later. This looks like glibc's allocator holding freed memory and is a candidate for M1 (`malloc_trim`, or a different allocator for the daemon). Not investigated further.

## 4. browser-screencast (M0-08)

Build: `pnpm tauri build --no-bundle` in `spikes/browser-screencast`. Browser: Brave (`/usr/bin/brave`, Chrome/156.0.8078.48), set with `ATRIUM_BROWSER`. On Linux, `bench.sh` wraps each run in `scripts/procsample.py`, which samples PSS and CPU per group (app, browser tree, WebKit) at 1 Hz from `/proc`. `procstat.rs` reads `ps` CPU time, which has 1 s resolution on Linux, so its CPU figures are coarse. Those runs were on 2026-11-01, on battery (87% → 84%), on the 60 Hz panel.

```
scripts/bench.sh 10 <out>/screencast-1x ATRIUM_BROWSER=/usr/bin/brave ATRIUM_DSF=1
scripts/bench.sh 1  <out>/screencast-2x ATRIUM_BROWSER=/usr/bin/brave ATRIUM_DSF=2 ATRIUM_EXTRA_ARGS=--force-device-scale-factor=2
```

| | 1× (1280×747), 10 runs: median (range) | 2× (2560×1494), 1 run | Target | macOS 1× / 2× |
|---|---|---|---|---|
| fps while streaming | **60.0** (60.0–60.1) | **60.0** | ≥ 30 | about 60 / 57.7 |
| draw interval max (ms); intervals over 33 ms | 20–28; 0 | 28; 0 | | |
| click → canvas p50 (ms) | **19** (18–19) | **35** | < 80 | 12 / 27 |
| click → canvas p95 (ms) | **21** (20–22) | **43** | < 80 | 19 / 34 |
| click → canvas max (ms); misses | 21–86 (one probe of 1,000 over 80); 0 | 54; 0 | | |
| frame size (KiB, JPEG q80) | 38–42 | 95 | | |
| context switch to first frame (ms) | 13–20 | 40 | | |
| browser CPU while streaming (`procstat`, % of a core) | 82 (74–85) | 189 | | |
| app process CPU while streaming | 20–21 | 21 | | |
| PSS: browser tree / WebKit / app (MB, max per run) | 449 (448–456) / 168 / 81 | 588 / 189 / 82 | | |
| GPU (DRM): WebKit / app (MB) | 105–115 / 44 | 193 / 44 | | |

**Pass at both scales.** Click → canvas is about 6 ms slower than on macOS at the median, and well inside the 80 ms target. The 2× pane costs about two browser cores, as on macOS.

Not measured:
- **Brave's GPU memory.** Its GPU process has no readable DRM fdinfo; the sandboxed processes aren't readable by the user, so `browser gpu_drm` is empty.
- **One browser process's PSS**, in some samples (`unreadable` = 1). The browser PSS can be a few MB low.

Chromium was not measured, only Brave. Both are Chromium-based and use the same CDP screencast path.

## Harness review (Codex, read-only)

D-037 asked for an independent read-only review of the harness before its numbers count. Codex 0.164.0 ran it on 2026-11-04 (`codex exec -s read-only`, with the prompt and result kept in `.orchestration/m0-linux/review/`). Both test suites passed, and no files were changed. The findings, and what was done about each:

| Finding | Severity | Effect on this report | Action |
|---|---|---|---|
| CPU % came from group totals, so a helper exiting or starting skews the delta | major | The stream-perf CPU figures (§3) were measured that way. During a stream the process set is stable, so treat them as approximate. §4's CPU figures come from `procstat`, not this sampler | Fixed: per-process deltas in `bench.py` and `procsample.py`, unit-checked |
| Browser PSS summed even when a process was unreadable | major | Checked in the raw data: unreadable samples occur only in the first 1–4 s while Brave starts. The median and maximum over readable samples equal the §4 values | Fixed: PSS statistics skip partial samples |
| The 60 Hz verdict said pass despite runs under the threshold | major | Verdict corrected: pass on `performance`, fail on `balanced` on battery. The power-saving cause is stated as consistent with the data, not proven | Report fixed |
| GPU values are DRM **total** summed per client, not unique resident memory | minor | Read §2.5, §3.3 and §4 GPU figures as summed client allocations. In the readings checked, total equalled resident | Noted |
| "No transient spike" overstates 30 s sampling | minor | Reworded to "none observed at 30 s sampling" | Report fixed |
| "MB" is MiB (kB ÷ 1024) | nit | Every memory figure in this report is MiB. The conclusions don't change | Noted here |

The review found no defect in the PSS grouping, opacity validation, warm-up exclusion, the Linux code paths, or the FFI in `linux.rs` (ownership, thread and casts match the WebKitGTK headers). Codex's verdict was "numbers need re-measuring" for browser PSS and CPU under process churn. The raw data above shows the reported figures weren't affected, so nothing was re-measured. The fixed harness applies to any future run.

## 5. Linux findings

- **L1. The "Blur strength" hint is macOS text on Linux.** Section 2.1. The disabled controls do have a Linux note. The fixes for M1 are wording only:
  - a platform-aware hint for "Blur strength";
  - a note that doesn't blame the compositor, such as "Atrium uses opaque windows on Linux."
- **L2. Memory budget metric for Linux is undefined.** Section 2.5. For Pedro to decide. Candidates for one window:
- PSS without shared-library pages (anon + mapped files): about 144 MB, within 150.
- The same plus GPU (DRM), closer to what `phys_footprint` counts: about 310 MB.
- Full PSS: about 275 MB.

Whichever is chosen, a pop-out costs about twice D-028's allowance on Linux: 75 MB PSS, plus GPU.
- **L3. TTI misses the budget by about 45–95 ms on this machine.** Section 2.4. The `performance` profile gains only 12–13 ms, so power saving isn't the cause. The largest gap from macOS is between mount and first paint. M1 should profile startup on WebKitGTK: main → JS, mount, and the first composite.
- **L4. Harness: GUI-lock deadlock (fixed).** stream-perf's `bench.py` takes the GUI lock itself around each run, so wrapping it in `flock` deadlocked the batch. The wrapper now skips the lock for `bench.py`.
- **L5. Harness: xe's DRM cycle counters (fixed).** The `xe` driver exposes `drm-total-cycles-<engine>`. A prefix match on `drm-total-` read those counters as bytes and reported hundreds of GB of GPU memory. Memory regions only now; covered by a test.
- **L6. Harness: power detection (fixed).** A wireless mouse's battery (`hidpp_battery_0`) also exposes `online`, so the first `on_ac()` reported mains power while the machine was on battery. Only `Mains` and `USB` supplies count now.
- **L7. Environment.** The orchestrator ran in a distrobox. GUI apps, git and the agent CLIs only work on the host (`distrobox-host-exec`, and `zsh -lic` for the user's `PATH`). The rebase glitch of D-030 didn't come up on this machine.
- **L8. The daemon doesn't give back its close-time memory on Linux.** Section 3.3. 227 MB is still resident 60 s after a 3 M-event session closes, where macOS drops to 9 MB. This adds to CR-03's lifecycle-fold issue. For M1: `malloc_trim` after the fold, a different allocator, or avoiding the fold altogether.
- **L9. Streaming and screencast are not Linux risks.** Sections 3 and 4. WebKitGTK keeps 60 fps at 10k events/s and with the screencast pane. The typing probe matches macOS within 1 ms, and click → canvas is within 6 ms of macOS.
- **L10. WebKitGTK renders at 60 fps on a 144 Hz display.** Section 2.3. Content updates ran at 60 fps on the 144 Hz monitor, with `PreferPageRenderingUpdatesNear60FPS` (on by default in 2.54) either on or off; only an idle page runs rAF faster. So turning that preference off isn't enough; the cap comes from elsewhere in WebKitGTK's rendering path. Pacing at 60 fps is clean. WebKit on macOS has a preference of the same name, and the macOS spikes ran only on a 60 Hz panel, so high refresh is unverified on macOS too. For M1 and Pedro: either accept 60 fps on high-refresh displays for now, or budget time to find the cap (WebKitGTK compositor and vblank monitor) before promising 120 Hz.

## 6. Reproducing

1. Build each spike: `pnpm install && pnpm tauri build --no-bundle` in `spikes/<spike>`.
2. Run the commands listed in each section from that spike's directory, one at a time, with the window on the monitor under test. A covered window stops painting under WebKitGTK and the run is discarded.
3. Harness tests: `python3 spikes/test_linuxproc.py`, `python3 spikes/ui-shell/scripts/test_measure.py`.
