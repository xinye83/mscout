# Changelog

All notable changes to MScout are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the major version is `0`, the minor version is bumped for features and the patch for fixes; anything may still change between `0.x` releases.

## [Unreleased]

### Added
- Three-state minimap detection anchored on the `WORLD` badge: expanded,
  title-bar-only, and folded.
- Folded-bar map reading: the single `Street :Map` line is matched against the
  client table in Arial Regular 12, returning both street and map.
- Shipped 1366×768 presets for Default Ratio and Default Ratio (Filter Applied).
  The three 1366×768 UI sizes share verified HUD geometry, and the saved-frame
  corpus reads every mode and all three minimap states at confidence 1.0.
- The tracked screenshot corpus now covers 1366×768 in Default, Filtered and
  Ideal, and 1920×1080 in Ideal, Default and Filtered, each with expanded,
  title-bar-only and folded minimap states. Maps include Parched Land, East
  Rocky Mountain III, and Hidden Street / Free Market <8>.
- Fixture-driven regression suite (`src-tauri/src/regression.rs`): the tracked
  corpus is replayed through the real minimap detector and `read_frame`.
  Hermetic tests check corpus integrity, minimap state and the settings-evidence
  invariant; an `--ignored` tier seeds EXP/level/identity from each frame and
  asserts the full read. `MSCOUT_CASE=<pair>` narrows a run to one pair, and a
  missing or clipped fixture fails loudly instead of being skipped.

### Fixed
- Map names ending in a roman numeral (`Thicket Around the Beach III`) could read
  as `unknown`; the matcher now breaks near-ties by rendered width and picks the
  correct numeral.
- Folding the minimap no longer forces the overlay to `unknown` permanently.
- Name/job could read as unknown after a profile switch or reseed because the
  reader re-loaded the identity atlas from a stale active-profile cache; it now
  uses the profile the engine just loaded/seeded.
- Reloading the overlay webview could leave name/job empty until the next
  profile change because the engine only emits on change; `get_profile` now
  returns the last published reading.
- The yellow status/unrecognised warning moved below the reading rows instead of
  sitting between the map and street names; `minimap collapsed` is one of those
  warnings, not the street line.
- The overlay status dot is decided in one place (`overlayState.ts`), so a
  collapsed minimap reliably shows yellow instead of falling through to green.

### Changed
- Calibration panels use one status vocabulary in both expanded and folded
  states: `<source> · <coverage>` (`auto · 10/10 digits`, `auto · 347 names`,
  `auto · Arial`, `no box`). The old `box ✓` / `name ✓ · job ✓` mixtures are gone.
- Preset geometry is no longer drawn as a calibration box. Only boxes that differ
  from the shipped preset are outlined, so a preset-only profile shows no boxes.
- Street / Map is fully detected, so its Draw/Clear box controls and preview ROI
  are removed. Its two line previews now use the detector's exact line regions
  rather than stale saved boxes.
- Calibration body order is consistent: `Read:` line, source note, preview,
  optional manual correction.
- The map line seeding now runs after the saved calibration is loaded, so legacy
  map boxes cannot overwrite the detected lines in the preview.
- Removed the unused `map_label_crop` command and the “Taught label letters”
  fallback line.
- Removed the `Fixes waiting` review queue (rejected frames, `data/review/`, and
  its Calibration drawer). The automatic atlases and the per-panel manual
  correction cover the same ground without a separate labelling queue.
- A frame whose minimap shows no name reports `unknown`; the last map is never
  carried forward. The overlay shows placeholders for the map and street, a yellow
  dot, and `minimap collapsed` as a warning at the bottom; it does not prompt
  calibration for it.
- At startup, every unread value (map, street, name, job, level, EXP %, rate, ETA,
  next) uses a dash placeholder sized to its element instead of a mix of
  `unknown` and `0.00%`. The street row and the EXP % slot are reserved from the
  first frame, so the reading rows do not move when the first sample arrives.

## [0.4.0] - 2026-11-14

### Added
- Automatic level readout: the level digits are seeded from the same client
  atlas as EXP, segmented on bright ink over the pale tiles, and stored in the
  profile's level atlas.
- Automatic character name and job from Windows Arial, with the taught
  fingerprints kept as the exact fallback.
- Automatic map/street matching against the client's own `String_000.wz` name
  table by whole-line DTW; teaching is now corrections only.
- Shipped Ideal Ratio presets for 1366×768, 1920×1080 and 1920×1200, all with
  EXP, level, name and job boxes.
- Calibration panels now share one layout: readiness/source status, a `Read:`
  line from the frozen frame, and manual corrections behind **Correct manually**.

### Changed
- Presets are geometry only and only Ideal Ratio ships. Default and Filtered
  fall back to calibration instead of applying unverified boxes; the automatic
  Arial/level/map path is Ideal-specific in this release.
- Fresh Ideal profiles auto-seed EXP/level, Arial name/job and the map table
  when the MSCW folder is configured; no per-character typing is required.
- Map/name source tracking and calibration readouts are consistent across
  corrections, profile switches and review frames.
- Documentation is now release-agnostic at the top level: the README is a stable
  entry point, while the current support matrix and Default/Filtered limits live
  in `docs/PRESETS.md` and `docs/ROADMAP.md`. Removed the internal working-notes
  session log; decisions and rendering evidence live in their reference docs.

### Fixed
- Level showing `unknown` after seeding on pale level tiles.
- Long map names clipping against a stored box; label detection now wins.
- Clean-install street matching requiring a drawn street box.
- `EurobIood` → `Euroblood` via the mixed-case `l`/`I` rule.
- The calibration level shape list flagging digits the reader had already read.
- Map/street log spam from a shared change-detection slot.

### Removed
- Shipped Default/Filtered presets. They were not verified against the current
  font/digit rendering; missing pairs now report uncalibrated rather than
  silently applying stale geometry.

## [0.3.0] - 2026-11-13

### Added
- Shipped presets for 1366×768 in Filter Applied and Ideal Ratio, and for 1920×1080 in Default Ratio. The 1920×1080 set was measured from the live games: its EXP readout sits 8 px further left than 1920×1200 implies, and its glyph atlas needed 14 variants of its own.
- `Atlas::best_split` — a shape holding two touching glyphs is cut where both halves match known glyphs and the pair explains the pixels clearly better than the shape whole. Small renderings, whether from a smaller HUD or Filter Applied's blur, leave neighbours touching; the atlas decides where the cut goes rather than a width guess, and the same rule is used by the teach and by both readers so that what is taught and what is read agree.
- Recording tools for calibration data (`record_live_map`, `record_live_labels`) that take their labels explicitly, so they work for a size with no previous set.

### Fixed
- A shape as tall as its crop — a rule or panel edge the box caught — is no longer treated as a glyph. It had been labelled `]`, shifting every label in a teach and producing a poisoned atlas.
- Two preset tests that asserted the sizes above did not exist.

## [0.2.0] - 2026-11-13

### Added
- Profiles and presets keyed by client size **and** the game's UI Size option, with a picker in Calibration. The default mode keeps the bare `1920x1200` name; the others take `@filtered` and `@ideal`.
- Shipped presets for 1920×1200 in all three UI modes and for 1366×768. A preset carries the boxes and the glyph set, including the name/job boxes, so a known size only needs its two labels typed.
- Minimap names read as text from a letter atlas taught by the same step that records a map, with fuzzy snapping and the position-aware structure of the EXP readout. An exact fingerprint takes precedence, because it cannot be mislabelled.
- Review frames: every rejected read saves its EXP box, with a panel in Calibration to label it once and turn it into a glyph.
- `repair history` flags recorded rows that break the invariants instead of deleting them; every reader skips flagged rows.
- A detected minimap-label preview in Calibration, and a `data/debug.log` trail for the teach path.

### Changed
- Validation tied to the percentage's own precision: the level derivation and the EXP/percentage cross-check now use the two-decimal floor rather than a flat 2% and 0.25 pp.
- A derived level may never decrease, the drop guard is carried across restarts, and an all-empty calibration save is refused.
- Glyph matching tolerates a one-pixel raster difference (centre-weighted comparison, proportionate size penalty), so a rendering no longer needs its own learned variant.
- The overlay sizes itself to its content, and reports each unrecognised item (map, name, job) on one line.

### Fixed
- Calibration and Target windows coming up blank and unclosable — a webview built inside the IPC handler.
- Teaches saving a window's stale box state back over boxes the profile already had, wiping them.
- The level derivation accepting truncated EXP numbers, which is what put dips in the history.
- Ideal Ratio glyphs never separating: a gradient background first, then a full-width rule, both defeating the adaptive threshold.

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

[Unreleased]: https://github.com/xinye83/mscout/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/xinye83/mscout/releases/tag/v0.4.0
[0.3.0]: https://github.com/xinye83/mscout/releases/tag/v0.3.0
[0.2.0]: https://github.com/xinye83/mscout/releases/tag/v0.2.0
[0.1.0]: https://github.com/xinye83/mscout/releases/tag/v0.1.0
