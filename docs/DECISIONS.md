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

*Superseded by D23: map/street names now come from the client table with OCR fallbacks; taught fingerprints remain the correction path.*

## D6 — Wall-clock rate, no session splitting
Rates use trailing differences over wall-clock time. Idle time counts against the rate; the curve is continuous and never split into sessions.
Consequence: a long break visibly drags the rate down and it takes one window length to recover. This is intended.

## D7 — Windows: 1 min (solid), 5 min (faint), 30 min (ETA)
1-minute is the responsive line; 5-minute is the stable trend; 30-minute drives the ETA so the number does not flicker.
*Superseded in part by D16:* the prominent rate and the ETA now use the 5-minute average.
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
*Superseded in part by D16:* the ETA source is now the 5-minute average; the full-window rule below still applies to every window.
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

The engine passes the profile's `identityAtlas` into `read_frame` explicitly
rather than letting the reader re-load it from the active-profile cache. The
cache can lag a profile switch or a just-seeded atlas, which made name/job read
as unknown even though the profile on disk was complete.

*0.6.0 note: the 1920×1080 Default/Filtered name/job render larger and
anti-aliased. They use the same Arial face, per-line seeding, and Windows OCR
fusion; the 12 px statements above are the Ideal path.*

## D23 — Map/street names come from the client's map table
The two minimap lines are the client's own `mapName` and `streetName` strings in `Data/String/String_000.wz` → `Map.img/<region>/<map id>`. The current client exposes 347 map names and 11 unique streets; the count is client-version-specific and is never shipped as a list. Both saved samples are present (`VictoriaIsland/010003007` = Kerning City Civic Center / Victoria Road, `010003070` = Sunset Sky / Victoria Road).
The label is Arial Bold at the measured size. The reader renders every candidate name at 12–24 px, keeps those whose width is within 8 px of the live line, and scores the whole line with a column-pattern DTW. The Ideal fixtures score 0.236 / 0.346 / 0.268; a 0.05 runner-up margin and a per-size cap (0.55 below 14 px, 0.95 at 14 px and above) reject ambiguous lines. No glyph segmentation is involved, which is what made per-glyph Arial seeding fail.
Taught fingerprints still win when present; the name table is the fallback for untaught maps. The same pass reads the street line.
The public map list at meowdb.com/msclassic/maps/all is larger because it includes Ossyria/El Nath and instanced/upcoming content; the client's own table is the authoritative live set and matches both samples.
Consequence: map/street no longer need teaching. The name index is tried at 12–24 px and the Default/Filtered labels can use Windows OCR; a UI Size whose label falls outside that range would need its own rendered index.

The client's table can lag the running client: `Where the Balrogs Live` is not in the
current 347-name table. The expanded label therefore tries Windows OCR before the
learned glyph map atlas. A line with no table match is returned as the literal
on-screen name, and a one-line label has no street; known names still snap to the
table. This stops the misaligned glyph atlas from inventing a different map name
(`Western Swamp`).

An **exact** OCR match also outranks the rendered whole-line DTW. The DTW can
confuse Roman-numeral siblings (`The Tree That Grew I` → `... III`) because it
scores the whole line; the OCR union reads the final `I` exactly, so it decides
before the DTW runs. The DTW remains the fallback when OCR is absent or only
fuzzy.

## D24 — Shipping boundary: code/geometry/numbers only
Ship our code, UI, preset **boxes** (geometry only), the EXP table (numbers, estimated values labelled) and detection constants. Do **not** ship Nexon bitmap atlases, minimap/UI art, game fonts, a map/street name list, Arial or any system font, or player profiles.
Read digit/level atlases from `_Canvas_000.wz` and map/street names from `String_000.wz` at runtime via `mscwDir`; use the installed Windows Arial for name/job and label rendering. If the client is unavailable, degrade to typed digits/taught fingerprints and `unknown` maps; never guess.
Consequence: presets carry zero glyphs (verified), `.gitignore` excludes `fonts/` and `data/`, and no `.wz`/`.ttf` is tracked. Derived caches, if added, stay in `data/cache/` and are keyed to client/font/app versions. Rationale and the full asset table are in [SHIPPING.md](SHIPPING.md).

## D25 — Only verified Ideal presets ship; Default/Filtered fall back to calibration

0.4.0 ships three presets: 1366×768, 1920×1080 and 1920×1200 at Ideal Ratio. The old Default/Filtered preset files were removed because the current auto-read path is Ideal-specific and those boxes were not verified against the current client. A pair with no preset reports uncalibrated and accepts manual boxes/teaching; it does not silently apply geometry from another rendering.

Evidence: on the live 1920×1080 Default frame the EXP digits read but punctuation drops, the level's `3` is far from the scaled Ideal/EXP glyph (`41.7` versus `27.8` for `5`), and Arial seeding finds no matching size for name/job. 1920×1200 Default EXP reads exactly, so this needs a pair-by-pair rendering investigation, not a global "Default" switch. Filtered keeps Default's positions but changes the glyph drawing, so it needs its own seeding.

Consequence: the supported matrix is explicit in [PRESETS.md](PRESETS.md). Adding a Default/Filtered preset requires a live capture, a source/size check for digits and level, an Arial quality/size match, a map index at that label size, and a saved-frame test first.

*Superseded for 0.6.0 by D31: 1920×1080 Default and Filtered now ship after the per-pair verification described here.*

## D29 — 0.5.0 drops the 1920×1200 preset

*Placed next to D25, which it amends, rather than in number order.*

The 1920×1200 Ideal preset is removed. The pair has no saved-frame fixture and no
verified automatic read, so it falls back to calibration like any other unshipped
size. This supersedes the 1920×1200 part of D25; the 1366×768 and 1920×1080
presets remain.

## D26 — The minimap has three states; detect the state before reading the name

The minimap widget zooms through three states: expanded (icon + two-line name
band), title-bar-only (no name drawn at all), and folded (one light bar showing
`Street :Map` on a single line).

Detection is anchored on the orange `WORLD` badge — a fixed 34×10 chip present in
all three states — rather than on the panel fill, because the folded state has no
`#99BBCC` panel at all. The fill around the badge classifies the state: the panel
component at 59 px is expanded, the same component at 16 px is title-bar-only,
and a `#DDDDDD` bar is folded. This also fixes the old `PANEL_MIN_H = 20`
rejection, which made the 16 px title bar look like "no minimap".

The folded bar's font is Arial Regular 12, anti-aliased and inverted (black on
light), scoring 0.000 against rendered `"{street} :{name}"` candidates. Matching
the whole line returns both fields at once, so no colon detection or glyph
segmentation is involved.

State 2 draws no name, so the frame is flagged `mapHidden` and the overlay shows
placeholders for map and street, a yellow dot, and `minimap collapsed` in the
bottom warning list. It does not carry the previous map forward — a name that is
not on screen must not be assumed — and it does not suggest calibration, because
there is nothing to read.

Consequence: one detector call per frame feeds both the map and street readers;
fingerprints only run for the expanded state, since their stored-box fallback
would feed a garbage crop into the fingerprint comparison on a folded frame.
Names that differ only by a trailing roman numeral (`... Beach I/II/III`) render
3 px apart and overlap in DTW score, so the matcher breaks that tie by rendered
width.

## D27 — Calibration shows the player's edits, not preset geometry

The calibration window distinguishes the shipped preset from a box the player
drew. A box equal to the preset's box is not outlined and does not offer Clear;
the preset is starting geometry, not a calibration the player made. Drawing
anywhere replaces it and makes the box visible/clearable again.

Street / Map is detection-only. Its Draw/Clear controls and its preview ROI are
gone; the two cards show the exact line crops from the same detector the overlay
uses. The stored street/map boxes remain only as a fallback for a frame where
detection fails and for legacy taught fingerprints. Header status is one
vocabulary for every panel — `auto · 10/10 digits`, `auto · 347 names`,
`auto · Arial`, `no box` — instead of the old mix of `box ✓` and `name ✓ · job ✓`.

## D28 — A small labelled screenshot corpus may ship as test fixtures

A curated set of real HUD screenshots/crops may be committed and shipped as test
fixtures, together with a manifest of ground truth (client size, UI size, minimap
state, name/job, level, EXP text/value/percentage, street/map, expected boxes).
This is a narrow exception to D24: the corpus is test data only, is not used by the
running app, and stays small. It does not permit shipping runtime glyph atlases,
map-name lists, fonts, or other client assets.

## D30 — 0.5.0 supports 1366×768 in all three UI sizes and 1920×1080 Ideal only

The 1920×1080 Default and Filtered presets are removed for 0.5.0. The 1366×768
three modes and 1920×1080 Ideal are the supported matrix; the 1920 Default and
Filtered fixtures remain for a later pass. Resolution stays automatic from the
captured client size; the UI Size stays the manual in-app setting (no layout
autodetection in 0.5.0).

*Superseded for 0.6.0 by D31: 1920×1080 Default/Filtered are supported; UI Size
auto-detection remains deferred.*

## D31 — 0.6.0 adds 1920 Default/Filtered with selected mpl-ocr ideas, not mpl-ocr

Goal: read the 1920×1080 Default and Default (Filter Applied) pairs. The
1366×768 three modes and 1920×1080 Ideal stay the reference paths and must not
change behaviour; new readers are fallbacks or content-based choices that return
the same result on Ideal.

After 0.5.0 the remaining gaps are Default job/map and Filtered level/name/map.
We borrow three ideas from [mpl-ocr](https://github.com/Shonitbar/mpl-ocr):

1. **Saturation-aware white key** for the level box: coloured tile pixels become
   background, near-white glyph pixels survive. MScout's luma-only bright floor
   lets the Filtered tile edge merge with a digit. Seeding and reading try both
   extractions and keep the longer valid read, so an Ideal frame that already
   reads is not changed.
2. **Strict learned-glyph acceptance** for identity text: a near-miss GDI fit is
   not stored as an exact variant. An uncertain letter is reported as unknown
   instead of becoming a confidently wrong name (the Filtered `Furoh ood`).
3. **Windows OCR as a candidate source** for the Default/Filtered name, job and
   map labels, followed by the client table and the existing width/DTW decision.
   OCR reads the two label lines on the deferred pairs but drops a trailing
   `<N>`, so the client table and suffix-only DTW decide the instance and an
   ambiguous line stays `unknown`. A confident leading glyph can restore a
   character OCR dropped (`age` + glyph `p` → `Page`).

Rejected: a runtime dependency on mpl-ocr, its `%LOCALAPPDATA%` config/data
model, its global `scale = clientH / 768` layout (the 1920×1080 HUD is 1:1 while
the minimap scales), its per-ROI learned-atlas teaching workflow (game-asset
seeding is better for digits), and its tooltip/inventory/damage detectors (out
of scope).

License: mpl-ocr is MIT, Copyright (c) 2026 mpl-ocr contributors. Algorithms
and ideas are reimplemented; if source is copied, the MIT notice is retained in
that source and in [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md). MScout
stays MIT and ships no Nexon assets.

Consequence: 0.6.0 adds 1920 Default/Filtered presets and fixture coverage; the
existing Ideal and 1366 paths remain the regression baseline.

## D32 — EXP near-miss digits are accepted; the level/percentage cross-check gates them

The Filtered HUD's EXP glyphs can drift past the strict 30 threshold: the
leading `3` of the live `36,818[18.90%]` measured 31.6, and a 4×11 `1` measured
58. The old readout dropped them, producing a plausible but wrong number
(`6,818[18.90%]`, `36,88[18.90%]`) that failed the level check and was never
recorded.

The readout now tries, in order: a strict single-glyph match; punctuation
inferred from a tiny shape in a comma/period slot; a split of a touching pair
(9 px or wider, parts from 3 px); then the closest glyph up to the 65 soft-text
threshold. The level reader uses the same strict → split → tolerant order
(digits only), so a Filtered level digit is not dropped just because it drifted
past 30.

A **large** mismatch between EXP, percentage and level invalidates both the EXP
and the level for that frame; map/name/job are kept. Before this, the EXP was
kept and only the level was dropped, so a plausible wrong EXP could still be
shown. A **small** percentage gap (up to 1 pp) is repaired from the EXP and
level instead: the percentage is redundant, and the Filtered `3`/`8` shape can
read `23.33` as `23.83`. The level/percentage check is not a complete validator:
an `8`→`6` substitution changes a 5-digit EXP by ~20, which shifts the
percentage by only ~0.01 pp and still fits the rounding tolerance.

That limit is real and recorded: the live `26,487[13.58%]` still reads
`26,467[13.59%]` with both the HUD-seeded and profile atlases. The correct `8`
scores ~29 against the nearest atlas `8`, while `6`/`9` score ~10–12 at 8×11; a
simple Gaussian soft variant and another frame's seed do not close it. The
digit sweep records the gap and the roadmap tracks the filtered atlas-variant
work.

Fixtures: `1920x1080@filtered/exp-21865.png`, `exp-26487.png` and
`exp-36818.png`, seeded by the live `hud-low-percent.png` (since 0.8 under
`testdata/crops/1920x1080@filtered/`, the last renamed `exp-2056-hud.png`); hermetic tests
`ocr::tests::readout_accepts_a_near_miss_digit` and
`ocr::tests::level_reads_a_near_miss_digit`; client-tier
`game_font::tests::reads_filtered_exp_digit_sweep`.

## D33 — Job names ship as a closed vocabulary and are snapped, not transcribed

The job line is the one HUD value with a bounded domain: Classic World prints one
of a known set of strings. It is now read as recognition over that set
(`src-tauri/src/jobs.rs`) instead of being accepted as transcribed text.

An exact, punctuation-insensitive hit is taken as-is; a read within two edits of a
known name snaps to that name; anything further away is returned unchanged and
reported as unrecognised. Ties between two equally close names are rejected
rather than resolved by list order. This repairs the documented `Page` → `age`
leading-capital drop that `winocr.rs` and `choose_identity_text` patched
case-by-case, and removes the failure mode where an unconstrained read returns a
plausible wrong string with nothing to distinguish it.

What it cannot do: it does not validate the box or the segmentation. A bad crop is
still a bad crop, and a wrong job is corrected only when it lands within two edits
of a real one. The character name is open-ended and stays a transcription.

This is a deliberate, narrow exception to D24's "no map/street name list" rule. The
vocabulary is **job names only** — a closed, small, non-version-specific set of
class labels, not client content like map names, NPC text or item names, and not
rendered glyph templates. It carries no game art and no per-client strings. The
justification is that the client exposes no job-name table to read:
`map_names::tests::probe_string_jobs` scans `String_000.wz`
(StringTable/UI/Etc/NameChange/Consume), `Etc_000.wz`, `UI_000.wz`, `Skill.wz` and
`Character.wz` and finds none, unlike `Map.img`, which is read at runtime. With no
client-side source, a shipped list is the only way to get closed-set matching;
the alternative is an unbounded transcription error class.

Source: <https://meowdb.com/msclassic/classes>. Five trees — Beginner, Warrior,
Magician, Bowman, Thief — with **no Pirate** in this game; ten second-job branches
(Fighter, Page, Spearman, F/P Wizard, I/L Wizard, Cleric, Hunter, Crossbowman,
Assassin, Bandit) and ten third jobs, which are not in the initial launch and are
listed only so a future level-up reads correctly on first sight. Slash forms are
compared punctuation-insensitively, so `FP Wizard` and `F P Wizard` match the same
entry.

Maintenance: the list is client-version-specific in the same way the map table is.
A new class, a rename, or the third-job launch makes it stale, and a stale entry is
worse than a missing one because it can attract a near miss from the wrong branch.
Re-check it against the class list above when a major client update lands.

Tests: `jobs::tests` (10 cases) cover exact hits, the `age` repair, single-glyph
confusions, slash/space variants, rejection of short fragments, rejection of
distant words, rejection of classes this game does not have (Pirate, Brawler,
Buccaneer, Corsair, Noblesse, Legend, Evan, Citizen, Archer), vocabulary
uniqueness, and self-consistency of every entry.
