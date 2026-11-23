# Roadmap

Open work, most important first within each group. What shipped is in
[../CHANGELOG.md](../CHANGELOG.md); known limitations of the current version
are in [SPEC.md](SPEC.md#13-known-limitations); the reasoning behind choices is
in [DECISIONS.md](DECISIONS.md). Remove an item when it ships.

## Reading correctness

- [ ] **Full Screen at a small size.** MesoVibe reports that Full Screen at
  1280×720 or 800×600 on a larger display stretches a 720- or 600-line
  picture (with black side bars for 4:3 on a wide display). `native.rs`
  assumes a 768-line picture above 768 lines, so such a frame reads wrong.
  Capture one, then let `Scaling` try those source heights too.
- [ ] **Confirm the scaling rules at more sizes.** The Ideal block size
  (`height / 720`) is measured only at 1080, 1200 and 1440 lines; a 1600- or
  2160-line Ideal capture settles it. The Default picture-width rounding is
  measured only at 16:9 and 16:10; a windowed size such as 1600×900 checks it
  at another width. Add each capture to `testdata/` as a scene.
- [ ] **Filtered glyph variants (atlas coverage).** The digit sweep found a live
  `8` that the seeded atlas does not represent: at 8×11 the correct `8` scores
  ~29 while `6`/`9` score ~10–12. A 20-EXP error can pass the percentage
  rounding and be recorded. A percentage `3` also scores as `8`; the frame check
  repairs that percentage, but the raw glyph is still wrong. Generate
  blurred/thresholded variants from the client glyphs at the measured size, or
  learn a variant only after an EXP/level/pct-consistent read. Rerun the digit
  sweep and promote its known-gap expectations to exact.
- [ ] **Exact 1x map label match.** On the 1x picture the label is drawn in
  1-bit Arial Bold at a fixed size, so it can be matched exactly against the
  client's map table rendered the same way, instead of Windows OCR plus the
  whole-line DTW. That would also settle the Roman-numeral siblings below and
  drop the `Windows.Media.Ocr` dependency.
- [ ] **Roman-numeral DTW fallback.** Exact OCR decides `I`/`II`/`III` when it
  reads the line. When OCR is absent or only fuzzy (small Ideal labels), the
  whole-line `match_map_name` DTW can still choose the wrong sibling. Give that
  path its own suffix/width decision, or require a cross-check before accepting
  a result from it.
- [ ] **Read the whole EXP field.** The EXP box is sized for the current digit
  count, so a much longer number can be clipped until the box is redrawn. In
  the 1x picture the readout always starts at the same place after `EXP.`;
  read from there to the end of the bar instead of a fitted box.
- [ ] **EXP bar fill as a cross-check.** The bar's fill length is an
  independent measure of the percentage (maple-route uses it). Reject a read
  whose percentage disagrees with the fill by more than its pixel resolution.
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

- [ ] **Overlay on a full-monitor game.** The overlay's top-left is put at the
  client's top-right corner (`engine::anchor_overlay`). When the game fills its
  monitor (2560×1440 on a 2560×1440 display) that corner is on the next
  display, so the overlay jumps there. Keep it on the game's monitor: place it
  inside the client's top-right corner when the outside one is off that
  monitor.
- [ ] **History window, full design.** Today it has 1/7/30-day presets, the
  EXP chart with gap breaks, a summary and the repair button. Add: All and
  custom ranges (keys `T / 7 / 30 / A`); a rate pane under the EXP pane sharing
  its x-axis (1-minute line, faint 5-minute average, drawn hollow and labelled
  coarse when built from one-minute buckets); idle bands; markers for
  level-ups, deaths and map changes (map markers on hover at month scale); a
  crosshair tooltip with time, level, EXP, map, rate and the ETA then; wheel
  zoom, drag pan, double-click reset and brush selection; date separators; a
  fidelity badge (`1s raw` / `1m aggregates`); best 1-minute rate and idle time
  in the summary.
- [ ] **Compaction on disk.** Keep raw 1 Hz rows for 7 days, then compact to
  one-minute aggregates (mean EXP, min/max, gain sum, map set) so months stay
  small; never delete aggregates automatically. Today day files keep every row
  and long ranges are only thinned when read (DATA.md, Compaction).
- [ ] Per-map comparison once enough real data exists.
- [ ] **Distributable build.** Run `tauri build`, check the NSIS installer keeps
  `data/` across an upgrade and an uninstall, offer a portable folder copy next
  to the exe, and attach both to the GitHub release (not done for 0.8.0).

## Later

Unscheduled, deferred or out of scope for now.

- [ ] **Retire the client-size reader machinery.** Every UI Size now reads the
  1x picture, so the scaled glyph variants (`scale_variants`), live rendering
  learning (`learn_rendering`), the near-tie settle (`settle_in_place`), the
  Default/Filtered seeded atlases, the 1920×1080 Default/Filtered presets and
  the Windows OCR name/job fusion no longer serve a live read. Remove them,
  checking the corpus and the digit sweep still pass.
- [ ] **Fewer teach flows.** With exact 1x reading the name/job and map teach
  steps in Calibration should rarely be needed; keep them as overrides, and
  drop the ones the corpus shows are never used.
- [ ] **Screenshot import tool.** Turn a full client screenshot + ground truth
  into a `testdata/frames/<WxH@mode>/<scene>/` with its `scene.json`. (A saved
  diagnostic already imports as focused crops: `corpus::import_diagnostic`.)
- [ ] **Auto-discover the MSCW folder** from the Steam library; keep the
  `mscwDir` override.
- [ ] **Optional offline map-name cache** after profiles are seeded (derived
  local data; no shipped name list).
- [ ] **Remove `window-error.log`** once history-window creation is stable.
- [ ] Damage/AP/potion calculators (out of v1 scope).
- [ ] Shareable taught glyph/map sets between machines at the same resolution.
