# Presets and supported UI sizes

MScout ships calibration **boxes only**. Digit and letter atlases are seeded from
the player's client and Windows at runtime; no game art or player data is
committed. See [SHIPPING.md](SHIPPING.md) for that boundary.

## Supported now

| Client size | UI Size | EXP | Level | Name | Job |
|---|---|---|---|---|---|
| 1366×768 | Default, Ideal, Filtered | `747,733 88×16` | `330,745 21×10` | `366,751 60×13` | `366,737 34×12` |
| 1920×1080 | Default, Filtered | `1048,1036 155×18` | `461,1044 36×23` | `510,1054 110×20` | `510,1034 70×22` |
| 1920×1080 | Ideal | `1023,1050 90×11` | `598,1050 41×26` | `645,1064 55×9` | `645,1050 27×11` |
| 1920×1200 | Ideal | `1023,1170 90×11` | `598,1167 35×29` | `645,1181 96×15` | `645,1166 96×15` |

Every box is physical pixels relative to the game client. The map/street box is
not needed: detection follows the label wherever the minimap is.

For these pairs a fresh profile is created from the preset and reads with no
manual box drawing. EXP and level seed from `_Canvas_000.wz`; name/job seed from
Windows Arial; map/street match the client's own name table.

**1366×768 is verified in all three UI sizes.** The modes share the same HUD
geometry, and the three fixture directories `1366x768`, `1366x768@filtered` and
`1366x768@ideal` under `src-tauri/testdata/` read every minimap state at
confidence 1.0 with the shipped boxes.

**1920×1080 ships all three UI sizes.** Default and Filtered share the Default
geometry; their presets ship and the three fixture directories hold all three
minimap states. The Ideal pair reads every state in the corpus. On Default the
corpus now reads EXP exactly (including punctuation), the level (`35`) and the
name (`Euroblood`); the job and the map-label size are still open. The Filtered
pair reads the EXP value/percentage but loses the comma, and its level/name/job
need a filter-specific pass. **1920×1200 ships Ideal only.**

All of these presets are covered by
`preset::tests::verified_modes_ship_presets_and_the_rest_fall_back`; the
1920×1080 Ideal and (partially) Default/Filtered pairs are read by the saved-frame
corpus.

## Reset and overrides

- **Reset to preset** in Calibration re-applies the preset boxes, clears the
  digit atlases, and reseeds them; taught fingerprints/identities are kept.
- A drop-in `data/presets/<W>x<H>[@mode].json` wins over the shipped copy.
- Presets are looked up in `data/presets/`, then `src-tauri/presets/`, then
  `<exe>/presets/`. See [DATA.md](DATA.md#presets).

## Partial: 1920 Default/Filtered

The 1920×1080 Default/Filtered presets ship, but their client-tier read is not
fully verified. On the live 1920×1080 Default frame the corpus reads EXP exactly
(`127,028[72.91%]`), the level (`35`) and the name (`Euroblood`); the job (`Page`)
and the map-label size are still open. Default Ratio (Filter Applied) keeps
Default's HUD positions but changes how glyphs are drawn: the corpus loses the
EXP comma and does not yet read the level, name or job. The 1920×1200
Default/Filtered pairs have no preset and still fall back to calibration.

The geometry is not the problem — the boxes above come from the live frames, and
the minimap state detector handles every pair and state. What remains is
filter-specific glyph handling and a map-label index at the Default/Filtered label
size. Manual calibration and teaching remain available, and the app does not
pretend those reads are correct. See the roadmap for the open items.
