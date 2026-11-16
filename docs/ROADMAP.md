# Roadmap

Living status. Update this file at the end of every working session — it is the first thing to read when resuming. Released history lives in [../CHANGELOG.md](../CHANGELOG.md); this file is forward-looking.

Last updated: 2026-11-15

## 0.4.0 — released 2026-11-14

- [x] Automatic level readout seeded from the client EXP atlas, with bright-ink segmentation over the pale level tiles
- [x] Automatic name/job read from Windows Arial, with taught fingerprints as the exact fallback
- [x] Automatic map/street matching against the client's own name table by whole-line DTW
- [x] Shipped Ideal Ratio presets for 1366×768, 1920×1080 and 1920×1200 with EXP, level, name and job boxes
- [x] Calibration panel consistency pass: shared status/read layout, readiness-based folding, `Correct manually`, auto-refreshed reads
- [x] Removed unverified Default/Filtered presets; those UI sizes fall back to calibration

## Done (pre-0.4.0)

- [x] Tauri 2 + React + uPlot scaffold; Rust backend compiles, release binary builds
- [x] Frameless, transparent, always-on-top, skip-taskbar overlay window
- [x] Click-through by default (`Ctrl+Alt+M` toggles interactivity, green border + `interactive` chip)
- [x] Show/hide (`Ctrl+Alt+V`), history window (`Ctrl+Alt+H`), tray menu with all actions
- [x] Storage commands: `append_sample`, `read_samples`, `get_settings`, `set_settings`
- [x] Classic World EXP table: 1–49 exact, 50–89 extrapolated (`estimated` flag), 90–100 at 2.1×
- [x] Wall-clock trailing rates (1 / 5 / 30 min); prominent rate and ETA on the 5-minute average
- [x] Live panel with map, level, EXP bar, rate, ETA, expandable chart
- [x] History window: 1/7/30-day presets, gap rendering, compact axis, recorded-time rate
- [x] Window targeting: picker window, title/process selector persisted in `settings.json`, 1 Hz re-resolve (reattaches after a game restart)
- [x] WGC capture in Rust on a dedicated thread: free-threaded frame pool, client-area crop via DWM extended frame bounds, 1 Hz
- [x] ROI calibration window: frozen preview, drag EXP / level / map / name / job boxes, save regions
- [x] Digit glyph atlas: learn by typing the on-screen string, size-aware variants, unknown/ambiguous glyphs rejected
- [x] Map, character-name and job fingerprints: teach once, nearest-match, `unknown` instead of guessing
- [x] Minimap panel auto-detected every frame; map fingerprints are position-independent (moving/resizing the minimap does not break them)
- [x] Per-resolution profiles (`data/profiles/<W>x<H>.json`): engine activates the profile for the live client size, recording gated on 3 consecutive good reads, legacy settings migrated on first start
- [x] Wire readings to storage; overlay renders real samples, demo data and chip removed
- [x] Cross-check `pct ≈ exp / required(lv)` (0.25 pp on confirmed levels, 1 pp on estimated ones)
- [x] Overlay identity line (character name + job) and capture-state dot/badge
- [x] Overlay anchored to the game client's top-right corner (re-anchors on move/resize)
- [x] Rate and ETA use the 5-minute average; `—` until a full 5-minute session window, at zero, and ETA while idle
- [x] `scripts/gen-mock.mjs` mock data generator
- [x] Native-bitmap glyph storage and matching (`Atlas.format = 2`): exact same-size matching, punctuation and `[`/`]` ambiguity fixed, legacy 12×16 cells migrated
- [x] **Game-atlas seeding**: locate the MSCW folder, read `StatusBar3.img/mainBar/EXPBar/number` at the rendered height, fill the missing `1`/`]`/`,`/`.`, and mark the atlas `source: game`; a 1920×1080 Ideal preset (boxes only) ships and reads after seeding
- [x] Level seeding: the live level digit is the EXP glyph widened (7×7 at Ideal), with bright-ink segmentation to separate it from the pale tile frame; confirmed at distance 0 against the stored `EXPBar/number` glyph
- [x] Text-rendering audit: enumerated every HUD glyph set with the `wz_reader` crate; confirmed `EXPBar/number` has no `1`/`]`/punctuation nodes, `lvNumber` is not the Ideal level source, and the native `7` is 5×7
- [x] Self-refining seed: a confident near match (the Ideal `7`) is stored as an exact live variant, so every segment reads at distance 0.0 after seeding
- [x] From-empty-atlas proof: seeding reads `74,521[42.78%]` on the Ideal captures (1366, 1920×1080, 1920×1200) and the 1920×1200 Default capture; the Ideal level reads `35`
- [x] **Automatic name/job**: seed an Arial 1-bit atlas from the live crop and read both lines as text (`Euroblood` / `Page` verified), with the taught fingerprints as the exact fallback
- [x] **Automatic map/street**: read the client's own `String_000.wz/Map.img` table, render each name in Arial Bold 12, and match the whole label line by width filter + column-pattern DTW; both saved maps (`Kerning City Civic Center`, `Sunset Sky`) and `Victoria Road` read exactly, no teaching
- [x] UI/code/docs audit after the read rework: source tracking (`game`/`taught`) is consistent across corrections and profile switches; stale 12×16/preset-glyph docs corrected; dead helpers removed; engine seeding retries throttled to 30 s
- [x] Clean-install audit: with no profiles/atlases/overrides, the 1920×1080 Ideal preset (now with level/name/job boxes) + client files + Windows Arial read both saved frames exactly; the street fallback no longer requires a drawn street box

## Post-0.4.0

- [x] Three-state minimap detection anchored on the `WORLD` badge: expanded, title-bar-only, folded
- [x] Folded-bar map reading: match the single `Street :Map` line against the client table in Arial Regular 12
- [x] Roman-numeral tie-break: names that differ only by `I`/`II`/`III` are separated by rendered width
- [x] No carried map: a state with no name on screen reports `unknown`
- [x] Calibration audit: preset geometry is not drawn as a box, Street/Map is detection-only, panel headers share one status vocabulary, and the legacy `Fixes waiting` review queue is removed

## 0.5.0 — target

Goal: support **at least two client resolutions across all three UI Size modes**
(Default Ratio, Ideal Ratio and Filtered).

- [ ] Pick two resolutions and verify every pair: EXP digit source/scale, level source/scale, Arial name/job rendering, map-label font size, and boxes. Ship a preset and saved-frame test for each verified pair. 1366×768 is verified in all three UI modes and all three minimap states; 1920×1080 has all three minimap states and presets for Ideal, Default and Filtered. Ideal is fully verified; Default reads EXP, level and name from the corpus, with the job and map-label size still open; Filtered loses the EXP comma and needs its level/name/job pass. 1920×1200 ships Ideal only.
- [ ] **Default/Filtered UI sizes**: identify each pair's digit/level source and Arial rendering, add the map-name index at its label size, then ship presets and saved-frame tests. 1366×768 Default and Filtered are verified — they share the Ideal geometry, and the saved-frame corpus reads every state at confidence 1.0; their presets ship. Remaining: 1920×1080/1200 Default/Filtered. The 1920×1080 Default read now handles EXP punctuation and merged digits, the level's `3`, and anti-aliased Arial; the job and map-label size remain. The Filtered rendering still needs a filter-specific pass (EXP comma, level, name, job); 1920×1200 Default/Filtered have no corpus or preset. The minimap state detector now handles every pair and state.
- [ ] **UI Size autodetection (layout, not label)**: pick the box set from the frame instead of requiring the player to choose. Scan the status bar for the orange level-tile cluster and the HP/MP/EXP bar anchors, then match the measured geometry against the known layouts for the live client size. 1366×768 has one layout for all three modes; 1920×1080/1200 splits into Ideal vs a shared Default/Filtered layout (they share boxes, and the reader seeds glyphs from the frame, so they need no separation). Fall back to the manual `uiMode` setting when the match is not unique or the status bar is missing; never guess. Resolution is already detected from the captured client size. An optional later refinement could read the label itself from the game's config or the always-open Graphics panel.
- [ ] **Map/street matching at other UI sizes**: render the name index at the label's measured pixel size for Default/Filtered; the client table itself is size-independent. The index is now built per size and the matcher tries 12–24 px, and the label line splitter isolates white text rows from the panel fill, icon and border; the 1366 and 1920 Ideal pairs still read. The 1920 Default line is detected, but its name still scores above threshold; the Filtered label is untested.
- [x] **Real-screenshot regression suite**: ship a small labelled corpus of captured frames/crops with ground truth in a manifest — client size, UI size, minimap state, character name/job, level, EXP text/value/percentage, street/map, and expected boxes. Replay it through the reader and minimap detector as a regression gate. Keep it small and treat it as test data, not runtime assets (see D28). The suite lives in `src-tauri/src/regression.rs`, with the corpus in `src-tauri/testdata/`; it has a hermetic tier and an `--ignored` client tier, `MSCOUT_CASE` narrows a run, and no fixture is skipped.
- [ ] **Screenshot import tool**: one command turns an externally supplied client-area screenshot plus ground truth into a complete `src-tauri/testdata/<WxH@mode>/` fixture. Run the real minimap detector and the pair's preset boxes to derive the minimap and HUD crops, locate the always-open Graphics panel (stored as resolution/UI-Size evidence only), validate the EXP arithmetic, the panel labels and the map/street names against the client table, then write the crops and `meta.json`. Accept PNG/BMP; JPEG needs a codec.
- [x] **Minimap-state coverage (1366×768 and 1920×1080)**: capture all three minimap states (expanded, title-bar-only, folded) for Default, Filtered and Ideal at both 0.5.0 resolutions. 1366×768 has all three for all modes; 1920×1080 has all three for Ideal (East Rocky Mountain III), Default and Filtered (Hidden Street / Free Market <8>). 1920×1200 Ideal still has no fixture.
- [ ] **Multi-resolution portability** (layers 1–2): anchor-relative boxes + scale-aware matching, so one calibration serves several resolutions. Blocked on a two-resolution capture diff to learn whether the UI is fixed-size, integer-scaled, or letterboxed.

### Open items in 0.5.0

- **1920×1080 Filtered (current live setting)**: the EXP now reads
  `127,028[72.91%]` exactly after the comma fix. The per-box probe reads the
  first level digit (`3`); the client tier still returns no level because the
  second digit's 11×14 bright shape is merged with the tile edge. The per-box
  probe reads job `Page` and name `Furoh ood`; the app's single merged identity
  atlas (job 14 px + name 19 px) reads `Poge` / `Furohjood`, so merging different
  font sizes is the next name/job root cause. The map stays `unknown`.
- **1920×1080 Default**: EXP `127,028[72.91%]`, level `35` and name `Euroblood`
  pass. The job reads `pogo` instead of `Page`, and the map is `unknown`: the
  170×13 map line is detected, but the client-table match scores above
  threshold.
- **1920×1200**: Ideal preset and no fixture; Default/Filtered have neither.
- **Map labels**: the index is size-aware (12–24 px) and the line splitter now
  isolates the street/map rows, but the 1920 Default/Filtered name match still
  needs the right render/table check.
- **Not started**: the screenshot import tool and UI Size layout autodetection.
- **Green**: all six pairs × three minimap states pass the state gate; 1366×768
  in all three UI sizes and 1920×1080 Ideal pass the full client read.

### Root-cause notes — 1920 Filtered (fixture probe)

- **EXP**: every glyph reads at distance 0 except the comma, which the filter
  renders as a 4×4 shape at distance 40.1. The punctuation templates cover
  2×2/3×4/4×3 and the adaptive-learn gate is 36, so the comma is dropped and the
  readout becomes `127028[72.91%]`.
- **Level**: the filter segments the two level digits at different sizes —
  10×11 and 11×14. `seed_level_atlas` picks one dominant size (11×14) and seeds
  only that, so the 10×11 digit never matches and `recognize_level` returns
  nothing.
- **Name/job**: the two crops seed at different Arial sizes (job 14 px, name
  19 px) and merge. The job's only failure is its `a`: 8×10 at distance 62.1,
  above the 45 text threshold. The name reads `Furoh ood` because the learned
  atlas stores the E as F and the b as h (both at distance 0 after learning),
  and the 2×13 `l` is closest to `L` at 79.7, so it is dropped.
- **Map**: the label yields two lines (110×10 street, 130×10 map). The street
  matches `Hidden Street` at 16 px (0.81). The table does contain
  `Free Market <8>`, but it renders 118×12 at 16 px; the live map line is
  130×10, twelve pixels wider and two shorter. The width filter (8 px) therefore
  excludes the correct entry, while `Free Market <10>` (127×12) is close enough
  in width to be tried and scores badly. The 1920 Default `Free Market <8>`
  line is 170×13 against a 163×14 22 px render (7 px); it fails on score.

**Fixes attempted from this probe**

- The EXP adaptive-learn gate is now 45, so the Filtered 4×4 comma (distance
  40.1) is learned; the Filtered EXP reads exactly.
- `seed_level_atlas` seeds every distinct bright digit size (10×11 and 11×14),
  but the second Filtered digit is still merged with the tile edge and does not
  match; the probe reads `3`, the client tier still reads no level.
- The text gate is now 65: the Filtered job `a` (distance 62.1) matches in the
  per-box probe and the job reads `Page`. The app merges the 14 px job and 19 px
  name atlases into one, which then misreads both (`Poge` / `Furohjood`); the
  merge is the next root cause, not the glyph shapes.
- The map width window is now 14 px, but the Filtered 130×10 line against the
  118×12 table entry still fails the DTW score. The label's horizontal/vertical
  scale differs from the GDI render; that is the remaining map cause.
- 1366×768 in all three UI sizes and 1920×1080 Ideal still pass the full client
  read; the hermetic suite passes 35 tests.

## Next

- [ ] Level-up stitching and death-event marking on the chart
- [ ] 7-day raw → 1-minute aggregate compaction job
- [ ] History window polish: brush/zoom panning
- [ ] Per-map comparison once enough real data exists
- [ ] Portable build (`tauri build`, folder copy next to the exe)

## Later

- [ ] Damage/AP/potion calculators (out of v1 scope)
- [ ] Shareable taught glyph/map sets between machines at the same resolution

## Known gaps / debt

- **1366×768 ships all three UI sizes; 1920×1080 ships all three; 1920×1200 ships Ideal only.** 1366 Default/Filtered share the Ideal geometry and read at confidence 1.0 in the saved-frame corpus. The 1920×1080 Default/Filtered presets ship and have full three-state fixtures; Default reads EXP/level/name, while its job and both pairs' map labels remain open. The Filtered pair is partial. 1920×1200 has only the Ideal preset and no fixture.
- Calibration boxes and fingerprints are physical-pixel and UI-scale specific. A resolution/UI-size change switches to that pair's profile; a pair with no profile shows uncalibrated until its boxes exist.
- A transient client height during a resolution change can create an empty profile (e.g. `1920x1081@ideal`). Move such profiles aside and restart if the app selects one instead of the intended size.
- Coverage reports characters, not sizes. A digit learned from HP/MP (smaller size) does not parse the larger EXP font; relearn from the EXP box when it appears there.
- The EXP box is sized for the current digit count. A much longer EXP number can clip the ROI until it is redrawn.
- Map/street matching uses the client's table (347 names in the current build; version-specific); a map missing from it falls back to taught fingerprints/letters. The label index is now built per font size (12–24 px) and the line splitter isolates the street/map rows, but the 1920 Default/Filtered name match still scores above threshold. Name/job auto-read and map/street rendering rely on Windows Arial (system font, not bundled).
- History window has no brush/zoom panning yet — presets only.
- `window-error.log` is written to the data dir when history window creation fails; remove once stable.
- The 1920×1080 Default/Filtered presets ship, but only Default EXP/level/name and the Ideal pair are fully read by the corpus. The Default job and the Default/Filtered map-label size, plus the Filtered EXP comma/level/name/job, remain open; 1920×1200 Default/Filtered still fall back to calibration. The minimap state gate passes all six pairs across all three states.
- Dev build expects the Vite server; a direct `cargo run` needs `npm run dev` running too.
