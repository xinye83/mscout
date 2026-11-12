# Changelog

All notable changes to MScout are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the major version is `0`, the minor version is bumped for features and the patch for fixes; anything may still change between `0.x` releases.

## [Unreleased]

### Planned
- Multi-resolution portability: anchor-relative boxes and scale-aware matching, so one calibration serves several client sizes. Depends on a two-resolution capture diff to establish whether the game UI is fixed-size, integer-scaled or letterboxed.
- Level-up stitching and death markers on the chart.
- Retention: compact raw samples older than 7 days into one-minute aggregates.
- History window pan/zoom.
- Per-map comparison once enough real data exists.

## [0.1.0] — 2026-11-11

First release. Read-only: it observes the screen and never interacts with the game process.

### Added
- **Overlay.** Frameless, transparent, always-on-top, click-through by default. Anchored to the game window's top-right corner and re-anchored when the game moves or resizes. Shows map, character name, job, level, EXP with two decimals, a progress bar, the 5-minute rate and the ETA.
- **Window targeting.** Picker listing capturable windows with process name, pid and client size. The choice persists as a title + process selector and is re-resolved every second, so restarting the game reattaches without user action.
- **Capture.** Windows Graphics Capture on a dedicated thread, free-threaded frame pool, client-area crop aligned with DWM extended frame bounds. One frame per second; continues while the game is unfocused or occluded.
- **Calibration window.** Frozen preview with drag-to-select boxes for EXP, level, map, character name and job; learn EXP digits by typing the on-screen string; teach map/name/job fingerprints; a live parse probe to confirm before trusting it.
- **Parsing.** Size-aware glyph atlas with Otsu thresholding per region; unlearned or ambiguous glyphs are rejected rather than guessed. EXP percentage cross-checked against `exp / required(level)` so a misread produces no sample.
- **Map recognition.** The minimap panel is detected on every frame and the name crop normalised to a fixed grid, so moving or resizing the minimap never invalidates a fingerprint. Two-line map names are supported and rendered as the game draws them.
- **Per-resolution profiles.** Calibration is scoped to the client size and stored in `data/profiles/<W>x<H>.json`. The engine activates the profile matching the live window; recording starts only after consecutive good reads, so an uncalibrated or mismatched profile reports *lost* instead of writing plausible-looking rows.
- **Storage.** One JSONL row per second at `data/samples/YYYY-MM-DD.jsonl`. Failed parses write nothing, so a broken capture appears as missing data, never as "stopped gaining EXP".
- **History window.** 1/7/30-day presets, gaps drawn as gaps, cumulative EXP gained within the selected range, summary strip.
- **Rates.** Wall-clock trailing 1/5/30-minute windows with no session splitting. The prominent rate and ETA use the 5-minute average, shown as `—` until a full window exists, at zero, and while idle.
- **Tray menu and hotkeys.** `Ctrl+Alt+T` pick window, `Ctrl+Alt+M` toggle interactivity, `Ctrl+Alt+V` show/hide, `Ctrl+Alt+H` history.
- **Docs.** Setup guide, behavioral spec, roadmap, decision log, data formats, dev notes.
- **Tests.** Rust unit tests for the EXP table, glyph segmentation and matching, thresholding, settings/profile round-trips and window enumeration, plus `#[ignore]`d diagnostics for live capture.

[Unreleased]: https://github.com/xinye83/mscout/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/xinye83/mscout/releases/tag/v0.1.0
