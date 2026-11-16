# Presets and supported UI sizes

MScout ships calibration **boxes only**. Digit and letter atlases are seeded from
the player's client and Windows at runtime; no game art or player data is
committed. See [SHIPPING.md](SHIPPING.md) for that boundary.

## Supported now

| Client size | UI Size | EXP | Level | Name | Job |
|---|---|---|---|---|---|
| 1366×768 | Default, Ideal, Filtered | `747,733 88×16` | `330,745 21×10` | `366,751 60×13` | `366,737 34×12` |
| 1920×1080 | Ideal | `1023,1050 90×11` | `598,1050 41×26` | `645,1064 55×9` | `645,1050 27×11` |

Every box is physical pixels relative to the game client. The map/street box is
not needed: detection follows the label wherever the minimap is.

For these pairs a fresh profile is created from the preset and reads with no
manual box drawing. EXP and level seed from `_Canvas_000.wz`; name/job seed from
Windows Arial; map/street match the client's own name table.

**1366×768 is verified in all three UI sizes.** The modes share the same HUD
geometry, and the three fixture directories `1366x768`, `1366x768@filtered` and
`1366x768@ideal` under `src-tauri/testdata/` read every minimap state at
confidence 1.0 with the shipped boxes.

**1920×1080 ships Ideal only.** The Default and Filtered presets are deferred
past 0.5.0; their fixtures remain, but the supported matrix is 1366×768 in all
three UI sizes plus 1920×1080 Ideal. The Ideal pair reads every state in the
corpus. **1920×1200 ships no preset and falls back to calibration.**

All of these presets are covered by
`preset::tests::verified_modes_ship_presets_and_the_rest_fall_back`; the
1920×1080 Ideal and the 1366×768 pairs are read by the saved-frame corpus.

## Reset and overrides

- **Reset to preset** in Calibration re-applies the preset boxes, clears the
  digit atlases, and reseeds them; taught fingerprints/identities are kept.
- A drop-in `data/presets/<W>x<H>[@mode].json` wins over the shipped copy.
- Presets are looked up in `data/presets/`, then `src-tauri/presets/`, then
  `<exe>/presets/`. See [DATA.md](DATA.md#presets).

## Deferred past 0.5.0: 1920 Default/Filtered

The 1920×1080 Default and Filtered presets are not shipped in 0.5.0. Their
fixtures and read work are deferred to a later pass; the supported matrix is
1366×768 in all three UI sizes and 1920×1080 Ideal. The 1920×1200 pairs have no
preset and still fall back to calibration.

The geometry is not the problem — the boxes above come from the live frames, and
the minimap state detector handles every pair and state. What remains is
filter-specific glyph handling and a map-label index at the Default/Filtered label
size. Manual calibration and teaching remain available, and the app does not
pretend those reads are correct. See the roadmap for the open items.
