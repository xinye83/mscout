# MScout — behaviour spec

What MScout must and must not do: the rules a change is checked against. It is
not a design doc or a status report. How the readers work is in
[TEXT_RENDERING.md](TEXT_RENDERING.md) and [DECISIONS.md](DECISIONS.md), the
file formats in [DATA.md](DATA.md), supported sizes in [PRESETS.md](PRESETS.md),
open work in [ROADMAP.md](ROADMAP.md).

MScout is a local, read-only overlay for **MapleStory Classic World** — not
original MapleStory or a private server. It reads the HUD, records EXP samples
and shows how fast EXP is gained. EXP tables, map lists and the level cap come
from Classic World data.

---

## Contents

1. [Hard constraints](#1-hard-constraints)
2. [Scope](#2-scope)
3. [Capture](#3-capture)
4. [Reading](#4-reading)
5. [States](#5-states)
6. [Data integrity](#6-data-integrity)
7. [Rate math](#7-rate-math)
8. [Live panel](#8-live-panel)
9. [History window](#9-history-window)
10. [EXP table](#10-exp-table)
11. [Events](#11-events)
12. [Non-goals](#12-non-goals)
13. [Known limitations](#13-known-limitations)

---

## 1. Hard constraints

The app must never touch the game process. That is the design boundary, not a
preference.

**Allowed**
- Windows Graphics Capture of the game window (an OS compositor API)
- Cropping, image processing, glyph matching, OCR against the client's own name tables
- A separate always-on-top window composited by Windows
- Global hotkeys via `RegisterHotKey` (no low-level keyboard hooks)
- Local files under `data/`

**Forbidden — no exceptions**
- `ReadProcessMemory` / `WriteProcessMemory`, handles into the game process
- DLL injection, D3D/API hooking, packet capture or manipulation
- `SendInput` / `keybd_event` / `PostMessage` to the game window — no auto-pot,
  auto-attack, macros, auto-relog
- Any pixel → in-game action loop
- Process renaming, packing, obfuscation, or anti-cheat evasion
- Multi-client support
- Shipping datamined client assets (stats and numbers are fine; art is not —
  see [SHIPPING.md](SHIPPING.md))

**Required player setting:** the game runs windowed or borderless. Exclusive
fullscreen cannot be captured or overlaid without hooking, which is forbidden.

Per Nexon's rules, botting is playing the game repetitively with a macro,
third-party program or physical method. MScout reads and displays; it never
plays. Every future feature must keep that true.

---

## 2. Scope

- Read the HUD: EXP, level, map and street, character name and job, at the
  supported client sizes and UI Sizes.
- Record samples locally, one per second of play.
- Live panel: map, level, EXP %, rates, ETA, a short chart.
- History window: browse and chart the stored samples.

Damage/AP/potion calculators, per-map comparison and anything that plays the
game are out of scope (§12).

---

## 3. Capture

- The player picks the game window once; the choice persists by window title
  and process. A window that cannot be found is reported at once, never waited
  on silently.
- One frame per second. Capture continues while the game is unfocused or
  covered. Minimised, closed or capture-denied windows are reported states
  (§5), not silent gaps.
- **Every frame is read as the game's 1x picture** (`native.rs`). Up to 768
  lines the client draws at 1x in every UI Size. Above that, Default Ratio
  stretches a 768-line picture bilinearly (undone by least squares, exactly),
  Ideal Ratio draws whole-pixel blocks (1x to 1200 lines, 2x at 1440), and
  Filter Applied sharpens the stretch (undone closely, not exactly; at sizes
  with a measured preset it is read on the client frame instead).
- **The UI Size is detected from the frame** above 768 lines: how well the
  Default stretch explains the HUD band separates the three. The setting only
  applies up to 768 lines, where every UI Size draws the same pixels.
- Read boxes are pixels of that picture, kept per **picture size × UI Size** in
  a profile; a resolution change switches profiles. A size with no shipped
  preset gets the 1366×768 boxes moved with the HUD bar, which sits centred on
  the bottom edge at every size.
- The EXP box is required; level, name/job and the map label are optional.
  Without a level box the level is reported unrecognised, never derived.
- Digit atlases are read from the player's installed client; nothing from the
  client is shipped.

---

## 4. Reading

- **Digits are matched against the client's own glyphs, not general OCR.** The
  HUD draws them from baked bitmaps.
- **The EXP readout is cross-checked:** `percentage ≈ exp / exp_required(level)`.
  A small disagreement (up to 1 percentage point) is a misread percentage and is
  recomputed from EXP and level; a larger one invalidates EXP and level for that
  frame. Map, name and job are kept either way.
- **The level is read from its own box**, never derived from EXP and percentage.
- **A shape that cannot be told apart from another is refused, not guessed.**
- **Name and job** are read as Arial text; a taught fingerprint is the exact
  fallback and the way to correct a misread. The job must be a known job name.
- **The map label is located, not boxed:** the minimap is found on every frame,
  and the name is matched against the client's own map table. A name not in the
  table is shown as read; a label that cannot be matched confidently is
  `unknown`.

---

## 5. States

Always visible in the overlay:

| State | Meaning | Overlay |
|---|---|---|
| **Reading** | The last frames read cleanly | normal |
| **Degraded** | ≥ 3 failed reads in the last 10, or the last read was refused with a reason | yellow, with the reason |
| **Lost** | Window gone, minimised, or capture denied | red, naming the cause |
| **Not calibrated** | The live client size and UI Size have no usable profile | names the size, with a calibrate action |

No desktop notification for a lost capture: the banner is the signal. A
notification is only used for a startup problem the overlay cannot show (a
hotkey another app holds).

---

## 6. Data integrity

- **Never write a zero or a guess.** A failed or refused read produces no
  sample, so a broken capture shows as missing data, never as "stopped gaining
  EXP".
- **Rows are written only after consecutive good reads**, and only when the
  frame has a recognised level.
- **Levels never go down for one character.** A lower level is a misread, or a
  different character.
- **EXP never drops within a level, except at a death.** A drop is held until
  it persists; a confirmed drop of at least 1% of the level is a death,
  recorded as an event on the row (§11). A short blip is rejected.
- **Rows carry the character name** when it was read; a different character
  starts a new run, so rates and charts never compare two characters' totals.
- **Recording states are distinguishable:** EXP rising (`gaining`), samples with
  EXP unchanged (`idle`), no samples at all (not recording). Gaps are never
  filled with synthetic rows.
- **History files are only appended to**, except by the explicit repair, which
  flags misreads (`"bad": true`) instead of deleting them.

---

## 7. Rate math

All rates use **wall-clock time** and **trailing differences** over recorded
rows; windows are view parameters, so changing one needs no re-recording.

```
rate_per_hour(t, W) = (exp_cumulative(t) − exp_cumulative(t − W)) / W_hours
```

- `exp_cumulative` is stitched through level boundaries with the EXP table, so
  a level-up never looks like a reset to zero.
- EXP lost at a recorded death is added back: the rate is EXP earned per hour,
  and the time spent re-earning it stays in the window.
- Windows: **1 minute** (responsive, jumpy by design), **5 minutes** (the
  prominent rate and the ETA), **30 minutes** (context).
- A window's rate is shown only once the session has lived through the whole
  window; before that it is `—`.
- **Idle** is samples flowing with EXP unchanged. Because rates are wall-clock,
  idle visibly drags them down. This is intended; idle is shown, never hidden
  by splitting the session.
- **ETA** = `(exp_required(level) − exp_in_level) / rate_5m`, shown as `—` when
  the 5-minute rate is missing or zero and after 2 minutes without a gain.
- The overlay computes rates only from rows the history recorded, so overlay
  and history agree.

---

## 8. Live panel

A frameless, always-on-top window composited by Windows. **Click-through by
default**, so every click reaches the game; it becomes clickable while the
cursor is over it, or by hotkey.

- Collapsed: state dot, map (wrapped, never cut), name and job, level and
  percentage, an EXP bar, the 5-minute rate and the ETA.
- Expanded: the 1/5/30-minute rates and a chart of recent EXP.
- Tabular figures, so changing digits do not shift the layout.
- Placed outside the game window at its top-right corner; a dragged position
  is remembered until the player resets it.
- **Never steals focus.** Expanding, state changes and notifications must not
  pull focus from the game.
- Respects display scaling.

---

## 9. History window

A normal window, opened by hotkey or tray. **Read-only:** the day files are the
source of truth, and the window never edits them (the repair button aside, §6).

- Gaps in the data are drawn as gaps; history never interpolates across
  missing samples.
- Long ranges may be thinned when read, but deaths and character switches are
  always kept.
- Rate lines, markers, zoom and custom ranges are open work in the roadmap.

---

## 10. EXP table

The requirement per level is one file, `src-tauri/exp-table.json`, read by the
engine, the UI and the mock-data script. Totals: 103,322,764 EXP to reach 90,
18,194,778,397 to reach 100.

- Lv 1–49: confirmed in play. Lv 90–99: launch data.
- **Lv 50–89 are planning numbers, not yet confirmed in Classic World.** They
  are labelled estimated wherever they drive a displayed number (the ETA
  especially), and the cross-check allows them a wider tolerance.

---

## 11. Events

- `death` — recorded on the first row after a confirmed death (§6).
- Level-ups are derived from the level field, not recorded.
- Map changes are part of every row; the curve continues across them.

---

## 12. Non-goals

- No automation of any kind, at any point.
- No memory reading, even read-only.
- No session splitting: the curve is continuous; idle is visible, not hidden.
- No damage meter, no auto-pot, no input synthesis.
- No cloud sync, accounts or telemetry.

---

## 13. Known limitations

What the current version does, not a task list. Work that addresses one is an
item in [ROADMAP.md](ROADMAP.md).

- **Support matrix:** every corpus size reads: 1366×768, 1920×1080, 1920×1200
  and 2560×1440 in all three UI Sizes. Other sizes use derived boxes and the
  Ideal block size `height / 720`, which is measured only at 1080, 1200 and
  1440 lines.
- **Filter Applied is approximate above 768 lines.** Its sharpen is not fully
  linear and its alignment drifts with the size, so the recovered picture is
  close, not exact; at 1920×1200 the name/job are not read.
- **Profiles are per picture size.** A transient client size can create an
  empty profile (e.g. `1920x1081@ideal`); move it aside if the app selects it.
- **The EXP box is sized for the current digit count.** A much longer EXP number
  can clip it until it is redrawn.
- **Filtered atlas gaps.** A live `8` can read as `6`/`9` (a 20-EXP error that
  can pass the rounding check), and a percentage `3` can read as `8` (the
  percentage is then repaired from EXP and level).
- **Map matching depends on the client table**, whose size is version-specific.
  A one-line label has no street; an ambiguous `<N>` suffix stays `unknown`;
  without exact OCR the whole-line fallback can confuse Roman-numeral siblings.
- **Windows dependencies:** Arial for name/job and label rendering,
  `Windows.Media.Ocr` for the Default/Filtered name/job and map label. Neither is
  bundled.
- **History ranges are presets only** (1, 7 and 30 days).
- **`window-error.log`** is still written when history-window creation fails.
- **Dev build expects Vite.** A direct `cargo run` needs `npm run dev` running.
