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

Last updated: 2026-11-19

## Current release — 0.7.0 (released 2026-11-19)

Goal: make the reader's failures legible and its corrections trustworthy — a
closed job vocabulary, a validated EXP readout, and per-field provenance that
explains a rejection — while fixing the overlay behaviours the 0.6.0 test run
turned up.

- [x] **Job reads are validated against a closed vocabulary** instead of accepted
  as free text: exact and near-miss hits resolve, anything else is reported as
  unrecognised, and the two elemental branches are never confused. Third-job
  names are reserved for when the client ships them. See
  [D33](DECISIONS.md) and [SHIPPING.md](SHIPPING.md).
- [x] **Read provenance** on every reading and diagnostic: `source`, `engine`,
  `reason` and `durationMs` per field, so a rejected frame says which stage gave
  up rather than one catch-all sentence.
- [x] **Save diagnostic** in Calibration, including rejected frames, under
  `data/diagnostics/`.
- [x] **EXP/level validation hardened:** runner-up-aware digits, an EXP bracket
  grammar, and rejection instead of repair for junk between `%` and `]`.
- [x] **Overlay position survives a restart**, with a tray item to resume
  following the game corner.
- [x] **Capture-lost desktop notifications** (WinRT toast under a registered
  AUMID), fired on the transition to `lost`.
- [x] **History chart keeps recording gaps** when it thins a large range, and the
  front end has a test runner (`npm test`).
- [x] **Hold-out accuracy reporting** for the corpus, with the hold-out case
  pinned by key.
- [x] Release metadata: set `0.7.0` in `Cargo.toml`, `package.json`,
  `tauri.conf.json`, and the lockfiles; move `CHANGELOG` to a dated 0.7.0
  section; commit and tag.
- [ ] Build the distributable (`tauri build`) and attach it to the release.

Deferred: UI Size auto-detection (Later); the manual setting is authoritative.

Verified 2026-11-19: `cargo fmt --check`, `cargo test --lib` (100 tests),
`cargo clippy -D warnings`, `npm run typecheck`, `npm test` (5 tests),
`npm run build` and `npm run version:check` all pass. The 0.6.0 corpus results
still hold — 18 of 18 frame cases read exact on the client tier.

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
- [ ] **Version-bump script.** One command to set the project version everywhere
  it is stored: `src-tauri/Cargo.toml`, the `mscout` entry in
  `src-tauri/Cargo.lock`, `package.json`, the root/workspace entries in
  `package-lock.json`, and `src-tauri/tauri.conf.json`; optionally move the
  `CHANGELOG` Unreleased section to a dated release. Add a `--check` mode so a
  release or CI run fails when the versions drift apart.
- [ ] **Screenshot import tool.** Turn a client screenshot + ground truth into a
  complete `testdata/<WxH@mode>/` fixture and its manifest image entries.
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
| 0.6.0 | 2026-11-16 | 1920×1080 Default/Filtered, OCR map/identity fallbacks, full 1920×1080 matrix |
| 0.7.0 | 2026-11-19 | Job vocabulary, read provenance, EXP grammar, overlay position, lost-capture toasts |

Details are in [../CHANGELOG.md](../CHANGELOG.md). Historical probe notes from
the 0.5→0.6 work are in the git history and [DECISIONS.md](DECISIONS.md); the
roadmap does not repeat them.
