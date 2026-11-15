# Roadmap

Living status. Update this file at the end of every working session — it is the first thing to read when resuming. Released history lives in [../CHANGELOG.md](../CHANGELOG.md); this file is forward-looking.

Last updated: 2026-11-14

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

## Next

- [ ] **Default/Filtered UI sizes**: identify each pair's digit/level source and Arial rendering, add the map-name index at its label size, then ship presets and saved-frame tests. The 1920×1080 Default frame currently drops EXP punctuation, reads only the level's `5`, and does not match Arial; 1920×1200 Default EXP is exact, so this is pair-specific.
- [ ] **Real-screenshot regression suite**: ship a small labelled corpus of captured frames/crops with ground truth in a manifest — client size, UI size, minimap state, character name/job, level, EXP text/value/percentage, street/map, and expected boxes. Replay it through the reader and minimap detector as a regression gate. Keep it small and treat it as test data, not runtime assets (see D28).
- [ ] **Map/street matching at other UI sizes**: the name index is rendered at Arial Bold 12, which matches Ideal. Render it at the label's measured pixel size for Default/Filtered; the client table itself is size-independent.
- [ ] **Multi-resolution portability** (layers 1–2): anchor-relative boxes + scale-aware matching, so one calibration serves several resolutions. Blocked on a two-resolution capture diff to learn whether the UI is fixed-size, integer-scaled, or letterboxed.
- [ ] Level-up stitching and death-event marking on the chart
- [ ] 7-day raw → 1-minute aggregate compaction job
- [ ] History window polish: brush/zoom panning
- [ ] Per-map comparison once enough real data exists
- [ ] Portable build (`tauri build`, folder copy next to the exe)

## Later

- [ ] Damage/AP/potion calculators (out of v1 scope)
- [ ] Shareable taught glyph/map sets between machines at the same resolution

## Known gaps / debt

- **Ideal only.** Only Ideal Ratio has a verified automatic level/Arial/map path and shipped presets. Default/Filtered fall back to calibration; 1920×1080 Default has a different digit rendering and no Arial match yet. 1920×1200 Default's EXP reads, but its level/name/job are unverified.
- Calibration boxes and fingerprints are physical-pixel and UI-scale specific. A resolution/UI-size change switches to that pair's profile; a pair with no profile shows uncalibrated until its boxes exist.
- A transient client height during a resolution change can create an empty profile (e.g. `1920x1081@ideal`). Move such profiles aside and restart if the app selects one instead of the intended size.
- Coverage reports characters, not sizes. A digit learned from HP/MP (smaller size) does not parse the larger EXP font; relearn from the EXP box when it appears there.
- The EXP box is sized for the current digit count. A much longer EXP number can clip the ROI until it is redrawn.
- Map/street matching uses the client's table (347 names in the current build; version-specific); a map missing from it, or a UI size whose label is not 12 px, falls back to taught fingerprints/letters. Name/job auto-read and map/street rendering rely on Windows Arial (system font, not bundled).
- History window has no brush/zoom panning yet — presets only.
- `window-error.log` is written to the data dir when history window creation fails; remove once stable.
- No automated tests beyond unit tests and ignored live diagnostics.
- Dev build expects the Vite server; a direct `cargo run` needs `npm run dev` running too.
