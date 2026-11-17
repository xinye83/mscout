# Test fixtures — labelled client-frame corpus (D28)

This directory holds the **tracked regression-test corpus** for MScout's capture
readers: lossless crops taken from real, saved Classic World client frames. Every
`WxH@mode` directory is self-contained: one `meta.json` indexes every PNG in the
directory with its crop and ground truth. There are no loose focused fixtures.

Scope is deliberately small and curated — the narrow "test data only" exception
to the shipping boundary described in
[the shipping boundary](../../docs/SHIPPING.md#test-fixtures-shipped-as-test-data-only)
(D28). It contains no runtime atlases, no fonts, no name lists and no player
profiles.

## Purpose

- Give the detection code (minimap state, HUD level/job/name/EXP, map + street
  labels, UI Size ground truth) an offline, deterministic oracle, so a change
  that breaks a read fails a test instead of silently mis-reporting in the
  overlay.
- Pin the exact pixels a reader is expected to consume for each covered
  `WxH@mode` pair, at the same crop rects the live pipeline uses.
- Record what the client itself says about the frame — the open Graphics
  settings panel names the resolution and UI Size, so the manifest's
  `clientW`/`clientH`/`uiMode` can be checked against the frame rather than
  trusted from the capture note.

## Directory layout

One directory per `WxH@mode`, named `<clientW>x<clientH><UiMode::suffix()>` — the
same convention as the preset files, with the suffix `""` for Default, `@ideal`
for Ideal and `@filtered` for Filtered, so Default takes the **bare** name. A
pair directory contains:

- `meta.json` — the manifest that indexes every image and carries the shared
  frame ground truth (schema below).
- `hud-main.png` — the bottom status-bar strip used to compose frame cases.
- `settings-panel.png` — the Graphics settings panel, evidence only.
- `minimap-<state>.png` — one image per minimap state; every image is listed as
  a `minimap` entry with its own `state` and crop.
- Optional focused images, all indexed as `exp` or `map-label` entries:
  `hud-low-percent.png`, `exp-<value>.png`, `minimap-tree-1.png`,
  `minimap-deep-valley.png`.

No PNG may sit in a pair directory without an image entry. The manifest — not
this README — is the record of which pairs exist, which files they contain and
where each crop sits. Adding a pair or a focused fixture is a manifest edit plus
the PNG.

The PNGs are **lossless crops of real client frames**, written with no
resampling, no palette reduction and no lossy encoding. The shared
`settings-panel.png` is pair-level evidence; it is not a claim about each state's
pixels. Only tests read this directory; the running application never opens it.
Client-tier tests require a shipped preset for the pair; every 1366×768 and
1920×1080 UI Size now has one. 1920×1200 has no fixture or preset.

## `meta.json` schema (version 2)

The values below are illustrative; each pair's manifest is authoritative.
`version` must be `2`.

```jsonc
{
  "version": 2,
  "clientW": 1920,          // client width in physical pixels
  "clientH": 1080,          // client height in physical pixels
  "uiMode": "filtered",     // "default" | "ideal" | "filtered"

  "name": "Euroblood",      // HUD character name, exact case
  "job": "Page",            // HUD job, exact case
  "level": 35,              // HUD level, integer

  "expText": "127,028[72.91%]", // HUD EXP readout exactly as drawn, label excluded
  "exp": 127028,            // numeric EXP parsed from expText
  "pct": 72.91,             // numeric percent parsed from expText

  "street": "Hidden Street",                    // minimap street line
  "map": "Free Market <8>",                     // minimap map-name line

  "settingsDisplay": {
    "resolution": "1920 x 1080", // Resolution control, as displayed (spaces around 'x')
    "uiSize": "Default Ratio (Filter Applied)" // UI Size control, as displayed
  },

  "images": [
    {
      "id": "hud-main",       // stable lookup id used by tests
      "kind": "hud",          // hud | settings | minimap | exp | map-label
      "file": "hud-main.png",
      "source": "client",     // client | standalone
      "crop": { "x": 398, "y": 1021, "w": 810, "h": 59 }
    },
    {
      "id": "minimap-expanded",
      "kind": "minimap",
      "state": "expanded",    // expanded | titlebar | folded | hidden
      "holdout": true,        // optional: the one frozen hold-out state (see below)
      "file": "minimap-expanded.png",
      "source": "client",
      "crop": { "x": 14, "y": 12, "w": 230, "h": 260 }
    },
    {
      "id": "hud-low-percent",
      "kind": "exp",
      "file": "hud-low-percent.png",
      "source": "standalone",
      "crop": { "x": 0, "y": 0, "w": 810, "h": 59 },
      "readCrop": { "x": 650, "y": 15, "w": 155, "h": 18 },
      "expText": "2,056[1.06%]",
      "exp": 2056,
      "pct": 1.06
    },
    {
      "id": "map-tree-1",
      "kind": "map-label",
      "file": "minimap-tree-1.png",
      "source": "standalone",
      "crop": { "x": 0, "y": 0, "w": 260, "h": 290 },
      "street": "Victoria Road",
      "map": "The Tree That Grew I"
    }
  ]
}
```

Field notes:

- Every PNG in the directory must have exactly one image entry; ids and files
  must be unique. The hermetic test fails on an unindexed PNG, a missing file, a
  duplicate id/file, or a crop whose size disagrees with the PNG.
- **`source: "client"`** images are pasted into a composed `clientW` x `clientH`
  frame at their `crop`. The crop is in client coordinates — the same space as
  `../presets/*.json` boxes and profile ROIs (physical client pixels, origin =
  top-left of the game client). It is *not* screen or window coordinates; see
  [`../../docs/DECISIONS.md`](../../docs/DECISIONS.md) D13.
- **`source: "standalone"`** images are complete fixtures in their own right.
  Their `crop` must be the whole image (`0,0,w,h`); `readCrop`, when present and
  used by an `exp` entry, names the region the reader consumes. `expText`,
  `exp` and `pct` are required for an `exp` entry. `map-label` entries require
  `street` and `map`.
- A pair has exactly one `hud` image, exactly one `settings` image, and at least
  one `minimap` image. The shared top-level HUD/EXP/street/map fields describe
  the `hud-main.png` frame; minimap images use their own `state`.
- `settingsDisplay.resolution`/`uiSize` must agree with `clientW`/`clientH` and
  `uiMode`. The settings image must not be a flat colour.
- `exp`/`pct` must satisfy `exp / pct` ≈ the character's required EXP for the
  level, to within the rounding of the displayed 2-dp percentage.
- **`holdout`** (default `false`) marks the one frozen hold-out state; see
  [Frozen hold-out](#frozen-hold-out). It is only meaningful on a `minimap`
  entry: a `holdout` flag on a `hud`/`settings`/`exp`/`map-label` entry is
  rejected when the corpus loads, because the flag would otherwise be silently
  dropped and the manifest would look reserved while nothing was.
  `exactly_one_frozen_holdout` (hermetic tier) fails unless exactly one minimap
  image in the whole corpus sets it, on the pinned case.

## Multi-state pairs

All minimap variants for one `WxH@mode` must come from the **same in-game map**.
They may be captured at different times, so the HUD values can drift. Only one
`hud-main.png` is stored, and the shared HUD fields describe that image; they are
not re-read per minimap state. The shared `street`/`map` must match the labels
drawn by the expanded and folded variants (the title-bar state draws no label,
so it cannot corroborate them).

## Frozen hold-out

One minimap state is the **frozen hold-out**, marked `"holdout": true` in its
image entry: **`1920x1080@ideal`, `expanded`** (case key
`1920x1080@ideal/expanded`), frozen on **2026-11-16**. Exactly one state may carry
the flag — `exactly_one_frozen_holdout` (hermetic tier) fails on zero or on two,
and fails if the flagged case is not that key. The key is pinned in the test, so
moving the flag to a case that already passes does not move the reservation; it
breaks the guard.

The hold-out is not read differently; the readers run over it like every other
case and `accuracy_report` (below) publishes its result. What it is *not* is an
input to tuning. Do not:

- move a box, threshold, atlas, matcher or map table because the hold-out reads
  wrong, or add glyph variants/fallbacks until it passes;
- re-crop, re-label or replace the hold-out frame when the reader changes;
- move the flag to a case that already passes, or drop it because it is
  inconvenient.

If the hold-out reads wrong, the report prints it as wrong and the entry stays in
**Known limitations** ([`../../docs/ROADMAP.md`](../../docs/ROADMAP.md)). Work on
the other cases first; if the hold-out result changes, that is a finding to
report, not a number to tune away.

**It was frozen retrospectively.** Every frame here was captured and labelled
during the 0.5 → 0.6 work, and all of them — this one included — were looked at
while the Filtered EXP, level and identity readers were tuned. The flag therefore
reserves the case *from now on*; it does not make it historically independent and
it must not be reported as an unbiased accuracy estimate. The published counts,
the command that produced them and this caveat are in
[`../../docs/ACCURACY.md`](../../docs/ACCURACY.md).

## Capture protocol

1. **One directory per `WxH@mode`.** Name it `<clientW>x<clientH><suffix>`, using
   the same suffix as the preset files: bare for Default, `@ideal` for Ideal,
   `@filtered` for Filtered. Never mix two resolutions or two UI Sizes in one
   directory. Add one `minimap` image entry per state to the same manifest.
2. **Keep the Graphics settings panel open for the pair's evidence capture.**
   At least one capture per pair has `OPTIONS > Graphics` open over the centre
   of the client. It is harmless to detection (it overlaps neither the minimap
   nor the bottom HUD strip) and it makes the frame self-describing:
   `settings-panel.png` shows the Resolution and UI Size the client is actually
   using. The other state captures may have it closed.
3. **Capture the client area only.** The frame must start at the client origin
   (no window chrome, no desktop). Verify `clientW`/`clientH` against the
   panel's Resolution label.
4. **Crop in client coordinates** using the rects in the image entries, with no
   resampling and no scaling, and save as PNG (lossless). A crop that has been
   resized, re-encoded as JPEG, or colour-reduced is not a valid fixture — the
   readers threshold and match on exact pixels. Include the **entire** point of
   interest plus a buffer of a few pixels beyond its frame, badge or text; never
   crop flush to the content. A clipped border, badge or glyph is a fixture bug,
   not a tight crop.
5. **Write `meta.json` by reading the frame, not from memory.** Level, name,
   job, EXP readout, street and map must come from the pixels in the saved
   frame. Re-capturing a frame for an existing directory means re-reading every
   field: EXP and level change as the character plays, and a stale manifest is
   worse than no manifest.
6. **Index every PNG.** A focused EXP crop is an `exp` entry with its ground
   truth; a standalone map label is a `map-label` entry. Unindexed files fail
   the hermetic tier.
7. **Keep the source frame out of the fixture directory.** Fixtures are the
   small labelled crops; a full source frame is kept outside this directory and
   outside version control.
8. **Keep one map per pair.** Capture the expanded, title-bar and folded states
   on the same in-game map. The HUD may change between them; keep one HUD crop
   and record its values. The shared `street`/`map` must agree with both the
   expanded and folded labels.

## `settings-panel.png` is evidence only

`settings-panel.png` exists so the manifest's resolution and UI Size claims are
auditable, and so a reviewer can see the client's own labels next to the HUD
geometry. **Nothing in the application reads it** — detection does not locate,
parse or depend on the settings panel, and no reader, preset, profile or
calibration path touches this file. Treat it as documentation with pixels: if
detection behaviour ever changes, `settings-panel.png` must not be part of the
reason.

The same applies to `settingsDisplay` in `meta.json`: it is a recorded
observation, not an input.

## Using the fixtures in tests

Fixtures are consumed **only by tests**. Resolve an image by its manifest id
(`crate::regression::fixture_image(pair, id)` or `fixture_frame`), not by a
hardcoded filename. Load the PNG, run the reader under test against it, and
assert against the manifest. Tests must not write into this directory, and must
not require the original BMP: the crops plus the manifest are the complete
input. A full source frame, if kept, is only a convenience for re-cropping or
for measuring a new rect, not a test dependency.

## Regression suite

[`src/regression.rs`](../src/regression.rs) is the committed gate: it walks every
immediate subdirectory below this file, parses its `meta.json`, validates every
indexed image, composes a frame from `hud-main.png` + one minimap image and runs
the real readers over it. New pair directories are picked up automatically. It
never skips a case — a directory with no manifest, a missing image, a crop
outside the client, an unindexed PNG, or a manifest that disagrees with its
pixels fails the run with the case name in the message.

Both tiers run from this directory's parent (`src-tauri`).

### Hermetic tier — every machine, no game install

```sh
cargo test --lib
```

Runs `fixture_corpus_is_complete` (every image indexed; required files; every
crop in bounds and matching its PNG's size; `standalone` crops equal the whole
image; `settingsDisplay.resolution`/`uiSize` agreeing with
`clientW`/`clientH`/`uiMode`; `settings-panel.png` not blank),
`exactly_one_frozen_holdout` (exactly one state carries `holdout` and it is the
pinned `1920x1080@ideal/expanded`, checked on the unfiltered corpus so
`MSCOUT_CASE` cannot narrow it away),
`minimap_state_matches_ground_truth` (`ocr::detect_minimap` names the captured
state) and `settings_panel_is_evidence_only` (the settings crop is never composed
into the frame, so no reader can see it). These need the repository only — the
PNG decoder comes from the test-only `image` dev-dependency.

### Client-dependent tier — needs MSCW + Windows Arial

```sh
cargo test --lib regression -- --ignored --nocapture
```

Runs `reads_every_fixture_from_empty_atlases` and
`reads_follow_the_manifest_minimap_state`: for each frame case the preset for its
`WxH@mode` must exist, the EXP/level/name atlases are seeded from the frame
itself (no saved profile is read or written), and `engine::read_frame` must
reproduce `expText`/`exp`/`pct`/`level`/`name`/`job`/`street`+`map` exactly. The
readings are printed, so a failure shows what the reader actually saw. The
minimap state from the image entry decides the expected map form: `expanded` and
`folded` give `"<street>\n<map>"`, `titlebar` reports the map as hidden, `hidden`
has no widget to read. Focused `exp`/`map-label` entries are covered by their
targeted tests; the manifest still validates them in the hermetic tier.

The same tier also carries the accuracy report:

- `accuracy_report` scores every case `exact` / `rejected` / `wrong` against its
  manifest, prints the per-pair counts and names the frozen hold-out. It asserts
  nothing about the counts, so a known wrong stays a printed wrong instead of
  becoming a pass. Its measured counts, the command that produced them and the
  corpus limits are in [`../../docs/ACCURACY.md`](../../docs/ACCURACY.md).
- The two gate tests above run the hold-out case like any other; nothing skips it
  because it is the hold-out.

**Map-label prerequisite.** An expanded/folded map name is matched against the
client table reached through the app's saved MSCW setting
(`data/settings.json` → `mscwDir`), which `MSCOUT_MSCW_DIR` does **not** set. In a
fresh worktree with no `data/settings.json`, every expanded/folded case reports
the map as `unknown` and the gate fails on the map assertion — that is the
environment, not the manifest; see
[`../../docs/ACCURACY.md`](../../docs/ACCURACY.md#corpus-limits).

Set `MSCOUT_CASE=<pair>` to restrict either tier to one pair (or
`MSCOUT_CASE=<pair>/<state>` for one state), and `MSCOUT_STATE=<expanded|titlebar|folded>`
to restrict further. That is how a pair with a preset is checked while other
pairs are still uncalibrated; for example
`MSCOUT_CASE=1366x768@ideal cargo test --lib regression -- --ignored`.

Add `--offline` to either command to prove the build needs no network. The MSCW
folder is taken from `MSCOUT_MSCW_DIR`, else from the app's saved MSCW setting
(read-only); nothing in this tier writes to the app's profile or settings store,
or to this directory. (The reader may append its normal debug log to the app's
data directory, exactly as it does at runtime.)
