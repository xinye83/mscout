# Roadmap

Living status. Update this file at the end of every working session — it is the first thing to read when resuming. Released history lives in [../CHANGELOG.md](../CHANGELOG.md); this file is forward-looking.

Last updated: 2026-11

## Done

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

## Next

- [ ] **Multi-resolution portability** (layers 1–2): anchor-relative boxes + scale-aware matching, so one calibration serves several resolutions. Blocked on a two-resolution capture diff to learn whether the UI is fixed-size, integer-scaled, or letterboxed.
- [ ] Calibration window: show which profile is active and let the user pick one
- [ ] Level-up stitching and death-event marking on the chart
- [ ] 7-day raw → 1-minute aggregate compaction job
- [ ] History window polish: brush/zoom panning
- [ ] Per-map comparison once enough real data exists
- [ ] Portable build (`tauri build`, folder copy next to the exe)

## Later

- [ ] Damage/AP/potion calculators (out of v1 scope)
- [ ] Shareable taught glyph/map sets between machines at the same resolution

## Known gaps / debt

- Calibration, glyph cells and fingerprints are physical-pixel and UI-scale specific. A resolution change invalidates them; the app clears them when the calibrated client size changes and the boxes must be redrawn.
- Coverage reports characters, not sizes. A digit learned from HP/MP (smaller size) does not parse the larger EXP font; relearn from the EXP box when it appears there.
- The EXP box is sized for the current digit count. A much longer EXP number can clip the ROI until it is redrawn.
- History window has no brush/zoom panning yet — presets only.
- `window-error.log` is written to the data dir when history window creation fails; remove once stable.
- No automated tests beyond unit tests and ignored live diagnostics.
- Dev build expects the Vite server; a direct `cargo run` needs `npm run dev` running too.
