# MScout — v1 Spec

A local, read-only companion overlay for **MapleStory Classic World** (Nexon, live since 2026‑10‑21, currently v1.2.0). v1 does one thing: it watches your EXP bar, records it, and plots how fast you are gaining EXP.

Target is Classic World specifically — not original MapleStory, not a pre-Big Bang private server. Formulas, EXP tables, map lists and level cap all come from Classic World data (cap 100, four jobs, 2nd job ceiling).

**Implementation status (2026-11, v0.1.0):** WGC capture, calibration, digit matching, map/name/job fingerprints and JSONL storage are implemented; the overlay renders real samples. Remaining work is listed in [docs/ROADMAP.md](ROADMAP.md), rationale in [docs/DECISIONS.md](DECISIONS.md).

---

## Table of contents

1. [Hard constraints](#1-hard-constraints)
2. [v1 scope](#2-v1-scope)
3. [Capture contract](#3-capture-contract)
   - [3.1 Target window](#31-target-window)
   - [3.2 Calibration](#32-calibration)
   - [3.3 Cadence](#33-cadence)
4. [Parsing](#4-parsing)
   - [4.1 EXP](#41-exp)
   - [4.2 Map name](#42-map-name)
5. [States and warnings](#5-states-and-warnings)
6. [Data integrity rules](#6-data-integrity-rules)
7. [Rate math](#7-rate-math)
   - [7.1 Idle](#71-idle)
   - [7.2 ETA to next level](#72-eta-to-next-level)
8. [Live panel](#8-live-panel)
9. [History window](#9-history-window)
10. [Storage](#10-storage)
    - [10.1 Samples](#101-samples)
    - [10.2 Settings](#102-settings)
    - [10.3 Retention](#103-retention)
11. [EXP table](#11-exp-table)
12. [Events](#12-events)
13. [Explicit non-goals for v1](#13-explicit-non-goals-for-v1)
14. [Open decisions](#14-open-decisions)

---

## 1. Hard constraints

The helper must never touch the game process. That is the design boundary, not a preference.

**Allowed**
- Windows Graphics Capture / DXGI Desktop Duplication of the game window (OS compositor APIs)
- Cropping, image processing, digit/template matching, text OCR against a known map list
- A separate always-on-top window composited by Windows
- Global hotkeys via `RegisterHotKey` (no low-level keyboard hooks)
- Local file storage under `./data/`

**Forbidden — no exceptions**
- `ReadProcessMemory` / `WriteProcessMemory`, handles into the game process
- DLL injection, D3D/API hooking, packet capture or manipulation
- `SendInput` / `keybd_event` / `PostMessage` to the game window — no auto-pot, auto-attack, macros, auto-relog
- Any pixel → in-game action loop
- Process renaming, packing, obfuscation, or anti-cheat evasion
- Multi-client support
- Shipping datamined client assets (stats and numbers are fine; art is not)

**Required player setting**: game runs windowed or borderless fullscreen. Exclusive fullscreen cannot be captured or overlaid without hooking, which is forbidden.

Compliance note: per Nexon's rules, botting is using a macro, third-party program or physical method to play the game repetitively. This tool reads and displays; it never plays. That distinction is the whole reason the constraint list above exists, and it must stay true in every future feature.

---

## 2. v1 scope

In:
- Auto-read of the EXP readout and the current map name from the game window
- Local sample storage
- Live panel: current map, level, EXP %, 1-minute rate, 5-minute average, ETA, last-30-minute chart
- ETA to next level on a 30-minute rate
- History window: browse and chart all stored data, back through months

Out (later versions): damage/AP/potion calculators, period-to-period comparison, per-map comparison view, multi-character profiles.

---

## 3. Capture contract

### 3.1 Target window
- The player selects the game window once. The choice persists via window title/process match.
- On startup, if the window is not found, report it immediately. Never idle silently.

### 3.2 Calibration
Regions of interest (ROIs), stored as physical pixels relative to the game window's client area so moving the window is harmless:

1. **EXP readout** — the status-bar EXP text, e.g. `10,105 [91.53%]` (required)
2. **Level digits** — the level tile next to the HP/MP bars (required)
3. **Map label** — optional; only a fallback, since the minimap panel is detected automatically
4. **Character name**, **Job** — optional fingerprints shown in the overlay

Calibration flow: show a frozen preview frame, player drags a box for each ROI, live parse feedback appears immediately ("Lv 17 · 10,105 EXP · 91.53% · Henesys"). Calibration is not accepted until the required ROIs parse cleanly in the live preview.

Calibration is scoped to the **client size**: each resolution keeps its own profile (`data/profiles/<W>x<H>.json`), and the engine activates the profile matching the live window. Re-calibration is required for a client size that has none, or after a patch that moves the status bar; a resolution change switches profiles instead of invalidating them. The app must detect this situation (no profile, parse failure) rather than keep plotting.

### 3.3 Cadence
- One frame per second, cropped to the ROIs only.
- Capture continues when the window is unfocused or occluded.
- Minimized, closed, or capture-denied windows are reported states (see §5), not silent gaps.

---

## 4. Parsing

### 4.1 EXP
- **Digits use sprite matching, not general OCR.** The UI font is fixed; each glyph is learned once by having the player type the string currently shown (`atlas learn`). Matching is fast and far more accurate than OCR at this font size.
- Parse yields: level, absolute EXP within level, and percentage.
- **Cross-check**: `percentage ≈ exp / exp_required(lv)`. If the two disagree by more than ~1 percentage point, the sample is rejected as a misread.
- Level is read from the same status area; if unavailable from the EXP ROI, a third tiny ROI is calibrated for it.

### 4.2 Map name

- **Bitmap fingerprint, not OCR.** The label is pixel-identical for the same map at the same resolution, and WebView has no access to a system OCR engine. The player teaches each map once, after which recognition is a nearest-match against the taught set.
- **The label is located, not boxed.** The minimap can be moved and resized by the player, so its panel is detected on every frame and the name crop is normalised to a fixed grid before matching. Moving or resizing the minimap therefore never requires re-teaching.
- The game draws some names on two lines (`Hidden Street` / `1st Accompaniment <Bonus>`); the stored label keeps the newline and the overlay renders it the same way.
- Recognition is exact-match oriented: an untaught or unrecognisable map reports `unknown` and is never guessed from partial similarity.
- Fingerprints are resolution- and UI-scale-specific; a new client size needs its own taught set.
- Rationale and rejected alternatives: [docs/DECISIONS.md](DECISIONS.md) D5, D17.

---

## 5. States and warnings

Three visible states, always shown in the overlay:

| State | Meaning | UI |
|---|---|---|
| **Reading** | Last sample parsed cleanly | normal |
| **Degraded** | ≥3 parse failures in the last 10 attempts | yellow banner; chart shades the unreliable span |
| **Lost** | Window gone, minimized, capture denied, or ROI no longer readable | red banner naming the cause, "recalibrate" action, **desktop notification** |

Notifications exist so a broken capture is discovered while playing, not three hours later.

---

## 6. Data integrity rules

- **Never write a zero or a guess.** A failed parse produces no sample. Broken capture therefore appears as missing data, never as "stopped gaining EXP".
- **Reject impossible jumps.** An EXP delta not explained by a level-up or a death marks that segment unreliable and excludes it from rate calculations.
- **Three distinguishable states in the recording**: samples present and EXP rising (`gaining`), samples present and EXP unchanged (`idle`), no samples at all (`not_recording`).
- Deaths and level-ups are recorded as events, not as negative EXP.

---

## 7. Rate math

All rates use **wall-clock time** and **trailing differences** over stored raw samples. Windows are view parameters — no re-recording is needed to change them.

```
rate_per_hour(t, W) = (exp_cumulative(t) − exp_cumulative(t − W)) / W_hours
```

- **1-minute rate** (default, solid line): most responsive, visibly jumpy by design — one 300 EXP kill shifts it by ~18,000 EXP/hour.
- **5-minute average** (faint line underneath): the stable trend. The prominent EXP/hour and the ETA both use this window.
- **30-minute rate** (dashed): context only; it no longer drives the ETA.
- Windows are configurable; defaults are 1 / 5 / 30 minutes.
- The 1-minute line is not drawn until ~60 seconds of clean data exist, so the chart does not open with a meaningless spike.
- `exp_cumulative` is stitched through level boundaries using the EXP table, so a level-up never looks like a reset to zero.

### 7.1 Idle
- Idle = samples flowing while EXP does not change.
- Idle spans over **2 minutes** are shaded on the chart as idle bands.
- Because the rate is wall-clock, idle drags the rate down and the curve shows a flat plateau followed by a visible climb back. This is intended.

### 7.2 ETA to next level
```
eta = (exp_required(level) − exp_in_level) / rate_5m
```
- Shown as `—` until the current session has a full 5-minute window, whenever the 5-minute average is zero (or rounds to zero), and after 2 minutes without a gain (idle).
- The 1-minute and 30-minute rates remain visible in the expanded panel as context.

---

## 8. Live panel

A frameless, always-on-top window composited by the OS. It is a picture first: **click-through by default**, so every click reaches the game.

Collapsed state (default), roughly 280 × 96 px:

```
┌──────────────────────────────────┐
│ ● Henesys Hunting Ground I   ▁▂▃▅│
│  Lv 71                62.4%      │
│  ████████████████████░░░░░░░     │
│  1.24M/h                 2h 14m  │
└──────────────────────────────────┘
```

- Left dot is the capture state from §5: green reading, yellow degraded, red lost.
- Map name truncates with an ellipsis. It never wraps — wrapping would resize the panel mid-grind.
- Level and percentage use tabular figures so updating digits do not shift the layout.
- Thin EXP bar, because a percentage alone does not read at a glance.
- 5-minute average rate and ETA are the two prominent numbers.
- Character name and job appear under the map once taught (bitmap fingerprints, not OCR).
- Optional 15-minute sparkline, shape only, no axes.
- No buttons in this state.

Expanded state, roughly 360 × 250 px, opens from the same anchor:

```
┌────────────────────────────────────────┐
│ ● Henesys Hunting Ground I        ⋮⋮   │
│  Lv 71  62.4%  ██████████░░░░          │
│                                        │
│  1.24M/h  now      1.18M/h  5m         │
│  ┌──────────────────────────────────┐  │
│  │        ╱╱╱                       │  │
│  │   ╱╱╱╱╱                          │  │
│  │▓▓ idle ▓▓╱╱╱╱                    │  │
│  └──────────────────────────────────┘  │
│  last 30m · 1m, 5m            ETA 2h14m│
└────────────────────────────────────────┘
```

- Chart covers the **last 30 minutes only**. "How am I doing right now" is this panel's job; anything longer belongs in history.
- Cumulative EXP stepped, rate line over it, idle shaded — same visual grammar as the history window.
- Session line: total gained, active vs idle minutes.
- Drag handle appears only while interactable.
- Opacity and "open history" live in the tray menu, not in the panel.

Behavior:

- **Interactivity**: holding the interact hotkey makes the panel clickable and draws a 1px accent border plus a small `interactive` chip, so it is always obvious whether the next click lands on the game or the panel.
- **Position and size persisted**; drag to move.
- **Scaling** respects display scale (no postage stamp on 4K).
- **Never steals focus** — expanding, banner changes and notifications must not pull focus from the game.
- **Degraded** replaces the top line with a yellow strip: `EXP reading unstable — check calibration`.
- **Lost** replaces it red and expands the panel once: `Not reading EXP — game window not found`, plus a `Recalibrate` button and a desktop notification.
- **Idle** shows a muted `idle` pill after 2 minutes without gain; the rate visibly decays (wall-clock model, §7.1).

---

## 9. History window

A normal, resizable, non-overlay window opened by hotkey or tray. It is read-only: the JSONL files remain the source of truth.

Layout, top to bottom:

**1. Range bar** — presets `Today · 7 days · 30 days · All · Custom`, plus a fidelity badge that states the truth: `1s raw` or `1m aggregates`.

**2. Chart, two panes sharing one x-axis**
- Upper: cumulative EXP gained, stepped.
- Lower: rate lines. At raw fidelity, the 1-minute line plus the faint 5-minute average. Beyond 7 days the 1-minute line is derived from one-minute buckets, so it is drawn hollow and labelled coarse rather than presented as equivalent.
- Idle bands shaded (§7.1).
- Markers: level-up, death, map change. At month scale map markers collapse and appear on hover only.
- Crosshair tooltip: timestamp, level, EXP, map, rate, and the ETA that would have applied.

**3. Summary strip** — total EXP gained, time recording, idle time, wall-clock average rate, best 1-minute rate, map set for the selected range.

**Navigation**
- Wheel zooms the x-axis, drag pans, double-click resets, brush selects a span.
- Preset keys `T / 7 / 30 / A`.
- Date separators on multi-day axes.

**Resolution rules**
- ≤ 7 days: raw 1-second samples (fidelity badge `1s raw`).
- > 7 days: one-minute aggregates (`1m aggregates`), with the fidelity badge visible and the coarse rate line styled accordingly.
- Gaps in the data are drawn as gaps. History never interpolates across missing samples.

**Not included**: editing, annotations, bookmarks, or period-to-period comparison. History answers "what happened"; it does not modify anything.

---

## 10. Storage

All state lives under `./data/` in the project directory — nothing outside it.

### 10.1 Samples

Path: `./data/samples/YYYY-MM-DD.jsonl`. One line per second of captured play:

```json
{"t":1762800000,"lv":17,"exp":10105,"pct":91.53,"map":"Henesys","state":"gaining","conf":0.99}
```

- `t` — Unix seconds
- `lv`, `exp`, `pct` — parsed EXP readout
- `map` — snapped map name
- `state` — `gaining` | `idle`
- `conf` — parse confidence, 0–1

`not_recording` is represented by the absence of samples; a gap longer than 5 seconds is interpreted as such. No synthetic rows are written for gaps.

### 10.2 Settings and profiles

Path: `./data/settings.json` — machine-wide state only: the target window match, rate windows (1/5/30), idle threshold.

Path: `./data/profiles/<W>x<H>.json` — everything that is physical-pixel specific, one file per client size: ROI rectangles, the learned glyph atlas, taught map fingerprints, and the character name/job fingerprints. The engine activates the profile matching the live client size.

Recording is gated: a size with no calibrated profile reports **lost** and writes nothing, and samples start only after consecutive good reads, so a wrong profile can never look like "stopped gaining EXP".

### 10.3 Retention
- Raw 1 Hz samples kept for 7 days.
- Beyond that, compact to one-minute aggregates (mean exp, min/max, sum of gain, map set) so months of play stay small.
- No automatic deletion of aggregates.

---

## 11. EXP table

| Range | Status | Source |
|---|---|---|
| Lv 1–49 | confirmed in-game | Classic World wiki |
| Lv 50–89 | **estimated** (~5.5% per level, pre-BB shaped) | extrapolated, labeled as such |
| Lv 90–100 | exact anchors + 2.1× per level | Nexon/launch data |

- Level 30 = 95,700 EXP; total to 90 = 103,322,764; total to 100 = 18,194,778,397.
- Death penalty: 10% of the bar toward the next level (not cumulative).
- Any value marked estimated must be **labeled estimated in the UI** wherever it drives a displayed number (ETA especially).
- The table is an editable local data file. If the player records a real value in-game, it overrides the estimate and the label clears.

---

## 12. Events

Recorded inline and/or derived on read:
- `level_up` — level field increases; cumulative EXP stitches through the table
- `death` — EXP drops without a level change; marked on the chart, excluded from gain
- `map_change` — map field changes; marked on the timeline, curve continues (no session split)
- `calibration` — ROI changes, so segments of the curve can be tied to a configuration

---

## 13. Explicit non-goals for v1

- No automation of any kind, at any point
- No session splitting — the curve is continuous; idle is visible, not hidden
- No memory reading, even read-only
- No damage meter, no auto-pot, no input synthesis
- No cloud sync, accounts, or telemetry

---

## 14. Open decisions

Settled: **Tauri 2 + Rust core + React/uPlot frontend** (see [docs/DECISIONS.md](DECISIONS.md) D3).

Still open:
- Whether the chart's 5-minute faint line is optional or always on (current default: always on)
- Whether an explicit `events.jsonl` is worthwhile, or derived events stay sufficient
- Live panel polish details — exact collapsed width, sparkline presence, map-name line breaking — deferred until capture lands
- Whether taught map fingerprints should be shared/exportable, or stay per-machine
