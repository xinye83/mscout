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

## Rejected frames — `data/review/*.json`

Whenever a read is rejected, the EXP box that produced it is saved here, so it can be labelled by hand instead of being lost. Cap at the 50 most recent; a frame that fails every second while it sits on screen is stored once.

```json
{"t":1762800000,"reason":"EXP read inconsistent (25.83% vs 26.44%)","w":152,"h":22,"px":"1f1f…"}
```

`px` is one hex byte per pixel, plain luminance. The **Review frames** panel in Calibration lists them, magnified; typing what the box shows learns those glyphs into the atlas and clears the entry. That is how a shipped glyph set gets completed — every rejection becomes training data.

## Profiles — `data/profiles/<W>x<H>.json`

Calibration is scoped to the game's **client size**. Each size gets one file:

```
data/profiles/1366x768.json
data/profiles/1920x1080.json
```

Each profile holds the calibration boxes, the glyph atlas, taught map fingerprints and the character name/job fingerprints for that size. The engine activates the profile matching the live frame size on every tick, so changing resolution switches calibration sets instead of invalidating everything.

```json
{
  "clientW": 1366,
  "clientH": 768,
  "calibration": { "clientW": 1366, "clientH": 768, "exp": { … }, "level": { … } },
  "atlas": { "cellW": 12, "cellH": 16, "glyphs": [ … ] },
  "maps": [ … ],
  "identities": { "name": { … }, "job": { … } }
}
```

A resolution that has never been calibrated activates as an empty profile and reports **lost: `not calibrated for 1920x1080 — run calibration`**; nothing is recorded. Recording also starts only after 3 consecutive good reads, so a mismatched profile can never write plausible-looking rows.

`data/settings.json` keeps only machine-wide state: the target window, rate windows and any other non-size-specific settings.

## Settings — `data/settings.json`

Written by the window picker. All keys are optional; unknown keys are preserved.

- `targetWindow` — `{ "title": "MapleStory", "process": "MapleStory.exe" }`. Never an HWND: it is re-resolved by name every second so a game restart reattaches.

Size-specific keys (`calibration`, `atlas`, `maps`, `identities`) moved to the profile files above. On first start after upgrading, they are migrated automatically into the profile for the recorded client size, and removed from `settings.json` only once that profile is safely on disk.

Bitmaps travel as hex strings; no image codec is involved. The whole file is ~13 KB with one map and 16 glyph variants.

### Resolution and size

Calibration boxes, glyph cells and fingerprints are physical-pixel and UI-scale specific, which is why each client size keeps its own profile. Within a profile, map fingerprints are position-independent: the minimap panel is detected on every frame, so moving or resizing the minimap does not invalidate them. Digits need one learned variant per font size they appear in.

Sharing a profile between machines is safe when they run the game at the same client size: copy `data/profiles/<W>x<H>.json`.

### Reading the map label

The minimap label is read as **text**, not matched as a picture. Its font is a separate glyph set (`mapAtlas`), and it is taught by the same step that records a map: type the two lines the panel shows and every letter in them is learned. Teaching five to eight maps therefore covers the alphabet, and from then on new maps read with no teaching at all — which is what makes a shipped preset able to name maps it has never seen.

A read is only trusted when nearly every shape matched and the label lines were found; a partly-read label would name the wrong map, so the engine falls back to the taught fingerprints and reports `unknown` rather than guess. A near miss is snapped onto a name that was taught explicitly — that restores the spaces a glyph read drops (`VictoriaRoad` → `Victoria Road`) and ignores the channel marker, so `Free Market <15>` still matches on channel 16.

The label glyph set is per UI Size like every other raster, so switching mode starts its letters over.

### Managing profiles

The **Calibration** window lists every saved set under *Resolution profiles*: the size currently in use is tagged `active`, the size of the live game client is tagged `live`, and a client size that has no set is flagged as needing calibration. `clear` deletes a set (behind a confirm); that size then reverts to uncalibrated and records nothing until recalibrated.

Sets never write across sizes: writing calibration, digits or labels routes through the profile for the size being worked on, so a set captured on another machine (e.g. `1366x768.json`) is left untouched and simply unused until a window of that size appears.

### UI Size

The game's Graphics → UI Size option (Default Ratio, Default Ratio (Filter Applied), Ideal Ratio) rescales the whole HUD and the font, so a set is keyed by **client size plus UI Size**, not by size alone. The default mode keeps the bare `1920x1200` name; the others take a suffix — `1920x1200@filtered`, `1920x1200@ideal`.

It is picked in the Calibration window rather than detected: guessing wrong reads nothing at all. The choice is stored as `uiMode` in `settings.json`. Switching it re-keys the engine to that mode's set; with no set for the pair the overlay reports uncalibrated and offers calibration, exactly as for an unknown client size.

The modes differ by more than rendering. **Default Ratio (Filter Applied)** keeps the HUD where Default Ratio puts it but changes how the glyphs are drawn; **Ideal Ratio** moves the HUD *and* draws a smaller font, so its boxes were measured separately (the EXP box is `1023,1170,90×11` against the default's `1063,1148,152×22`). Ideal Ratio also draws a bright one-pixel rule across the status bar above the readout, which merges every glyph into one shape under an adaptive threshold — `segments` falls back to a high fixed threshold when a crop collapses to a single shape, since these readouts are near-white text.

### Presets

One file per known **client size × UI Size**: `src-tauri/presets/<W>x<H>[@mode].json`. A preset carries the **boxes and the glyph set**, so a shipped pair reads with no calibration at all.

Lookup order at runtime:

1. `data/presets/<W>x<H>[@mode].json` — a drop-in override, editable without rebuilding;
2. `src-tauri/presets/` — development;
3. `<exe dir>/presets/` — release; the folder is shipped through `bundle.resources` in `tauri.conf.json`.

When the engine first sees a client size with no `data/profiles/<W>x<H>[@mode].json`, it seeds one from the preset and saves it. After that the file belongs to the player: later preset updates do not change it — **reset to preset** in Calibration re-applies the preset's boxes and glyphs while keeping the taught map/name/job. A pair with no preset reports **lost** and falls back to calibration. Boxes are physical pixels and glyphs are rasters of the rendered font, so an entry applies only to the exact pair it was built for — a preset for one UI Size must never be used with another.

## Compaction

- Raw 1 Hz rows are kept for 7 days.
- Older days compact to one-minute aggregates (last value per bucket, gain sum, map set).
- Aggregates are never deleted automatically.

## Mock data

`node scripts/gen-mock.mjs` writes three days of plausible rows for UI work. They are indistinguishable in format from real captures — delete `data/samples/*.jsonl` before recording real play.
