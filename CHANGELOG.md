# Changelog

All notable changes to MScout are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the major version is `0`, the minor version is bumped for features and the patch for fixes; anything may still change between `0.x` releases.

## [Unreleased]

### Added
- Every reading and diagnostic now carries per-field **read provenance**:
  `source` (the reader stage that answered — `atlas`, `font`, `fingerprint`,
  `ocr`, `table`, `dtw`, `literal`, `glyphs`, `hidden`, `unknown`, `none`),
  `engine` (`game` / `taught` / `font`, where that stage's data came from),
  `reason` (why an empty field is empty) and the read's `durationMs`. A
  `glyphs` source marks the learned minimap letter atlas, so a name it guessed
  is no longer reported as OCR text or as the rendered-name DTW.
- Calibration **Save diagnostic** writes a local JSON snapshot with the probe
  result, rejection reason, raw readout text, glyph candidates and map-line
  crops under `data/diagnostics/`; rejected probes now show their reason in the
  readout panel.

### Changed
- The testdata corpus is now image-indexed (`meta.json` version 2). Every PNG in
  a `WxH@mode` directory has an entry with its kind, source, crop and ground
  truth; `hud.png`/`settings.png` are now `hud-main.png`/`settings-panel.png`,
  focused EXP and map-label fixtures are indexed alongside the frame images, and
  the regression walker fails on any unindexed PNG. Targeted tests resolve
  fixtures by manifest id.

### Fixed
- A saved diagnostic for a **rejected** frame now records the atlas origin
  (`game` / `taught` / preset) for EXP, level and name/job. It was stamped only
  on a successful read, so the support file that exists to explain a rejection
  could not say where the digits came from. The diagnostic's stage list is also
  renamed `engine` → `stages`, so it no longer collides with the provenance's
  `engine` meaning.
- An unresolved map names the stage that gave up instead of one catch-all
  sentence: a missing folded-minimap crop, a missing MSCW install, a folded
  label that matched no client-table entry, and an expanded label with no text
  each report their own `reason`.
- The name/job provenance no longer blames a box that is set. A drawn-but-too-
  small name box used to hide a usable job box, and a frame that seeded its own
  per-line letters was reported as having "nothing to read with".
- The Calibration readout now shows `Source:` and `Why:` for a **rejected**
  frame, not only for the fields of an accepted one: the probe returns its
  provenance and rejection reason beside the (absent) readings.
- The map-table DTW fallback now resolves Roman-numeral siblings by counting the
  trailing `I` bars before comparing widths. This fixes `The Tree That Grew I`
  being read as `... III` when exact OCR is unavailable or fuzzy.
- A folded minimap is no longer hidden when a larger scenery-colored blob passes
  the weak titlebar fallback first. Folded and expanded bodies are checked across
  every candidate badge before the titlebar-only fallback is accepted.

## [0.6.0] - 2026-11-16

### Added
- 1920×1080 Default and Default (Filter Applied) presets (boxes only). All six
  1920×1080 pair/state cases and all nine 1366×768 cases pass the client-tier
  saved-frame corpus.
- Saturation-aware white-ink key for the level tile. The level seeder and reader
  try both plain luma and the key and keep the longer valid read, so Ideal is
  unchanged while the Filtered tile no longer merges with a digit.
- Windows OCR fallback for the 1920 Default/Filtered name, job and minimap
  labels. `Windows.Media.Ocr` is an OS component; no model is shipped.
- Client-table snapping and suffix-only DTW for map labels whose trailing
  `<N>` Windows OCR drops (`Free Market <8>`), for both expanded and folded
  minimaps.
- Glyph/OCR identity fusion: a confident leading glyph restores a character
  OCR dropped (`age` + glyph `p` → `Page`).
- `hud-low-percent.png` regression fixture and a test for the live
  `2,056[1.06%]` readout; the missing 1×2 period variant is now seeded.
- Live Filtered EXP fixtures `exp-21865.png`, `exp-26487.png` and
  `exp-36818.png`, a profile-seeded test for the near-miss leading digit,
  hermetic unit tests for the tolerant readout/level matchers, and a
  digit-sweep test covering `0-9` plus punctuation across the live crops.
- The level readout now uses the same ordered matcher as EXP (strict → split a
  touching pair → tolerant near miss) instead of the strict-only text path.
- EXP/level/pct are validated as a frame: a mismatch reports both EXP and level
  as unrecognised while keeping map/name/job.
- Third-party notice for mpl-ocr (MIT); its ideas are reimplemented with no
  runtime dependency. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and
  decision D31.

### Fixed
- Filtered name no longer bakes a partial GDI fit into the atlas as a wrong
  letter; partial fits go to the OCR fallback, and an uncertain line is
  reported as unknown rather than `Furoh ood`.
- Default job reads `Page` instead of `p ge`.
- Filtered folded minimap reads `Hidden Street / Free Market <8>` instead of a
  wrong map name; the suffix splitter ignores the bar's zoom controls and badge.
- Filtered EXP punctuation: a leading-zero percentage no longer loses its
  period; comma/period shapes are inferred from the readout grammar when the
  atlas drifts.
- Filtered EXP no longer drops a near-miss digit (the leading `3`, a 4×11 `1`)
  or a touching pair, so `36,818[18.90%]` reads in full instead of
  `36,88[18.90%]`/`6,818[18.90%]`. The tolerant match is gated by the existing
  level/percentage cross-check.
- Map labels now prefer an exact client-table match from OCR and accept a clear
  fuzzy winner when OCR reads one character wrong (`Deep Vallev I` →
  `Deep Valley I`). Added the live `Warning Street / Deep Valley I` minimap as
  `minimap-deep-valley.png` and a focused regression test.
- A map missing from the client's name table is now named from the OCR label
  before the learned glyph atlas. `Where the Balrogs Live` no longer becomes
  `Western Swamp`, and a one-line label reports no street. Known names still
  snap to the client table; the literal fallback is covered by resolution tests.
- An exact OCR map match now outranks the whole-line rendered DTW, which could
  read a single trailing `I` as `III`. The live `Victoria Road / The Tree That
  Grew I` minimap is `minimap-tree-1.png` with a focused regression test.
- The overlay no longer truncates long map names at 30 characters or ellipsizes
  them. The map and street rows wrap, and the existing panel-height observer
  grows the window to fit.
- When the game window is lost, the overlay status line now shows only the
  capture reason (e.g. `game window not found`). Stale `name`/`job` not
  recognised warnings from the previous reading are suppressed; a minimap
  warning no longer joins that message either.
- The 1 Hz loop was taking about 10 s per read because name/job atlases were
  re-seeded from Arial on every tick and map matching ran every tick. The
  profile's merged identity atlas is now reused, map results are cached by
  label pixels, and successful reads are emitted to the overlay immediately
  (history still waits for three good reads). A map change now appears in a
  couple of seconds instead of 10–20.
- A disagreement between the EXP readout, its percentage and the level no
  longer leaves a plausible but wrong EXP on screen while dropping only the
  level; the frame reports both as unrecognised.
- A close percentage error no longer invalidates a valid level and EXP. The
  percentage is redundant, so a gap of up to 1 pp is recomputed from
  `exp / required(level)`; a larger disagreement still rejects both. The live
  `45,458[23.83%]` read at level 36 (screen `23.33%`) keeps its level and EXP.

### Known issues
- One live Filtered `8` shape is not represented in the atlas:
  `26,487[13.58%]` reads `26,467[13.59%]`. The correct `8` scores ~29 while
  `6`/`9` score ~10–12, and the 20-EXP difference still fits the percentage
  rounding. The digit sweep records it; the filter-variant work is tracked in
  [docs/ROADMAP.md](docs/ROADMAP.md).

## [0.5.0] - 2026-11-15

### Added
- Three-state minimap detection anchored on the `WORLD` badge: expanded,
  title-bar-only, and folded.
- Folded-bar map reading: the single `Street :Map` line is matched against the
  client table in Arial Regular 12, returning both street and map.
- Shipped 1366×768 presets for Default Ratio and Default Ratio (Filter Applied).
  The three 1366×768 UI sizes share verified HUD geometry, and the saved-frame
  corpus reads every mode and all three minimap states at confidence 1.0.
- The tracked screenshot corpus covers 1366×768 in Default, Filtered and Ideal,
  and 1920×1080 in Ideal, Default and Filtered, each with expanded,
  title-bar-only and folded minimap states. The 1920 Default/Filtered fixtures
  are tracked for a later pass; their presets are not shipped in 0.5.0. Maps
  include Parched Land, East Rocky Mountain III, and Hidden Street / Free
  Market <8>.
- Fixture-driven regression suite (`src-tauri/src/regression.rs`): the tracked
  corpus is replayed through the real minimap detector and `read_frame`.
  Hermetic tests check corpus integrity, minimap state and the settings-evidence
  invariant; an `--ignored` tier seeds EXP/level/identity from each frame and
  asserts the full read. `MSCOUT_CASE=<pair>` narrows a run to one pair and
  `MSCOUT_STATE=<expanded|titlebar|folded>` to one minimap state. A fixture
  with no shipped preset is skipped as unsupported; a missing or clipped fixture
  for a supported pair fails loudly.

### Removed
- The 1920×1080 Default and Filtered presets. Those pairs are deferred past
  0.5.0; 1366×768 and 1920×1080 Ideal presets remain.
- The 1920×1200 Ideal preset. It has no saved-frame fixture or verified
  automatic read, so the pair falls back to calibration; 1366×768 and
  1920×1080 presets remain.

### Fixed
- Expanded minimap detection when the name band sits below the badge: the
  detector now uses the panel's two label lines before falling back to the
  title-bar-only state.
- Minimap state detection now recognises the 1920×1080 Default title-bar-only
  state and every Filter Applied state. The filtered path uses tolerant
  badge/chrome colours with solidity checks, and the Default title bar (white
  rather than `#99BBCC`) falls back to the `MINI MAP` text plus the dark map body.
- The map-label matcher builds its name/street index per font size and tries
  12–24 px, and the label line splitter isolates white text rows from the panel
  fill, icon and border.
- Reader improvements for the deferred 1920 Default/Filtered pairs — EXP
  punctuation and merged digits, the level digit shapes, and per-line identity
  atlases — are in place, but those presets are not shipped in 0.5.0.
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

[Unreleased]: https://github.com/xinye83/mscout/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/xinye83/mscout/releases/tag/v0.6.0
[0.5.0]: https://github.com/xinye83/mscout/releases/tag/v0.5.0
[0.4.0]: https://github.com/xinye83/mscout/releases/tag/v0.4.0
[0.3.0]: https://github.com/xinye83/mscout/releases/tag/v0.3.0
[0.2.0]: https://github.com/xinye83/mscout/releases/tag/v0.2.0
[0.1.0]: https://github.com/xinye83/mscout/releases/tag/v0.1.0
