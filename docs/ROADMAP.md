# Roadmap

Open work, most important first within each group. What shipped is in
[../CHANGELOG.md](../CHANGELOG.md); known limitations of the current version
are in [SPEC.md](SPEC.md#15-known-limitations); the reasoning behind choices is
in [DECISIONS.md](DECISIONS.md). Remove an item when it ships.

## Reading correctness

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
- [ ] **Live Balrog label fixture.** The `Where the Balrogs Live` literal
  fallback is unit-tested, but its live label crop is not in `testdata/`. Add
  the crop as a client-tier regression fixture when the map is available.
- [ ] **Percentage-only EXP readout — capture one first.** `ocr::validate_readout`
  rejects a readout with no number before its bracket (`[58.47%]`), so such a
  frame records nothing. Other trackers say the HUD drops the number once EXP
  is large, but no screenshot or fixture here shows it. First capture one and
  add it to `testdata/` (Save diagnostic, then `corpus::import_diagnostic`). If
  it is real, record `exp: null` with the percentage — never an EXP derived
  from it (SPEC §6, "never write a guess") — and let rates and the ETA fall
  back to %/h for those rows.

## Product

- [ ] Level-up stitching and death-event marking on the chart.
- [ ] 7-day raw → 1-minute aggregate compaction job.
- [ ] History window brush/zoom panning (preset ranges only today).
- [ ] Per-map comparison once enough real data exists.
- [ ] **Distributable build.** Run `tauri build`, check the NSIS installer keeps
  `data/` across an upgrade and an uninstall, offer a portable folder copy next
  to the exe, and attach both to the GitHub release (not done for 0.8.0).

## Later

Unscheduled, deferred or out of scope for now.

- [ ] **UI Size auto-detection.** Choose the box set from the status bar instead
  of the manual setting; fall back to manual when the match is not unique. The
  manual setting remains authoritative until this ships.
- [ ] **Screenshot import tool.** Turn a full client screenshot + ground truth
  into a `testdata/frames/<WxH@mode>/<scene>/` with its `scene.json`. (A saved
  diagnostic already imports as focused crops: `corpus::import_diagnostic`.)
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
