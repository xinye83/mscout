# Presets and supported UI sizes

MScout ships calibration **boxes only**. Digit and letter atlases are seeded from
the player's client and Windows at runtime; no game art or player data is
committed. See [SHIPPING.md](SHIPPING.md) for that boundary.

## Supported now: Ideal Ratio, three client sizes

| Client size | EXP | Level | Name | Job |
|---|---|---|---|---|
| 1366×768 | `747,733 88×16` | `330,745 21×10` | `366,751 60×13` | `366,737 34×12` |
| 1920×1080 | `1023,1050 90×11` | `598,1050 41×26` | `645,1064 55×9` | `645,1050 27×11` |
| 1920×1200 | `1023,1170 90×11` | `598,1167 35×29` | `645,1181 96×15` | `645,1166 96×15` |

All six fields are physical pixels relative to the game client. The map/street
box is not needed: detection follows the label wherever the minimap is.

For these pairs a fresh profile is created from the preset and reads with no
manual box drawing. EXP and level seed from `_Canvas_000.wz`; name/job seed from
Windows Arial; map/street match the client's own name table. The three Ideal
presets are live-verified and covered by
`preset::tests::ideal_presets_cover_the_three_supported_resolutions`.

## Reset and overrides

- **Reset to preset** in Calibration re-applies the preset boxes, clears the
  digit atlases, and reseeds them; taught fingerprints/identities are kept.
- A drop-in `data/presets/<W>x<H>[@mode].json` wins over the shipped copy.
- Presets are looked up in `data/presets/`, then `src-tauri/presets/`, then
  `<exe>/presets/`. See [DATA.md](DATA.md#presets).

## Not shipped: Default Ratio and Default Ratio (Filter Applied)

No Default/Filtered presets ship in 0.4.0. Those UI sizes fall back to normal
calibration and report uncalibrated until boxes are drawn.

The reason is rendering, not geometry. On the live 1920×1080 Default frame the
EXP digits read but punctuation drops (`74605[4282`), the level's `3` is a
different shape from the Ideal/EXP level atlas (distance 41.7; `5` is 27.8), and
Arial seeding finds no matching size for the name/job crop. Default Ratio
(Filter Applied) keeps Default's HUD positions but changes how glyphs are drawn,
so it needs its own seeding too. The 1920×1200 Default frame's EXP reads
exactly, so the gap is pair-specific, not simply "Default is broken".

To make a Default/Filtered pair supported, it needs a live capture, a
pair-specific digit/level source check, an Arial size/quality match, a
map-label index at that label size, and a saved-frame test — then a preset. Until
then, manual calibration and teaching remain available, and the app does not
pretend a preset is valid.
