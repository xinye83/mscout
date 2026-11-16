# Shipping boundary — app assets vs local client files

The rule: **ship behaviour, geometry and game numbers; never ship game art,
fonts, map content, or player data.** Proprietary or version-specific content is
read from the player's MapleStory install at runtime; anything player-specific
stays in `data/`.

## Shipped with the app

| Item | Why it is safe to ship | Notes |
|---|---|---|
| Rust/React executable, UI, icons | our code and art | — |
| Preset calibration boxes (`src-tauri/presets/*.json`) | measured **geometry** only | Every preset has `expGlyphs = levelGlyphs = mapGlyphs = 0`, no maps/identities. Shipped pairs: **1366×768 and 1920×1080 at Default, Ideal and Filtered**. 1920×1200 has no preset and falls back to calibration. See [PRESETS.md](PRESETS.md). |
| EXP table | game **numbers**, not art | `exp_table.rs` / `expTable.ts`; 1–49 confirmed, 50–89 estimated and labelled, 90–100 anchored. |
| Detection constants and rate math | our logic | minimap panel colour/proportions, ROI rules, rate windows. |
| Docs, tray/overlay assets | ours | — |

Nothing else is bundled: no glyph atlases, no map-name list, no fonts, no
fingerprints.

## Read from the local client (`mscwDir`)

| Client path | Used for | Why it is read at runtime |
|---|---|---|
| `Data/UI/_Canvas/_Canvas_000.wz` → `EXPBar/number`, `ShowLevel/0`, `status/lvNumber` | EXP/level digit atlases | Nexon bitmap art; not redistributable. Seeded into the player's profile at runtime. |
| `Data/String/String_000.wz` → `Map.img/<region>/<map id>/mapName`, `streetName` | map and street names (347 / 11 in the current client; count varies) | Client content and version-specific; rendered with Arial Bold 12 at runtime. Not shipped as a list. |
| `Data/UI/Font/Font_000.wz` → Noto Sans KR Medium | general/Korean text font | **Not used** by MScout: the game UI is English and name/job/map are Arial. OFL, so it could be shipped, but there is no need. |
| Client version, resolution, UI Size | profile selection | Detected/selected, not a file to copy. |

The calibration window asks for the folder once (`mscwDir`, native folder picker).
No game file is copied into the repository or into the app bundle.

**If the client files are missing:** the app falls back to typed digit learning,
taught map/street fingerprints and taught letter sets, and reports the map as
`unknown`. It never guesses.

## Read from Windows

| Asset | Used for | Why not shipped |
|---|---|---|
| **Arial Regular 12 px** (1-bit GDI) | character name / job (all 1366×768 UI sizes; Ideal at 1920×1080/1200) | Proprietary system font; loaded from Windows at runtime, never bundled. 1920 Default/Filtered are not auto-supported. |
| **Arial Bold 12 px** (anti-aliased GDI) | map / street label rendering for the Ideal name-table match | Same. |
| WGC / hotkey / shell APIs | capture, hotkeys, folder picker | OS. |

If Arial is unavailable, font seeding fails gracefully to the taught
fingerprints/letters; the app does not download or install fonts.

## Local-only (`data/`, never shipped)

- `data/profiles/<W>x<H>[@mode].json` — boxes, EXP/level/identity atlases
  (seeded from the client/Arial), taught map/street fingerprints, identities.
  Player-specific and may contain derived Nexon/Arial glyphs.
- `data/samples/`, `data/map-samples/`, logs, backups.
- `data/fonts/` — extracted Noto TTF if used for diagnostics.
- `data/settings.json` — target window, UI Size, `mscwDir`.
- `fonts/` — local reference copies of `arial.ttf` and `Font_000.wz`
  (gitignored).
- Optional caches (parsed map names / rendered label index) — derived, local,
  keyed to the client and font versions.

`data/` and `fonts/` are gitignored because they are machine-specific and may
contain redistributable-restricted content.

## Test fixtures (shipped as test data only)

- A small, curated corpus of real HUD screenshots/crops with a ground-truth
  manifest may ship under the test fixtures (D28). It is used only by tests, is
  not loaded by the running app, and does not include runtime atlases, fonts or
  name lists.

## Never shipped

- Nexon bitmap atlases/glyphs, minimap/UI art, game fonts as runtime assets. The
  narrow test-fixture exception is D28.
- Arial / Arial Bold (proprietary), Nexon brand fonts.
- A generated map/street name list or rendered name templates.
- Player profiles, fingerprints, samples.
- The extracted Noto TTF, unless a future CJK feature needs it (it is OFL, but
  the English UI does not).

## Current compliance

- `.gitignore` covers `data/` and `fonts/`; `git ls-files` contains no `.wz`,
  `.ttf`, `.otf`, `.bmp` or `.jsonl` game/data files (only our icons).
- All shipped presets were checked: zero glyphs in `atlas`, `levelAtlas`,
  `mapAtlas`; no maps or identities.
  `preset::tests::shipped_presets_contain_no_game_art` fails the build if a
  preset ever gains glyph/map/identity content.
- Verified presets ship for 1366×768 and 1920×1080 in all three UI sizes.
  1920×1200 has no preset; a missing pair reports uncalibrated instead of
  applying a stale preset.
- `map_names.rs` reads `String_000.wz` at runtime; `game_font.rs` reads
  `_Canvas_000.wz`; `font_seed` renders Windows Arial; no font or map list is
  bundled.
- Seeded atlases live in the player's profile under `data/`.
- Clean-start proof: with no profiles, atlases or preset overrides, the shipped
  presets (boxes only) + `_Canvas_000.wz` + `String_000.wz` + Windows Arial read
  the saved frames exactly (level, EXP, name, job, map, street): 1366×768 in all
  three UI sizes and 1920×1080 Ideal. No local override is needed for a known
  size.

## Recommendations / open decisions

1. **Cache the derived map name list and rendered Arial index** in
   `data/cache/`, keyed by `String_000.wz` size+mtime, the Windows Arial
   version/size, renderer px, and app version. Startup currently rebuilds the
   client-name index; a cache is optional.
2. **If offline map naming is wanted** after profiles are seeded, cache the
   derived name list locally. Digits continue from the profile; only the map
   table needs the client. It is derived data, not a shipped asset.
3. **Auto-discover the MSCW folder** from the Steam library, with `mscwDir`
   override. No data is copied.
4. **Keep the EXP table shipped** (numbers only; the client does not expose it
   cleanly).
5. **For a UI Size whose label is not 12 px**, build a size-specific rendered
   index at runtime; do not ship a static rendered set.
6. **Preset updates** are geometry only; drop-in overrides stay in
   `data/presets/`.
