# Decisions

Append-only. Each entry records what was decided and why, so future sessions do not relitigate settled questions. Date = when decided.

## D1 — Target Classic World, not original MapleStory
Classic World is its own game: level cap 100, four jobs (Warrior/Magician/Bowman/Thief), 2nd job ceiling at launch, no Holy Symbol EXP buff, and a custom damage formula that is **not** the old `(STR×4 + DEX) × WATK / 100`.
Consequence: all formulas and tables come from Classic World sources; old GMS/v.62 numbers are wrong here.

## D2 — Read-only, no game-process interaction
No `ReadProcessMemory`, injection, D3D/API hooks, packet capture, or `SendInput`. Screen capture and window compositing only.
Consequence: exclusive fullscreen cannot be overlaid; the game must run windowed or borderless. This is a hard constraint, not a limitation to work around.

## D3 — Tauri + Rust core, not Electron
Electron was the initial plan (fastest UI iteration), but Rust became available. Tauri gives the web UI ergonomics (React + uPlot) with native capture and a single-process footprint (~35 MB observed vs ~150–250 MB for Electron).
Consequence: build requires Rust + MSVC + Windows SDK; frontend still uses Vite/React.

## D4 — Build capture ourselves; do not depend on mpl-ocr
`mpl-ocr` is an MIT Rust project that already reads Classic World's screen state accurately (WGC + Windows OCR + glyph atlases). It was evaluated and rejected: Rust-only, prototype maturity, and it writes config to `%LOCALAPPDATA%`.
Consequence: we own the capture and matching code; nothing third-party runs alongside the game.

## D5 — Map names by bitmap fingerprint, not OCR
The map label is pixel-identical for the same map at the same resolution. Electron/WebView has no access to `Windows.Media.Ocr`, and Tesseract on a 12 px stylized font is poor.
Decision: teach each map once (store the ROI bitmap), then nearest-match.
Consequence: maps must be taught; templates are resolution/scale specific; unknown maps report `unknown`, never a guessed name.

## D6 — Wall-clock rate, no session splitting
Rates use trailing differences over wall-clock time. Idle time counts against the rate; the curve is continuous and never split into sessions.
Consequence: a long break visibly drags the rate down and it takes one window length to recover. This is intended.

## D7 — Windows: 1 min (solid), 5 min (faint), 30 min (ETA)
1-minute is the responsive line; 5-minute is the stable trend; 30-minute drives the ETA so the number does not flicker.
Consequence: ETA shows `—` until 30 minutes of clean data exist.

## D8 — Idle and not-recording are different states
`gaining` and `idle` have samples; `not_recording` is the absence of samples. Gaps are never interpolated and never counted as active time in history summaries.
Consequence: "I played badly" and "the tool broke" are visually and numerically distinguishable.

## D9 — History is a separate window, read-only
The overlay is glanceable and click-through; history is studied with a mouse on a normal window. The JSONL files remain the source of truth.
Consequence: two windows, one data path. No editing or annotating from the UI.

## D10 — JSONL storage, daily files
`data/samples/YYYY-MM-DD.jsonl`, one row per second. Raw kept 7 days, then compacted to 1-minute aggregates.
Consequence: trivial to inspect and append; large files handled by compaction rather than a database.

## D11 — Data stays inside the project (user requirement)
All app data under `./data/`; npm downloads were explicitly allowed to use the default AppData cache.
Consequence: packaged builds write `data/` next to the exe; `MSCOUT_DATA_DIR` overrides.

## D12 — Attach by title + process, never by HWND
`EnumWindows` lists visible, titled, non-tool windows and the player picks one. The selector is stored as `targetWindow: { title, process }`; the HWND is never persisted. A 1 Hz tracker thread re-resolves the selector, so a game restart, move or resize reattaches without user action.
Match priority: title+process > process alone > title alone; ties go to the largest client area. Process name is read with `PROCESS_QUERY_LIMITED_INFORMATION` (no handle into the game).
Consequence: no game-process interaction beyond reading its window title and exe name; a renamed executable needs re-selection.

## D13 — WGC frames are cropped with DWM extended frame bounds
`Direct3D11CaptureFramePool::CreateFreeThreaded` gives a CPU-readable frame; `GetClientRect` alone is wrong because `GetWindowRect` includes DWM's invisible resize borders (7–8 px), while the captured frame matches the visible bounds. The client origin is therefore offset by `ClientToScreen` against `DWMWA_EXTENDED_FRAME_BOUNDS`.
`SetIsBorderRequired(false)` and cursor capture are disabled best-effort. Uniform (blank) first frames are skipped, and the previous frame is reused when the window is static so a still screen is not reported as lost.
Consequence: ROIs are stored in physical client pixels and are correct for windowed/borderless play; exclusive fullscreen remains unsupported.

## D14 — Glyph cells are thresholded per ROI, with size variants
Each ROI is binarized with Otsu, so a dark status bar and an orange level tile both work without tuning. Glyphs are segmented on empty columns and normalized to 12×16; matching combines cell distance with a penalty for bounding-box size.
The same character is stored once per size: EXP, level and HP/MP use different font sizes, and a single entry per character would be overwritten by whichever was learned last.
Consequence: learning is per-resolution and per-font-size; coverage is reported per character, so a digit visually present only at another size still needs a variant from the target ROI.

## D15 — ETA requires a full 30-minute baseline and is hidden while idle
`rateOver` now requires a sample at or before `now − window`, so the 30-minute rate is not computed from a few seconds of data at session start. While there has been no gain for two minutes, the ETA is shown as `—` rather than climbing as the 30-minute rate decays.
Consequence: the ETA can be blank for the first half hour and during breaks; rates and the chart still show the wall-clock decay, which is intentional (D6).

## D16 — Prominent rate and ETA use the 5-minute average
The collapsed panel's EXP/hour and time-to-level are both derived from the trailing 5-minute rate. They show `—` until the current session has lived a full 5 minutes, whenever the 5-minute average is zero (or rounds to zero), and the ETA also stays `—` after 2 minutes without a gain. The 1- and 30-minute rates stay in the expanded panel as context.
Consequence: a restart starts blank for 5 minutes; idle decay is visible in the numbers until they reach zero, then they revert to `—`. This supersedes the 30-minute ETA source in D7/D15.

## D17 — Map labels are two lines; the label box is detected, not stored
The minimap prints the map name on one or two lines depending on its length (`Kerning City` is one, `Hidden Street` / `1st Accompaniment <Bonus>` is two), and the stored label keeps the newline so the overlay renders it exactly as the game does.
The minimap can be moved and resized by the player, so its position cannot be a stored box. Each frame, the panel is found by connected-component search for its solid `#99BBCC` fill (largest component that is a mostly-solid rectangle of panel proportions), the white name text is bounded below the title bar and right of the icon, and that crop is resampled to a fixed 128×24 grid before matching.
Consequence: fingerprints are resolution-specific but position-independent — moving or resizing the minimap does not require re-teaching, and the calibrated `map` box is now only a fallback for when detection fails.

## D18 — Settings must survive a UTF-8 BOM
`settings.json` is read with a leading BOM stripped before parsing. A BOM (written by some editors and by PowerShell's `Set-Content -Encoding UTF8`) made `serde_json` reject the whole file, which silently cleared the window selector, calibration, atlas and fingerprints — the app then reported "not calibrated" with no visible cause.

## D19 — Calibration is scoped to client size (profiles)
A client size is the unit of calibration: `data/profiles/<W>x<H>.json` holds the boxes, glyphs, map fingerprints and identities for that size. The engine activates the profile matching the live frame size each tick, and the calibration commands activate the profile of the frame they are working on.
This replaces the previous "window size changed — recalibrate" dead end, where a single `settings.json` set became silently useless after a resolution change.
Recording is gated on `VERIFY_READS = 3` consecutive good reads, so an uncalibrated or mismatched profile reports **lost** with an actionable reason and writes no rows — a wrong profile can never look like "stopped gaining EXP".
Consequence: a new machine or resolution needs one calibration pass, then keeps its own profile. Profiles are portable between machines running the same client size. Getting a *single* calibration to serve several resolutions is a separate problem (anchor/scale normalisation) and is not solved by this.
