# Roadmap

Living status. This file is forward-looking: released detail lives in
[../CHANGELOG.md](../CHANGELOG.md), and the reasoning behind a choice lives in
[DECISIONS.md](DECISIONS.md).

How to read it:

- **Current release** — the verification and release checklist for the next
  version. No unchecked product work belongs here; deferred items move out.
- **Next** — scheduled work, correctness and reading quality before features.
  Each item has a concrete outcome or approach.
- **Later** — unscheduled, deferred, or out-of-scope work.
- **Known limitations** — what the app does today. These are facts, not tasks.
  If work is scheduled, the entry links to the one Next item that owns it; the
  work is not repeated here.

Last updated: 2026-11-16

## Current release — 0.6.0 (implemented, pending release)

Goal: read 1920×1080 **Default** and **Default (Filter Applied)**, keeping
1366×768 in all three UI sizes and 1920×1080 Ideal as the unchanged reference
paths. mpl-ocr is a reference only; its MIT-licensed ideas are reimplemented
locally with no runtime dependency (see [D31](DECISIONS.md) and
[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)).

- [x] 1920×1080 Default/Filtered presets (boxes only); all six pair/state cases
  pass the client-tier corpus.
- [x] Saturation-aware white key for the Filtered level tile; the level uses its
  own atlas and the ordered matcher (strict → split → tolerant).
- [x] Filtered name/job: per-line Arial seeding, strict live-glyph learning, and
  Windows OCR fusion; the corpus reads `Euroblood` / `Page`.
- [x] Filtered EXP: punctuation variants, readout grammar, touching-pair split,
  and near-miss digits; live low-percent and leading-digit fixtures pass.
- [x] EXP/level/pct frame check: a large disagreement invalidates both; a close
  percentage error (up to 1 pp) is recomputed from the valid EXP and level.
- [x] Map labels: OCR union, client-table exact/fuzzy/suffix matching, literal
  names for maps missing from the table, one-line labels without a street, and
  exact OCR ahead of the Roman-numeral DTW. Live Tree `I` and literal Balrog
  cases are covered.
- [x] Filtered EXP digit sweep across live crops; expected inputs cover `0-9`
  plus punctuation. The sweep records the remaining live `8` case.
- [x] Overlay shows full long map names (wraps; the window grows to fit).
- [ ] Release mechanics: set `0.6.0` in `Cargo.toml`, `package.json`,
  `tauri.conf.json`, and the lockfile; move `CHANGELOG` Unreleased to a dated
  0.6.0 section; commit, tag, and build.

Deferred: UI Size auto-detection (Later); the manual setting is authoritative.

Verified 2026-11-16: `cargo test --lib` passes, and
`reads_every_fixture_from_empty_atlases` passes every case — 1366×768 and
1920×1080 in all three UI sizes, all three minimap states. 1920×1200 has no
preset and falls back to calibration.

Non-goals: vendoring/running mpl-ocr, its `%LOCALAPPDATA%` config model, its
global `clientH / 768` scale, tooltip/inventory/damage detectors, and HP/MP as
product features.

## Next

### Reading correctness

- [ ] **Filtered glyph variants (atlas coverage).** The digit sweep found a live
  `8` that the seeded atlas does not represent: at 8×11 the correct `8` scores
  ~29 while `6`/`9` score ~10–12. A 20-EXP error can pass the percentage
  rounding and be recorded. A percentage `3` also scores as `8`; the frame check
  repairs that percentage, but the raw glyph is still wrong. Generate
  blurred/thresholded variants from the client glyphs at the measured size, or
  learn a variant only after an EXP/level/pct-consistent read. Rerun the digit
  sweep and promote its known-gap expectations to exact.
- [ ] **Roman-numeral DTW fallback.** Exact OCR decides `I`/`II`/`III` when it
  reads the line. When OCR is absent or only fuzzy (small Ideal labels), the
  whole-line `match_map_name` DTW can still choose the wrong sibling. Give that
  path its own suffix/width decision, or require a cross-check before accepting
  a result from it.
- [ ] **Calibration window startup freeze.** The window blocks for several
  seconds before it can be moved or clicked (observed on 1920×1080 Filtered).
  Open an immediate shell/preview, then move capture, label-index construction,
  atlas seeding and WZ work off the UI thread. Cache the derived name/label
  index as part of the fix. Measure the open path rather than extending a
  timeout.
- [ ] **Live Balrog label fixture.** The `Where the Balrogs Live` literal
  fallback is unit-tested, but its live label crop is not in `testdata/`. Add
  the crop as a client-tier regression fixture when the map is available.

### Product work

- [ ] Level-up stitching and death-event marking on the chart.
- [ ] 7-day raw → 1-minute aggregate compaction job.
- [ ] History window brush/zoom panning (preset ranges only today).
- [ ] Per-map comparison once enough real data exists.
- [ ] Portable build (`tauri build`, folder copy next to the exe).

## Later (unscheduled or deferred)

- [ ] **UI Size auto-detection.** Choose the box set from the status bar instead
  of the manual setting; fall back to manual when the match is not unique. The
  manual setting remains authoritative until this ships.
- [ ] **Redesign the testdata corpus layout.** Each `WxH@mode` setting should be
  one self-contained subdirectory with many explicitly named PNGs — multiple
  minimap-state images, multiple HUD/EXP captures, map-label crops, and the
  settings evidence — with a manifest that indexes every image and its crop and
  ground truth. This replaces the current mix of manifest-listed state files and
  extra focused fixtures (`exp-*.png`, `minimap-*.png`, `hud-low-percent.png`)
  at the pair root. Update the `regression.rs` walker and tiers,
  `testdata/README.md`, and migrate the existing fixtures while preserving
  `MSCOUT_CASE`/state filtering and the hermetic/client split.
- [ ] **Version-bump script.** One command to set the project version everywhere
  it is stored: `src-tauri/Cargo.toml`, the `mscout` entry in
  `src-tauri/Cargo.lock`, `package.json`, the root/workspace entries in
  `package-lock.json`, and `src-tauri/tauri.conf.json`; optionally move the
  `CHANGELOG` Unreleased section to a dated release. Add a `--check` mode so a
  release or CI run fails when the versions drift apart.
- [ ] **Screenshot import tool.** Turn a client screenshot + ground truth into a
  complete `testdata/<WxH@mode>/` fixture and manifest entry.
- [ ] **Multi-resolution portability.** Anchor-relative boxes + scale-aware
  matching so one calibration serves several resolutions; blocked on a
  two-resolution capture diff.
- [ ] **1920×1200 support.** No preset or fixture today; manual calibration
  works. Add presets and corpus only if the size is requested.
- [ ] **Auto-discover the MSCW folder** from the Steam library; keep the
  `mscwDir` override.
- [ ] **Optional offline map-name cache** after profiles are seeded (derived
  local data; no shipped name list).
- [ ] **Remove `window-error.log`** once history-window creation is stable.
- [ ] Damage/AP/potion calculators (out of v1 scope).
- [ ] Shareable taught glyph/map sets between machines at the same resolution.

## Known limitations

These are current behavior, not a task list. Work that addresses one is linked
to its single entry under **Next** or **Later**.

- **Support matrix:** 1366×768 and 1920×1080, all three UI sizes. 1920×1200 has
  no preset or fixture and needs manual calibration (Later).
- **UI Size is selected manually.** A wrong mode selects the wrong profile and
  boxes, so it reads nothing or misreads. Auto-detection is deferred (Later).
- **Profiles are physical-pixel and UI-scale specific.** A size/UI-size change
  switches profiles; a transient client size can create an empty profile (e.g.
  `1920x1081@ideal`). Move it aside if the app selects it.
- **Glyph coverage is reported by character, not size.** A digit learned from
  HP/MP at a smaller size does not parse the larger EXP font.
- **The EXP box is sized for the current digit count.** A much longer EXP number
  can clip the ROI until it is redrawn.
- **Filtered atlas gaps.** A live `8` can read as `6`/`9` (20-EXP error; the
  20-EXP change can pass the rounding check), and a percentage `3` can read as
  `8` (the percentage is repaired from EXP/level). See Next → filtered glyph
  variants.
- **Map matching depends on the client table.** It has 347 names and 11 streets
  in the current build; the count is version-specific. A name absent from the
  table is shown from the OCR line; a one-line label has no street; an ambiguous
  `<N>` suffix stays `unknown`. Exact OCR beats the whole-line DTW, but the DTW
  fallback can still confuse Roman-numeral siblings; see Next.
- **Windows dependencies:** Arial is used for name/job and label rendering;
  `Windows.Media.Ocr` is used for the Default/Filtered name/job and map label.
  Neither is bundled or installed by the app.
- **History ranges are presets only** (no brush/zoom); see Next.
- **Calibration can block for seconds when opened;** see Next.
- **`window-error.log`** is still written when history-window creation fails;
  see Later.
- **Dev build expects Vite.** A direct `cargo run` needs `npm run dev` running.

## Release history

| Version | Date | Headline |
|---|---|---|
| 0.4.0 | 2026-11-14 | Automatic Ideal HUD reads, calibration, presets, storage, history |
| 0.5.0 | 2026-11-15 | 1366×768 in all three UI sizes, 1920×1080 Ideal, three minimap states |
| 0.6.0 | pending | 1920×1080 Default/Filtered, OCR map/identity fallbacks, full 1920×1080 matrix |

Details are in [../CHANGELOG.md](../CHANGELOG.md). Historical probe notes from
the 0.5→0.6 work are in the git history and [DECISIONS.md](DECISIONS.md); the
roadmap does not repeat them.
