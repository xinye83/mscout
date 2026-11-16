# Presets and supported UI sizes

MScout ships calibration **boxes only**. Digit and letter atlases are seeded from
the player's client and Windows at runtime; no game art or player data is
committed. See [SHIPPING.md](SHIPPING.md) for that boundary.

## Supported now

| Client size | UI Size | EXP | Level | Name | Job |
|---|---|---|---|---|---|
| 1366×768 | Default, Ideal, Filtered | `747,733 88×16` | `330,745 21×10` | `366,751 60×13` | `366,737 34×12` |
| 1920×1080 | Ideal | `1023,1050 90×11` | `598,1050 41×26` | `645,1064 55×9` | `645,1050 27×11` |
| 1920×1080 | Default, Filtered | `1048,1036 155×18` | `461,1044 36×23` | `510,1054 110×20` | `510,1034 70×22` |

Every box is physical pixels relative to the game client. The map/street box is
not needed: detection follows the label wherever the minimap is.

For these pairs a fresh profile is created from the preset and reads with no
manual box drawing. EXP and level seed from `_Canvas_000.wz`; name/job seed from
Windows Arial; map/street match the client's own name table.

**1366×768 is verified in all three UI sizes.** The modes share the same HUD
geometry, and the three fixture directories `1366x768`, `1366x768@filtered` and
`1366x768@ideal` under `src-tauri/testdata/` read every minimap state at
confidence 1.0 with the shipped boxes.

**1920×1080 is verified in all three UI sizes.** Ideal keeps its own geometry;
Default and Filtered share the measured HUD boxes and are read with the
saturation-aware level key, per-line Arial seeding plus Windows OCR fallback for
the Filtered name/job, and OCR + client-table suffix matching for the minimap
label. All six 1920×1080 pair/state combinations pass the client-tier corpus.

**1920×1200 ships no preset and falls back to calibration.**

All of these presets are covered by
`preset::tests::verified_modes_ship_presets_and_the_rest_fall_back`; the
1920×1080 pairs and the 1366×768 pairs are read by the saved-frame corpus.

## Reset and overrides

- **Reset to preset** in Calibration re-applies the preset boxes, clears the
  digit atlases, and reseeds them; taught fingerprints/identities are kept.
- A drop-in `data/presets/<W>x<H>[@mode].json` wins over the shipped copy.
- Presets are looked up in `data/presets/`, then `src-tauri/presets/`, then
  `<exe>/presets/`. See [DATA.md](DATA.md#presets).

## 1920×1200

The 1920×1200 pairs have no preset and no fixture; they fall back to manual
calibration instead of borrowing geometry from another rendering.
