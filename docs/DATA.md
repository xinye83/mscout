# Data format

Everything is local, plain text, and append-only. No database, no telemetry, no cloud.

## Location

| Context | Path |
|---|---|
| Development | `./data/` in the project root |
| Packaged | `data/` next to the executable |
| Override | `MSCOUT_DATA_DIR` environment variable |

## Samples — `data/samples/YYYY-MM-DD.jsonl`

One JSON object per line, one line per second of captured play, grouped by local date.

```json
{"t":1762800000,"lv":17,"exp":10105,"pct":91.53,"map":"Henesys","state":"gaining","conf":0.99}
```

| Field | Type | Meaning |
|---|---|---|
| `t` | integer | Unix seconds |
| `lv` | integer | Character level |
| `exp` | integer | EXP accumulated inside the current level |
| `pct` | number | EXP bar percentage as shown in game |
| `map` | string | Taught map name, or `unknown` |
| `state` | string | `gaining` or `idle` |
| `conf` | number | Parse confidence 0–1 |
| `bad` | boolean | Optional. The row is a known misread; every reader skips it |

### Invariants

- **No guesses.** An unreadable frame produces no row. `exp` is never fabricated.
- **No synthetic rows.** `not_recording` is represented by the absence of rows; a gap > 5 s means "not recording".
- **Monotonic within a level.** `exp` only decreases on level-up (reset) or death (penalty). A decrease that does not hold is rejected at capture time.
- **Levels never go backwards.** A derivation that lands below the last accepted one is a misread however self-consistent it looks; it is rejected.
- **Cross-check.** `pct ≈ exp / required(lv) × 100`, within the precision of the two-decimal percentage (about 0.02 pp for the confirmed levels 1–49, looser where the requirement is only estimated).

### Repairing recorded history

`repair_history` (the **repair history** button on the History page) marks rows that break those invariants with `bad: true`. A row is only marked when it sits below the running high-water mark *and* the recording later climbs back over it — a genuine loss of EXP never recovers, so it is left alone. Rows are flagged rather than deleted: the file stays as evidence of how often the reader failed, and a mistaken flag can be removed by hand.

## Profiles — `data/profiles/<W>x<H>[@mode].json`

Calibration is scoped to the game's **client size and UI Size**. Each pair gets one file:

```
data/profiles/1366x768@ideal.json
data/profiles/1920x1080@ideal.json
data/profiles/1920x1080@filtered.json
```

A profile for another UI Size uses the `@filtered` suffix or the bare name for
Default Ratio. 1366×768 and 1920×1080 ship presets in all three UI sizes;
1920×1200 ships none. A pair with no preset is created locally when the
player calibrates it.

Each profile holds the calibration boxes, the EXP and level glyph atlases, the Arial-seeded name/job atlas, taught map/street fingerprints and the character name/job fingerprints. The engine activates the profile matching the live frame size and UI Size on every tick, so changing either switches calibration sets instead of invalidating everything.

```json
{
  "clientW": 1366,
  "clientH": 768,
  "uiMode": "ideal",
  "calibration": { "clientW": 1366, "clientH": 768, "exp": { … }, "level": { … } },
  "atlas": { "format": 2, "source": "game", "glyphs": [ … ] },
  "levelAtlas": { "format": 2, "source": "game", "glyphs": [ … ] },
  "identityAtlas": { "format": 2, "source": "font", "glyphs": [ … ] },
  "mapAtlas": { "format": 2, "source": "taught", "glyphs": [ … ] },
  "maps": [ … ],
  "streets": [ … ],
  "identities": { "name": { … }, "job": { … } }
}
```

Each glyph is a **native bitmap** (`ch`, `w`, `h`, and `bitmap` as hex of `w×h`
bytes of `00`/`ff`), not a fixed 12×16 cell. `format: 2` marks that layout; older
profiles carrying 12×16 `cell` values are migrated best-effort on load. `source`
records where the set came from: `game` when seeded from the client atlas,
`taught` when the digits were typed, empty for a preset.

A resolution that has never been calibrated activates as an empty profile and reports **lost: `not calibrated for 1920x1080 — run calibration`**; nothing is recorded. Recording also starts only after 3 consecutive good reads, so a mismatched profile can never write plausible-looking rows.

`data/settings.json` keeps only machine-wide state: the target window, the chosen UI Size and the MSCW folder used for digit seeding (`mscwDir`).

## Settings — `data/settings.json`

Written by the window picker. All keys are optional; unknown keys are preserved.

- `targetWindow` — `{ "title": "MapleStory", "process": "MapleStory.exe" }`. Never an HWND: it is re-resolved by name every second so a game restart reattaches.
- `uiMode` — `default`, `filtered` or `ideal`; selects which profile suffix the engine loads.
- `mscwDir` — the player's `.../MapleStory/MSCW` folder. Read on demand to seed digits from `Data/UI/_Canvas/_Canvas_000.wz` and to load the map/street name table from `Data/String/String_000.wz`; no game art is copied into the repository.

Size-specific keys (`calibration`, `atlas`, `maps`, `identities`) moved to the profile files above. On first start after upgrading, they are migrated automatically into the profile for the recorded client size, and removed from `settings.json` only once that profile is safely on disk.

Bitmaps travel as hex strings; no image codec is involved. A seeded EXP atlas has 60–130 variants (one per glyph at the rendered height plus narrower/wider neighbours), so a profile is tens of kilobytes, not the old ~13 KB.

### Resolution and size

Calibration boxes, glyph cells and fingerprints are physical-pixel and UI-scale specific, which is why each client size keeps its own profile. Within a profile, map fingerprints are position-independent: the minimap panel is detected on every frame, so moving or resizing the minimap does not invalidate them. Digits need one learned variant per font size they appear in.

Sharing a profile between machines is safe when they run the game at the same client size and UI Size: copy `data/profiles/<W>x<H>[@mode].json`.

### Reading the name and job

The name and job are **Arial** (12 px, 1-bit GDI) and are seeded automatically from
the Windows face when their boxes are drawn — no teaching. The atlas stores
`source: "font"`; the one ambiguity at this size (`l` and `I` are the same 1 px
bar) is resolved by treating a mid-word bar in a mixed-case word as `l`. The
taught fingerprints remain as the exact fallback, and **Teach name/job** corrects
a misread.

The minimap label is **Arial Bold 12 px** with a 1 px shadow. Its text is the
client's own `mapName`/`streetName` table, so it is matched as a whole line rather
than segmented; see below.

### Reading the map label

The fingerprint crop is found by **detection**, not by a stored box: detection follows the whole label wherever the minimap sits, and a longer map name simply makes a wider crop. A stored map box is only the fallback for a frame where detection fails — a box drawn around a shorter name must never clip the next map's name.

The two lines are the client's own **`mapName`/`streetName` strings** in `Data/String/String_000.wz` → `Map.img/<region>/<map id>`. The current client exposes 347 map names and 11 unique streets; the count is client-version-specific and is never shipped as a list. The label is Arial Bold 12 px; the reader renders every candidate name, keeps those within 8 px of the live line's width, and scores the whole line with a column-pattern DTW. The correct name wins by a wide margin (0.236 / 0.346 on the two saved map lines, 0.268 on the street); a 0.05 runner-up margin and 0.55 cap reject ambiguous lines. No glyph segmentation is involved, which is what made per-glyph Arial seeding fail.

Taught **fingerprints** still win when they match, and a taught `mapAtlas` letter set remains as a fallback for a name not in the client table. Teaching is now only for corrections or a map missing from the table. The client table matches the saved samples exactly; the public meowdb list is larger because it includes Ossyria/El Nath and instanced/upcoming content.

The name-table match is only trusted when the best whole-line DTW score is below 0.55 and clear of the runner-up; an ambiguous line is reported `unknown` rather than guessed. The taught letter-atlas text path keeps its old rule (nearly every shape matched), and a near miss is snapped onto a name that was taught explicitly — that restores the spaces a glyph read drops (`VictoriaRoad` → `Victoria Road`) and ignores the channel marker, so `Free Market <15>` still matches on channel 16.

The label glyph set is per UI Size like every other raster, so switching mode starts its letters over.

### Minimap states

The minimap has three zoom states, and the reader handles each:

| State | What is on screen | Map name |
|---|---|---|
| Expanded | title bar + icon/name band | two-line label, Arial Bold 12 white |
| Title-bar only | title bar + map graphic, no icon/name | **not drawn — reported `unknown`** |
| Folded | one bar with `Street :Map` inside | matched against the client table |

Detection is anchored on the orange `WORLD` badge, which is present in every
state (34×10, `#DD9911` border / `#FFCC55` fill). The fill around it identifies
the state: the normal `#99BBCC` panel component is expanded at 59 px and
title-bar-only at 16 px; a light `#DDDDDD` bar is folded. The `-`/`+` button
fills corroborate the state but are not required.

The folded bar draws black text on `#DDDDDD` and joins the two fields with a
colon (`Victoria Road :Kerning City`). It is Arial Regular 12 anti-aliased, so
the reader renders `"{street} :{name}"` for every client-table pair and matches
the whole line; the winning entry supplies both fields. The folded crop is
inverted before matching because the existing matcher expects ink-on-dark.

A frame that carries no name — title-bar-only — records `map: "unknown"` with
`mapHidden: true`. The overlay shows placeholders for the map and street, a yellow
dot, and `minimap collapsed` as a warning at the bottom; it does not offer
calibration, because there is nothing on screen to read. A label that is present but fails to match
records `unknown` with `mapHidden: false`, which is the case that does warrant a
correction. The app never carries the last name forward: if the name is not on
screen, it is not assumed.

### Managing profiles

The **Calibration** window lists every saved set under *Resolution profiles*: the size currently in use is tagged `active`, the size of the live game client is tagged `live`, and a client size that has no set is flagged as needing calibration. `clear` deletes a set (behind a confirm); that size then reverts to uncalibrated and records nothing until recalibrated.

Sets never write across sizes: writing calibration, digits or labels routes through the profile for the size being worked on, so a set captured on another machine (e.g. `1366x768.json`) is left untouched and simply unused until a window of that size appears.

### UI Size

The game's Graphics → UI Size option (Default Ratio, Default Ratio (Filter Applied), Ideal Ratio) rescales the whole HUD and the font, so a set is keyed by **client size plus UI Size**, not by size alone. The default mode keeps the bare `1920x1080` name; the others take a suffix — `1920x1080@filtered`, `1920x1080@ideal`.

It is picked in the Calibration window rather than detected: guessing wrong reads nothing at all. The choice is stored as `uiMode` in `settings.json`. Switching it re-keys the engine to that mode's set; with no set for the pair the overlay reports uncalibrated and offers calibration, exactly as for an unknown client size.

The modes differ by more than rendering. **Default Ratio (Filter Applied)** keeps the HUD where Default Ratio puts it but changes how the glyphs are drawn; **Ideal Ratio** moves the HUD *and* draws a smaller font, so its boxes were measured separately (the 1920×1080 EXP box is `1023,1050,90×11` against Default's `1048,1036,155×18`). Ideal Ratio also draws a bright one-pixel rule across the status bar above the readout, which merges every glyph into one shape under an adaptive threshold — `segments` falls back to a high fixed threshold when a crop collapses to a single shape, since these readouts are near-white text.

1366×768 and 1920×1080 ship all three UI sizes, verified by the saved-frame corpus
(the modes share HUD geometry per size). 1920×1200 ships no preset and falls back
to calibration. See [PRESETS.md](PRESETS.md).

### Presets

One file per supported pair: `src-tauri/presets/<W>x<H><suffix>.json` (bare for Default, `@ideal`, `@filtered`). A preset carries the **boxes only**; the glyph set is seeded from the installed client's atlas (`source: game`) or taught, so no game asset is committed. A shipped pair auto-seeds when `mscwDir` is configured; no *Load digits* press is required. See [PRESETS.md](PRESETS.md) for the supported matrix.

Lookup order at runtime:

1. `data/presets/<W>x<H>[@mode].json` — a drop-in override, editable without rebuilding;
2. `src-tauri/presets/` — development;
3. `<exe dir>/presets/` — release; the folder is shipped through `bundle.resources` in `tauri.conf.json`.

When the engine first sees a client size with no `data/profiles/<W>x<H>[@mode].json`, it seeds one from the preset and saves it. After that the file belongs to the player: later preset updates do not change it — **reset to preset** in Calibration re-applies the preset's boxes and clears both digit atlases so the next frame reseeds them, while keeping the taught map/name/job. A pair with no preset reports **lost** and falls back to calibration. Boxes are physical pixels, so an entry applies only to the exact pair it was built for — a preset for one UI Size must never be used with another.

## Compaction

- Raw 1 Hz rows are kept for 7 days.
- Older days compact to one-minute aggregates (last value per bucket, gain sum, map set).
- Aggregates are never deleted automatically.

## Mock data

`node scripts/gen-mock.mjs` writes three days of plausible rows for UI work. They are indistinguishable in format from real captures — delete `data/samples/*.jsonl` before recording real play.
