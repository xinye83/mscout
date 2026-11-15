# Test fixtures — labelled client-frame corpus (D28)

This directory holds the **tracked regression-test corpus** for MScout's capture
readers: lossless crops taken from real, saved Classic World client frames, each
paired with a `meta.json` ground-truth manifest.

Scope is deliberately small and curated — the narrow "test data only" exception
to the shipping boundary described in
[the shipping boundary](../../docs/SHIPPING.md#test-fixtures-shipped-as-test-data-only)
(D28). It contains no runtime atlases, no fonts, no name lists and no player
profiles.

## Purpose

- Give the detection code (minimap state, HUD level/job/name/EXP, map + street
  labels, UI Size selection) an offline, deterministic oracle, so a change that
  breaks a read fails a test instead of silently mis-reporting in the overlay.
- Pin the exact pixels a reader is expected to consume for each covered
  `WxH@mode` pair, at the same crop rects the live pipeline uses.
- Record what the client itself says about the frame — the open Graphics
  settings panel names the resolution and UI Size, so the manifest's
  `clientW`/`clientH`/`uiMode` can be checked against the frame rather than
  trusted from the capture note.

## Directory layout

One directory per saved frame, named by the client size and UI Size:
`<clientW>x<clientH><UiMode::suffix()>` — the same convention as the preset
files, with the suffix `""` for Default, `@ideal` for Ideal and `@filtered` for
Filtered, so Default takes the **bare** name. A pair directory contains:

- `meta.json` — the ground-truth manifest for the pair (schema below).
- `hud.png` — the bottom status-bar strip.
- `minimap.png` — one minimap-state capture, described by `minimapState` and
  `crops.minimap`; **or** one `minimap-<state>.png` per state, each listed in
  the manifest's `minimaps` array with its own `file` and `crop`.
- `settings.png` — the Graphics settings panel, evidence only (see below).

A pair with several minimap states keeps one manifest, one `hud.png` and one
`settings.png`; the shared fields are recorded once and only the minimap image
and its crop vary by state. The manifest — not this README — is the record of
which pairs exist and where each crop sits, so adding a pair is a directory
plus a manifest, never an edit here.

The PNGs are **lossless crops of real client frames**, written with no
resampling, no palette reduction and no lossy encoding. For a pair with several
states, the shared `settings.png` is a crop of the state captured with the panel
open; it is pair-level evidence, not a claim about each state's pixels. Only
tests read this directory; the running application never opens it.

## Multi-state pairs

All minimap variants for one `WxH@mode` must come from the **same in-game map**.
They may be captured at different times, so the HUD values can drift. Only one
`hud.png` is stored, and the manifest's HUD fields describe that image; they are
not re-read per minimap state. The shared `street`/`map` must match the labels
drawn by the expanded and folded variants (the title-bar state draws no label,
so it cannot corroborate them).

## `meta.json` schema

The values below are illustrative; each pair's manifest is authoritative.

```jsonc
{
  "clientW": 1920,          // client width in physical pixels
  "clientH": 1080,          // client height in physical pixels
  "uiMode": "ideal",        // "default" | "ideal" | "filtered" (see ../../docs/PRESETS.md)
  "minimapState": "expanded", // "expanded" | "titlebar" | "folded" | "hidden"

  "name": "Euroblood",      // HUD character name, exact case
  "job": "Page",            // HUD job, exact case
  "level": 35,              // HUD level, integer

  "expText": "102,718[58.96%]", // EXP readout exactly as drawn, label excluded
  "exp": 102718,            // numeric EXP parsed from expText
  "pct": 58.96,             // numeric percent parsed from expText

  "street": "Victoria Road",                     // minimap street line
  "map": "Thicket Around the Beach III",         // minimap map-name line

  "crops": {
    "minimap":  { "x":   19, "y":   19, "w": 226, "h": 183 },
    "hud":      { "x":  560, "y": 1036, "w": 574, "h":  44 },
    "settings": { "x":  763, "y":  283, "w": 395, "h": 515 }
  },

  "settingsDisplay": {
    "resolution": "1920 x 1080", // Resolution control, as displayed (spaces around 'x')
    "uiSize": "Ideal Ratio"      // UI Size control, as displayed
  }
}
```

A pair with several minimap states keeps the shared fields once and replaces
`minimapState` + `crops.minimap` with a `minimaps` array:

```jsonc
{
  // ...the same shared fields (clientW, clientH, uiMode, name, job, level,
  // expText, exp, pct, street, map, settingsDisplay)...
  "crops": {
    "hud":      { "x": 250, "y": 720, "w": 690, "h":  48 },
    "settings": { "x": 350, "y":  50, "w": 430, "h": 620 }
  },
  "minimaps": [
    { "state": "expanded", "file": "minimap-expanded.png", "crop": { "x": 8, "y": 8, "w": 160, "h": 200 } },
    { "state": "titlebar", "file": "minimap-titlebar.png", "crop": { "x": 8, "y": 8, "w": 160, "h": 200 } },
    { "state": "folded",   "file": "minimap-folded.png",   "crop": { "x": 8, "y": 6, "w": 260, "h":  36 } }
  ]
}
```

A manifest has either `minimapState` + `crops.minimap` (one case) or
`minimaps` (several), never both.

Field notes:

- **`crops` are in client coordinates** — the same space as
  `../presets/*.json` boxes and the ROIs stored in a profile (physical client
  pixels, origin = top-left of the game client). They are *not* screen
  coordinates and *not* window coordinates; see
  [`../../docs/DECISIONS.md`](../../docs/DECISIONS.md) D13 for why the client
  origin is the only valid frame of reference.
- Each `crops` entry is the rect the corresponding PNG was cut from, so
  `crops.<id>.w/h` equal that PNG's pixel dimensions by construction. In a
  multi-state manifest, `minimaps[].crop` is the rect for that state's `file`,
  while `crops.hud` and `crops.settings` are shared by all states. Every crop
  covers the whole point of interest plus a buffer; a frame, badge or glyph cut
  off at an edge means the fixture is wrong, not that the crop is tight.
- `minimap` is the widget plus a 4 px margin; `hud` runs from the status-bar left
  border to one pixel before the CASH SHOP widget. Both are intentionally a
  little larger than the field being read, so a reader cannot depend on a
  pixel-perfect origin.
- `settings` is the whole Graphics panel grown by a small margin. The panel is
  drawn at the UI Size's own scale, so its bounds are per pair rather than a
  global constant; the manifest records the rect actually used.
- `expText` is the readout **without** the `EXP` label (no period is drawn
  between the label and the value). The thousands separator is stored as a
  comma and the percentage's decimal as a period; the two glyphs are
  pixel-identical at this size, so the convention is the only way to record it.
- `exp`/`pct` must satisfy `exp / pct` ≈ the character's required EXP for the
  level, to within the rounding of the displayed 2-dp percentage.

## Capture protocol

1. **One directory per `WxH@mode`.** Name it `<clientW>x<clientH><suffix>`, using
   the same suffix as the preset files: bare for Default, `@ideal` for Ideal,
   `@filtered` for Filtered. Never mix two resolutions or two UI Sizes in one
   directory.
   A pair with several minimap states shares one `meta.json`, `hud.png` and
   `settings.png`, and adds one `<state>` minimap PNG per state, listed in the
   manifest's `minimaps` array.
2. **Keep the Graphics settings panel open for the pair's evidence capture.**
   At least one capture per pair has `OPTIONS > Graphics` open over the centre
   of the client. It is harmless to detection (it overlaps neither the minimap
   nor the bottom HUD strip) and it makes the frame self-describing:
   `settings.png` shows the Resolution and UI Size the client is actually using,
   so `clientW`, `clientH` and `uiMode` in `meta.json` can be audited from the
   frame itself. The other minimap-state captures may have it closed; the pair
   root keeps the one `settings.png`.
3. **Capture the client area only.** The frame must start at the client origin
   (no window chrome, no desktop). Verify `clientW`/`clientH` against the
   panel's Resolution label.
4. **Crop in client coordinates** using the rects in `crops`, with no resampling
   and no scaling, and save as PNG (lossless). A crop that has been resized,
   re-encoded as JPEG, or colour-reduced is not a valid fixture — the readers
   threshold and match on exact pixels. Include the **entire** point of interest
   plus a buffer of a few pixels beyond its frame, badge or text; never crop
   flush to the content. A clipped border, badge or glyph is a fixture bug, not
   a tight crop.
5. **Write `meta.json` by reading the frame, not from memory.** Level, name,
   job, EXP readout, street and map must come from the pixels in the saved
   frame. Re-capturing a frame for an existing directory means re-reading every
   field: EXP and level change as the character plays, and a stale manifest is
   worse than no manifest.
6. **Keep the source frame out of the fixture directory.** Fixtures are the
   small labelled crops; a full source frame is kept outside this directory and
   outside version control.
7. **Keep one map per pair.** Capture the expanded, title-bar and folded states
   on the same in-game map. The HUD may change between them; keep one HUD crop
   and record its values. The shared `street`/`map` must agree with both the
   expanded and folded labels.

## `settings.png` is evidence only

`settings.png` exists so the manifest's resolution and UI Size claims are
auditable, and so a reviewer can see the client's own labels next to the HUD
geometry. **Nothing in the application reads it** — detection does not locate,
parse or depend on the settings panel, and no reader, preset, profile or
calibration path touches this file. Treat it as documentation with pixels: if
detection behaviour ever changes, `settings.png` must not be part of the reason.

When one pair has several state cases, the panel is captured once (in the state
with the panel open) and shared at the pair root; it is not re-cropped per
state. Each case's `crops.settings` still names that shared PNG's rect.

The same applies to `settingsDisplay` in `meta.json`: it is a recorded
observation, not an input.

## Using the fixtures in tests

Fixtures are consumed **only by tests**. Load a PNG, run the reader under test
against it, and assert against `meta.json`. Tests must not write into this
directory, and must not require the original BMP: the crops plus the manifest
are the complete input. A full source frame, if kept, is only a convenience for
re-cropping or for measuring a new rect, not a test dependency.

## Regression suite

[`src/regression.rs`](../src/regression.rs) is the committed gate: it walks every
immediate subdirectory below this file, parses its `meta.json`, rebuilds the
frame from the minimap PNG + `hud.png` and runs the real readers over it. New
fixture directories are picked up automatically, so neither the suite nor this
README needs editing when the corpus grows. It never skips a case — a directory
with no manifest, a missing image, a crop outside the client, or a manifest that
disagrees with its pixels fails the run with the case name in the message.

Both tiers run from this directory's parent (`src-tauri`).

### Hermetic tier — every machine, no game install

```sh
cargo test --lib
```

Runs `fixture_corpus_is_complete` (corpus present; required files; every crop in
bounds and matching its PNG's size; `settingsDisplay.resolution`/`uiSize`
agreeing with `clientW`/`clientH`/`uiMode`; `settings.png` not blank),
`minimap_state_matches_ground_truth` (`ocr::detect_minimap` names the captured
state) and `settings_panel_is_evidence_only` (the settings crop is never composed
into the frame, so no reader can see it). These need the repository only — the
PNG decoder comes from the test-only `image` dev-dependency.

### Client-dependent tier — needs MSCW + Windows Arial

```sh
cargo test --lib regression -- --ignored --nocapture
```

Runs `reads_every_fixture_from_empty_atlases` and
`reads_follow_the_manifest_minimap_state`: for each case the preset for its
`WxH@mode` must exist, the EXP/level/name atlases are seeded from the frame
itself (no saved profile is read or written), and `engine::read_frame` must
reproduce `expText`/`exp`/`pct`/`level`/`name`/`job`/`street`+`map` exactly. The
readings are printed, so a failure shows what the reader actually saw. The
minimap state from `meta.json` decides the expected map form: `expanded` and
`folded` give `"<street>\n<map>"`, `titlebar` reports the map as hidden, `hidden`
has no widget to read.

Set `MSCOUT_CASE=<pair>` to restrict either tier to one fixture (or all state
cases of a pair), and `MSCOUT_STATE=<expanded|titlebar|folded>` to restrict it
further to one minimap state. That is how a pair with a preset is checked while
other pairs are still uncalibrated; for example
`MSCOUT_CASE=1366x768@ideal cargo test --lib regression -- --ignored`.

Add `--offline` to either command to prove the build needs no network. The MSCW
folder is taken from `MSCOUT_MSCW_DIR`, else from the app's saved MSCW setting
(read-only); nothing in this tier writes to the app's profile or settings store,
or to this directory. (The reader may append its normal debug log to the app's
data directory, exactly as it does at runtime.)
