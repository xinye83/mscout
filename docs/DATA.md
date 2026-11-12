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

### Invariants

- **No guesses.** An unreadable frame produces no row. `exp` is never fabricated.
- **No synthetic rows.** `not_recording` is represented by the absence of rows; a gap > 5 s means "not recording".
- **Monotonic within a level.** `exp` only decreases on level-up (reset) or death (penalty). Un explained drops mark the segment unreliable.
- **Cross-check.** `pct ≈ exp / required(lv) × 100`; disagreement beyond ~1 pp means the row is rejected at capture time.

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

## Compaction

- Raw 1 Hz rows are kept for 7 days.
- Older days compact to one-minute aggregates (last value per bucket, gain sum, map set).
- Aggregates are never deleted automatically.

## Mock data

`node scripts/gen-mock.mjs` writes three days of plausible rows for UI work. They are indistinguishable in format from real captures — delete `data/samples/*.jsonl` before recording real play.
