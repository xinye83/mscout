# Handoff — mscout state as of 2026-11-13

Read this before touching the OCR path. Most of a long session went into one defect
that hid behind three convincing-looking wrong answers, and the notes on what *not*
to do are as important as the fix.

## What this is

Tauri + React desktop overlay that reads a live MapleStory Classic World client by
screen OCR: level, EXP, EXP %, character name, job, current map. No per-user
calibration beyond drawing four boxes per (resolution × UI-size mode).

- Rust backend: `src-tauri/src/` — `ocr.rs` (matching), `engine.rs` (tick loop,
  validation, history), `calibration.rs` (commands), `profile.rs` (per-size profiles),
  `preset.rs` (shipped defaults), `settings.rs`.
- Frontend: `src/` — `Calibration.tsx` (the calibration window), `App.tsx` (overlay).
- Data: `data/` — `profiles/`, `samples/` (history jsonl), `review/`, `debug.log`.
- Shipped presets: `src-tauri/presets/`.

## Commands

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib     # 24 tests; 19 are ignored diagnostics
npm run typecheck
# dev, with the calibration window open:
cmd /c "set MSCOUT_OPEN_CALIBRATION=1&& npm run tauri dev"
```

Restart the app after backend changes. The engine caches the active profile in
memory (`profile::active`), so edits made on disk are not necessarily live.

## THE defect (fixed — understand this before changing `normalize`)

A glyph was encoded by sampling a **binary foreground mask** built from a
crop-wide Otsu threshold:

```rust
if mask[sy * stride + sx] { cell[...] = 255; }
```

The level readout is drawn on **pale tiles**. The tile is foreground, so a whole
11x13 tile became a solid block of 255 and the digit inside was discarded *at learn
time*. Consequence: `3` and `5` encoded identically, `distance = 0.0` for both, and
the level read `33` forever — no matter how often it was taught, and with a
round-trip check that happily passed because replaying the same alignment is
self-consistent. The EXP was unaffected (dark background), which is why this hid.

Fixed in `ocr.rs::normalize`:

- default: the crop-wide mask, as before;
- fallback: threshold against the shape's **own** background (modal intensity),
  applied **only when the mask saturates** (>= 85% of the shape reads as ink) —
  i.e. only where the crop-wide threshold has demonstrably failed.

**Do not apply the fallback everywhere.** That was tried and it was worse: a
low-contrast shape has a tiny span, most pixels clear the threshold, the cell
saturates into a blob, and one learned glyph then matches every other shape
("labelled one shape as 7 and now everything is 7").

Verified with an empty seed atlas:
`level box round-trip = Ok(("35", 1.0))` and `EXP round-trip = Ok(("74,490[42.76%]", 1.0))`.

### Cells are stored, so the encoder is a data format

`Atlas::from_settings` reads `stored.cell`. Every learned atlas holds encoded cells,
so **any change to `normalize` invalidates every profile and preset**. There is no
migration — the old cells discarded the ink. Clear and re-learn.

## Current state

All learned data has been **deliberately reset** so it can be rebuilt under the new
encoder:

- All 14 files (`data/profiles/*.json` + `src-tauri/presets/*.json`) have
  `atlas`, `levelAtlas`, `mapAtlas`, `maps`, `identities` empty.
- Calibration **boxes are kept** (geometry, not knowledge). The level box matters:
  `1920x1200@ideal` has `598,1167 35x29`; 1366 default/filtered also have level boxes.
- Backups: `data/backup-pre-reset/`, `data/backup-before-scratch/`.
- `data/samples/` (3 days of EXP history) untouched.

**One test is red on purpose:**
`preset::tests::known_size_ships_with_boxes_and_glyphs` asserts the shipped presets
carry a glyph set, because a fresh install must read without being taught. It fails
only because the reset emptied the presets. Do not weaken it — satisfy it by
re-learning, then writing the preset glyph sets.

## Remaining work, in order

1. **Frontend shape list (backend already done).** `box_glyphs(serial, kind)` exists
   for `exp | level`, and `label_glyph` takes `kind` and writes to the matching atlas
   (`atlas` vs `level_atlas`). What is missing: a shared `ShapeList kind="exp|level"`
   component, placed in the **EXP digits** and **Level digits** cards, with the
   standalone "Unknown shapes" section removed. `Calibration.tsx::labelGlyph`
   currently passes `kind: "exp"` so nothing is broken in the meantime.
2. **Re-learn every set** — 9 combinations (3 resolutions x 3 UI modes). Per set:
   EXP digits, level digits, name/job, map. Watch `data/debug.log`.
3. **Write the learned glyph sets into the presets**, which makes the red test pass.
4. **Re-release v0.3.0.** `package.json` / `Cargo.toml` / `tauri.conf.json` already
   read 0.3.0; git HEAD is `f7bceba` (0.2.0) with the changes uncommitted.

## Architecture notes that matter

- **Separate atlases per readout**: `atlas` (EXP), `level_atlas`, `map_atlas`. They
  were once shared, which let the two readers compete for the same characters.
  `learn_atlas` / `learn_level_atlas` / `box_glyphs` all respect this.
- **Map fingerprints prefer the drawn box** over detection (`ocr::map_label`).
  Detection guesses a boundary from the minimap panel and clips the ends of the
  name, which inflates distances. `MAP_DISTANCE_LIMIT` is 12 (was 24, which let
  `1st Accompaniment <4th stage>` match the taught `<Bonus>` at 23.12).
- **The level is never derived** from EXP and the percentage. It is read from its own
  box, or reported as `0` = unrecognised. A derived level is only as right as the
  EXP table and puts plausible-but-false numbers on screen.
- **A level/percentage mismatch no longer rejects the frame.** EXP and its percentage
  come from one read of one box, so a mismatch is evidence against the *level* — the
  frame is kept and only the level is dropped. Rejecting the whole frame is what made
  every successful teach look like nothing had happened.
- **The overlay never changes shape for a failed read.** Values fall back to
  placeholders (`unknown`, `Lv unknown`, `0.00%`, `—`); there is no early return.
- One status line, not one per problem: problems are joined with ` · ` and share a
  single trailing instruction.

## Diagnostics (ignored tests, run with `-- --ignored --nocapture`)

| Test | Purpose |
|---|---|
| `live_settings_read` | reads the live game with saved settings; prints each map's distance |
| `explains_level_box` | per-shape `best 'x' d=…` for the level box — the tool that found the defect |
| `dumps_atlas_counts` | glyph counts of each atlas in the saved profile |
| `dumps_exp_crop` | writes a crop of `MSCOUT_CROP` from a fresh frame |
| `dumps_map_label` | magnified minimap panel + label crops |
| `dumps_overlay` | captures a window to a BMP (`MSCOUT_DUMP_TITLE/PROCESS/FILE`) |
| `build_atlas` | learns `MSCOUT_LEARN_TEXT` from `MSCOUT_FRAME` + `MSCOUT_CROP` into `data/atlas-build.json` |
| `detect_minimap`, `map_text_round_trip`, `probes_status_bar`, `teach_map_from_bmp`, `record_live_map`, `record_live_labels`, `repair_history_now` | as named |

Env vars: `MSCOUT_CROP="x,y,w,h"`, `MSCOUT_LEARN_TEXT`, `MSCOUT_FRAME`,
`MSCOUT_PROFILE_KEY`, `MSCOUT_UI_MODE`, `MSCOUT_MAP_NAME`, `MSCOUT_DUMP_*`.

Verify with images rather than assertions where you can: `dumps_exp_crop` +
`dumps_overlay` + a vision pass caught two of the three wrong theories in this session.

## Hard-won geometry

| Size / mode | EXP box | Level box | Notes |
|---|---|---|---|
| 1366x768 default | `747,737,97x10` | `330,746,21x9` | |
| 1366x768 filtered | same as default | same | |
| 1366x768 ideal | `750,733,84x16` | — | |
| 1920x1200 default | `1063,1148,152x22` | — | |
| 1920x1200 filtered | same as default | — | |
| 1920x1200 ideal | `1023,1170,90x11` | `598,1167,35x29` | level digits sit on pale tiles |
| 1920x1080 default | `1043,1031,177x17` | — | |

Ideal-mode EXP boxes start *past* the `EXP` label — the label's letters otherwise
read as digits. Name/job boxes are bottom-anchored and 96x15 at 1920.

## Lessons from this session (read these)

- **Do not claim a fix without a verified read.** Three times a "fix" was reported
  that did not hold up: the split penalty, the background subtraction, and applying
  the per-shape threshold everywhere. Each was plausible and each was wrong.
- **A diagnostic can lie.** `settings::load_level_atlas()` reads the *active in-memory*
  profile; in a test process nothing has activated one, so it returns empty and logs
  "no level glyphs learned yet" — which sent a whole detour chasing data loss. Use
  `profile::load_for_mode(...)` fields directly in diagnostics.
- **Never blanket-kill processes.** `Get-Process node | Stop-Process -Force` killed
  Command Code itself. Use the task id, or the process holding port 1420.
- **Never edit `.commandcode/taste/`.** Read-only; use the `taste` tool.
- **Do not delete user data without explicit permission.** Flag history rows
  (`bad: true`) rather than removing them.
- Ask when the symptom is ambiguous. Guessing cost more time than asking would have.
