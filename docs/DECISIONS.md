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
**Superseded by D20** for the HUD readouts: digits are native `w×h` bitmaps now, not 12×16 cells. The per-ROI Otsu split and one-variant-per-size rule still hold.
Each ROI is binarized with Otsu, so a dark status bar and an orange level tile both work without tuning. Glyphs are segmented on empty columns and matched as native bitmaps in their own size bucket (D20).
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
Consequence: fingerprints are resolution-specific but position-independent — moving or resizing the minimap does not require re-teaching. Detection is authoritative for the fingerprint crop: a stored `map` box is only a fallback when detection fails. Preferring the box clipped a longer name and made maps with a shared prefix indistinguishable; the crop now grows with the name. Untaught maps are named from the client's `String_000.wz` table by whole-line DTW (D23).

## D18 — Settings must survive a UTF-8 BOM
`settings.json` is read with a leading BOM stripped before parsing. A BOM (written by some editors and by PowerShell's `Set-Content -Encoding UTF8`) made `serde_json` reject the whole file, which silently cleared the window selector, calibration, atlas and fingerprints — the app then reported "not calibrated" with no visible cause.

## D19 — Calibration is scoped to client size and UI Size (profiles)
A client size plus UI Size is the unit of calibration: `data/profiles/<W>x<H>[@mode].json` holds the boxes, glyphs, map fingerprints and identities for that pair. The engine activates the profile matching the live frame size and selected UI Size each tick, and the calibration commands activate the profile of the frame they are working on.
This replaces the previous "window size changed — recalibrate" dead end, where a single `settings.json` set became silently useless after a resolution or UI Size change.
Recording is gated on `VERIFY_READS = 3` consecutive good reads, so an uncalibrated or mismatched profile reports **lost** with an actionable reason and writes no rows — a wrong profile can never look like "stopped gaining EXP".
Consequence: a new machine, resolution or UI Size needs one calibration pass, then keeps its own profile. Profiles are portable between machines running the same pair. Getting a *single* calibration to serve several resolutions is a separate problem (anchor/scale normalisation) and is not solved by this.

## D20 — HUD digits are bitmap atlases; store native bitmaps
The EXP/level digits are not font renders. The client blits baked atlases from `Data/UI/_Canvas/_Canvas_000.wz` (`StatusBar3.img/mainBar/EXPBar/number`, `Basic.img/LevelNo`, …) and scales them per UI mode/resolution; `Font_000.wz` holds only the general Noto Sans KR Medium TTF and its language config. This was verified live: 5×7 at 1366 and 1920 Ideal, 8×11/9×11 at 1920×1200 Default, with the same atlas at each scale. 1920×1080 Default is a different rendering and is not an auto-supported target in 0.4.0 (D25).
Consequence: glyph atlases store the native `w×h` bitmap (`Atlas.format = 2`); legacy 12×16 cells are migrated best-effort on load. Matching tries an exact/near native bitmap in the same size bucket first, then falls back to the 12×16 distance for anti-aliased/scaled text (the map label). Shift search counts out-of-bounds as background, so a narrow shape cannot "match" by moving its ink out of the box. Presets ship boxes only; glyphs are seeded from the installed client at runtime (asking for the MSCW folder) or taught. No Nexon art is committed.

## D21 — Level uses the EXP atlas widened; punctuation is derived
`StatusBar3.img/mainBar/status/lvNumber` is a complete 10-digit level set (ink 6×9, `1` is 4×9), but it is **not** what the live Ideal HUD draws. At 1920×1080 Ideal the live level digit is 7×7 and matches the EXP `5×7` glyph stretched to 7×7 at distance 0.0; `lvNumber` scores 78.1 and `ShowLevel/0` 5.2. The level atlas is therefore seeded from the EXP set at the measured level height, so one source covers both readouts. `lvNumber` is left unused.
`StatusBar3.img/mainBar/EXPBar/number` has only `%`, `0`, `2`–`9` and `[` — its 11 nodes contain no `1`, `]`, comma or period. `1` is taken from `Basic.img/ShowLevel/0/1`, `]` is the horizontal mirror of `[`, and `,`/`.` are derived at the rendered scale. This is exact in the captured frames: from empty atlases they read `74,521[42.78%]` at 1366 Ideal, 1920 Ideal and 1920 Default.
The asset and the rendered `7` are both 5×7. Its strokes differ by about one pixel (distance ~15 on the design set), still under the 30 threshold. Seeding now self-refines: `learn_rendering` stores the exact shape of every confident, non-exact segment from the live readout, so after the first seed every segment reads at 0.0. Doubtful shapes are not learned, so a misread cannot teach the wrong character. ±1 width variants still cover scale changes.
The level reader uses bright-ink segmentation **first**, not as a fallback. On a pale tile the adaptive threshold marks the whole tile as ink, so a tile-sized shape can match a stale learned glyph and the adaptive read looks successful while returning the wrong digits (a real `35` read as `55`). Only near-white text is trustworthy there. The level atlas also seeds both the widened size and the glyph's natural-width neighbours, so a level containing a narrow `1` still matches.
Consequence: no font or extra art is needed for punctuation, and a missing node is a seeding gap the reader fills rather than a calibration failure.

## D22 — Name/job are font-seeded; map/street use the client name table
The name and job are Arial 12 px, 1-bit GDI and reproduce pixel-exact. The engine renders Arial at the measured crop height, stores the native glyphs as `identityAtlas` (`source: "font"`), and reads both lines as text. The taught `identities` fingerprints remain the exact fallback. At this size `l` and `I` are the same 1 px bar, so a mid-word bar in a mixed-case word is read as `l` (`EurobIood` → `Euroblood`); an all-caps word keeps `I`.
The minimap label is **Arial Bold 12 px** with a 1 px shadow. Seeding it per glyph fails because anti-aliasing fragments the strokes, but whole-line comparison identifies the face; D23 uses the client's name table and whole-line DTW instead of segmentation.
Consequence: name and job need no per-character teaching.

## D23 — Map/street names come from the client's map table
The two minimap lines are the client's own `mapName` and `streetName` strings in `Data/String/String_000.wz` → `Map.img/<region>/<map id>`. The current client exposes 347 map names and 11 unique streets; the count is client-version-specific and is never shipped as a list. Both saved samples are present (`VictoriaIsland/010003007` = Kerning City Civic Center / Victoria Road, `010003070` = Sunset Sky / Victoria Road).
The label is Arial Bold 12 px, so the reader renders every candidate name with GDI Arial Bold 12, keeps those whose width is within 8 px of the live line, and scores the whole line with a column-pattern DTW. The correct name scores 0.236 / 0.346 / 0.268 on the three saved lines; a 0.05 margin over the runner-up and a 0.55 cap reject ambiguous lines. No glyph segmentation is involved, which is what made per-glyph Arial seeding fail.
Taught fingerprints still win when present; the name table is the fallback for untaught maps. The same pass reads the street line.
The public map list at meowdb.com/msclassic/maps/all is larger because it includes Ossyria/El Nath and instanced/upcoming content; the client's own table is the authoritative live set and matches both samples.
Consequence: map/street no longer need teaching. The name index is rendered at 12 px, matching the Ideal HUD; a UI Size that draws the label at another pixel size needs its own rendered index (roadmap multi-resolution work).

## D24 — Shipping boundary: code/geometry/numbers only
Ship our code, UI, preset **boxes** (geometry only), the EXP table (numbers, estimated values labelled) and detection constants. Do **not** ship Nexon bitmap atlases, minimap/UI art, game fonts, a map/street name list, Arial or any system font, or player profiles.
Read digit/level atlases from `_Canvas_000.wz` and map/street names from `String_000.wz` at runtime via `mscwDir`; use the installed Windows Arial for name/job and label rendering. If the client is unavailable, degrade to typed digits/taught fingerprints and `unknown` maps; never guess.
Consequence: presets carry zero glyphs (verified), `.gitignore` excludes `fonts/` and `data/`, and no `.wz`/`.ttf` is tracked. Derived caches, if added, stay in `data/cache/` and are keyed to client/font/app versions. Rationale and the full asset table are in [SHIPPING.md](SHIPPING.md).

## D25 — Only verified Ideal presets ship; Default/Filtered fall back to calibration

0.4.0 ships three presets: 1366×768, 1920×1080 and 1920×1200 at Ideal Ratio. The old Default/Filtered preset files were removed because the current auto-read path is Ideal-specific and those boxes were not verified against the current client. A pair with no preset reports uncalibrated and accepts manual boxes/teaching; it does not silently apply geometry from another rendering.

Evidence: on the live 1920×1080 Default frame the EXP digits read but punctuation drops, the level's `3` is far from the scaled Ideal/EXP glyph (`41.7` versus `27.8` for `5`), and Arial seeding finds no matching size for name/job. 1920×1200 Default EXP reads exactly, so this needs a pair-by-pair rendering investigation, not a global "Default" switch. Filtered keeps Default's positions but changes the glyph drawing, so it needs its own seeding.

Consequence: the supported matrix is explicit in [PRESETS.md](PRESETS.md). Adding a Default/Filtered preset requires a live capture, a source/size check for digits and level, an Arial quality/size match, a map index at that label size, and a saved-frame test first.
