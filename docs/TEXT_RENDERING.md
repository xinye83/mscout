# Text rendering, fonts, and HUD glyph atlases

How MapleStory Classic World (MSCW) draws the text MScout reads, where the font
and glyph assets live, and what was verified against the live client.

This supersedes the earlier "the UI is Arial" summary. The client uses **three
different mechanisms**, and only one of them is a font render at all.

## Summary

| What MScout reads | How the client draws it | Source | Reproducible? |
|---|---|---|---|
| EXP digits and `%` | Baked bitmap atlas, scaled per UI mode/resolution | `Data/UI/_Canvas/_Canvas_000.wz` → `StatusBar3.img/mainBar/EXPBar/number` | Yes — native-bitmap match |
| Level digits | Baked bitmap atlas, or the EXP set widened | `Basic.img/LevelNo` (11×13, old UI) or `StatusBar3.img/mainBar/status/lvNumber` (7×10); the live level is the EXP 5×7 glyph widened to 7×7 | Yes |
| Character name / job | Arial 12 px, 1-bit (GDI) | Windows `arial.ttf` | Yes — pixel-exact, auto-seeded |
| Map / street label | **Arial Bold 12 px**, anti-aliased + 1 px shadow | Windows Arial + `String_000.wz` map table | Yes — name table + whole-line DTW |

The correction that matters: **the EXP and level digits are not font renders.**
They are pre-baked bitmaps blitted at a scale that depends on the resolution and
the UI setting. No font file reproduces them.

---

## `Font_000.wz` — the client's general text font

Path in the game folder: `Data/UI/Font/`

| File | Size | Role |
|---|---|---|
| `Font_000.wz` | 6,189,565 B | The payload |
| `Font.wz` | 63 B | PKG1 index stub (no data) |
| `Font.ini` | 15 B | `LastWzIndex|0` |

The WZ header is standard PKG1: `PKG1`, declared size 6,189,505, header size 60,
`Package file v1.0 Copyright 2002 Wizet, ZMS`, data starts at offset 76. It parses
with the `CLASSIC` key (see [Reproducing](#reproducing)).

It contains exactly two images:

### `LanguageFont.img` — configuration, not glyphs

```
LanguageFont.img
  0  (English)   Fonts/0/Path = "NotoSansKRMedium"
  1  (Spanish)   Fonts/0/Path = "NotoSansKRMedium"
  2  (Korean)    Fonts/ is empty
  fallback
    fonts/notokr
      fontName      = "Noto Sans KR Medium"
      scales/all    : orig 16, scaled 15
    sets/all
      replaces/0
        fallback    = "notokr"
        charRanges/0 = "all"
```

### `NotoSansKRMedium.img` — one raw TrueType font

The image has a single property, `FONT_DATA`, of the WZ extended type
`RawData` (the stock `@tybys/wz` reader does not know it; the handler had to be
added). It is **6,188,887 bytes** and begins with a 7-byte wrapper; at offset 7
there is a valid TrueType font:

- 19 tables (`BASE`, `GDEF`, `GPOS`, `GSUB`, `OS/2`, `STAT`, `cmap`, `gasp`,
  `glyf`, `head`, `hhea`, `hmtx`, `loca`, `maxp`, `name`, `post`, `prep`,
  `vhea`, `vmtx`), `glyf` alone 5,710,579 B.
- `name` table: family **Noto Sans KR Medium**, subfamily Regular, PostScript
  `NotoSansKR-Medium`.
- 24,868 glyphs, no embedded bitmap strikes (`EBDT`/`EBLC` absent).

So the game ships a real, OFL-licensed TTF. **There are no ASCII or digit glyph
atlases in `Font_000.wz`** beyond this font.

---

## HUD digit atlases — `_Canvas_000.wz`

The digit glyphs MScout needs are baked sprites in
`Data/UI/_Canvas/_Canvas_000.wz` (35 MB), not text renders. The relevant sets:

| Image / path | Canvas | Role |
|---|---|---|
| `Basic.img/ItemNo` | 8×11 (`1` is 5×11) | **Old** status-bar digits |
| `Basic.img/LevelNo` | 11×13, plus `blank` | Level tiles (old status bar) |
| `Basic.img/ShowLevel/0..4` | 5×12 | Level display variants |
| `StatusBar.img/gauge/number2`, `number3` | 6×7 | Old gauge digits (anti-aliased) |
| `StatusBar.img/gauge/number/%` | 7×7 | Percent sign (matches the live frame exactly) |
| `StatusBar3.img/mainBar/status/lvNumber` | 7×10 | Level digits (new UI) |
| `StatusBar3.img/mainBar/EXPBar/number` | 7×9 (5×7 content) | **Current EXP digits**, `[`, `%` |
| `StatusBar3.img/mainBar/status/gauge/number` | 7×9 | Gauge digits |
| `Util.img/number/0` | 6×8; siblings 21–26×13 | Large UI numbers |

Key details:

- **`StatusBar3.img/mainBar/EXPBar/number` is the real EXP source** for the
  current UI. It has exactly 11 nodes: `0`, `2`–`9`, `[`, `%`. There is **no
  `1`, `]`, comma or period anywhere in the set** — the archive simply does not
  contain them. `1` is taken from `Basic.img/ShowLevel/0/1`, `]` is the mirror
  of `[`, and `,`/`.` are derived at the rendered scale.
- `StatusBar3.img/mainBar/status/lvNumber` is a complete 10-digit set (ink
  6×9; `1` is 4×9), but it is **not** the live Ideal level. The live 7×7 digit
  matches the EXP glyph widened, not `lvNumber` (see below).
- The `%` in that set is 7×7 and matches the live frame pixel-for-pixel.
- `Basic.img/ItemNo` is the **old** status bar. It only coincidentally matched a
  few digits; do not use it for the current UI.
- The set has per-resolution layout siblings (`800`, `1024`, `1280`, `1366`,
  `1920`), which confirms the client scales the same UI assets per setting.

---

## Verification against the live client

Method: capture a raw client frame (`dumps_exp_crop`, `MSCOUT_FRAME=…`), crop the
shipped preset box, Otsu-threshold, segment on empty columns, and compare each
segmented bitmap to the atlas PNGs (nearest/bilinear/lanczos fit).

| Setting | EXP glyphs | Result vs `EXPBar/number` |
|---|---|---|
| 1920×1200 @ Ideal | 5×7 | Exact for `4`, `5`, `2`, `[`, `4`, `2`, `8`, `%`; `7` became exact after self-refinement |
| 1920×1200 Default | 8×11 / 9×11 | Same atlas scaled up ~1.6× (IoU 0.76–0.89). `ItemNo` scored 0.36–0.80 and was ruled out |
| 1366×768 @ Ideal | 5×7 | Exact for every digit; seeding also learns the live rendering, so `7` lands at distance 0 |
| 1920×1080 @ Ideal | 5×7 | Live preset + seed reads `74,521[42.78%]` exactly |

From-empty-atlas proof: `reseed_reads_all_captured_frames` seeds each profile from
scratch from `_Canvas_000.wz` and reads EXP on the captured frames —
`74,521[42.78%]` at 1366 Ideal, 1920 Ideal, 1920×1200 Default and 1920×1080
Ideal. The Ideal level boxes read `35` at 7×7. The 1920×1200 Default level is
outside that supported proof.

Level source, measured on the 1920×1080 Ideal frame. The live level digit `5`
was compared with every client set scaled to the live 7×7:

| Source | Native ink | Distance to live |
|---|---|---|
| `EXPBar/number/5` | 5×7 | **0.0** |
| `Basic.img/ShowLevel/0/5` | 4×7 | 5.2 |
| `status/lvNumber/5` | 6×9 | 78.1 |
| `Basic.img/ItemNo/5` | 6×9 | 72.9 |

So the level is the **EXP glyph widened**, confirmed at 0 distance. `lvNumber`
is a complete 10-digit set but belongs to a different HUD design; do not seed
from it.

Conclusions:

- The EXP readout is the **same atlas at a per-setting scale**. Default is not a
  different font; it is the ideal design scaled up, which is why its strokes get
  thicker.
- **The rendered glyph size is the right atlas key** (5×7 at both ideal
  resolutions, 8×11 at default), not the resolution. A single glyph set can serve
  every profile that renders at that size.
- **Seeding refines itself against the live frame.** After loading the design set,
  `learn_rendering` stores the exact shape of every confident, non-exact readout
  segment. The Ideal `7` is one stroke off the asset (distance ~15); after the
  first seed it is an exact variant and every segment reads at 0.0. A doubtful
  shape is left alone rather than learned as the wrong character.

Name and job were verified separately: `Euroblood` and `Page` are reproduced
**pixel-for-pixel** by Arial Regular 12 px with GDI
`SingleBitPerPixelGridFit` (IoU 1.000 on every glyph). They are now auto-seeded
from the Windows Arial face: the live crops read `Page` and `Euroblood` with
every segment at distance 0.0 (after resolving the `l`/`I` bar ambiguity).

The minimap label is **Arial Bold 12 px**. A per-glyph Arial seeding attempt
failed (Regular ~48, Bold ~37 mean distance) because the anti-aliased/shadowed
line fragments into stroke pieces under column segmentation. The font was
identified by comparing the saved whole lines against rendered whole strings with
a column-pattern DTW: Arial Bold 12 scores 0.2–0.3, every other face 0.4+.

The label text itself is the client's own `mapName`/`streetName` table in
`String_000.wz/Map.img` (347 map names and 11 unique streets in the current
client; the count is client-version-specific), so the
reader matches whole rendered names instead of OCR. See
[Map and street names](#map-and-street-names).

---

## What this means for MScout

1. **Do not generate digit templates from fonts.** The HUD digits are a bitmap
   atlas. Arial, the embedded Noto TTF, and the Nexon brand fonts all fail on
   the small glyphs.
2. **Native bitmaps are the stored format.** `Atlas.format = 2` keeps each
   shape's `w×h` mask and matches it exactly (with a small shift) when the size
   bucket agrees. The old 12×16 cell is now only the fallback for
   anti-aliased/scaled text (the map label). Legacy cells are migrated
   best-effort on load.
3. **Shift matching counts out-of-bounds as background.** Skipping those pixels
   let a 2×7 `[` "match" a `1` by shifting its ink out of the box; the live
   round-trip read `74,521142.78%1` until this was fixed. It now reads
   `74,521[42.78%]` at distance 0 on 1366 Ideal, 1920 Ideal and 1920×1200 Default.
   0.5.0 ships 1366×768 in all three UI sizes and 1920×1080 Ideal; the
   1920 Default/Filtered pairs are deferred.
4. **The level reader tries bright ink first.** On a pale tile the adaptive
   threshold marks the whole tile as ink; with a stale tile-sized glyph in the
   atlas, the adaptive read looked successful and returned `55` for a real `35`.
   `recognize_level` only trusts near-white text, where the 7×7 digits segment
   cleanly, and falls back to the adaptive path only if that finds nothing.
5. **Key atlases by glyph size, not resolution.** 5×7 covers 1366 and 1920
   Ideal; the 1366 Default/Filtered pairs render at their own sizes and are
   supported. The 1920 Default/Filtered pairs are deferred.
6. **Seeding is implemented.** The calibration window asks for the MSCW folder
   (`mscwDir` in `settings.json`, via a native folder picker). On freeze or
   “Load digits” the app reads `EXPBar/number`, scales it to the live glyph
   height, fills `1` from `ShowLevel/0/1`, mirrors `]`, and derives `,`/`.`. The
   profile stores `source: "game"`; taught corrections remain as overrides. Do
   **not** bundle extracted Nexon art.
7. **The EXP atlas is incomplete; seeding fills the gaps.** `EXPBar/number` has
   no `1` or `]` and no punctuation anywhere in its 11 nodes. Seeding takes `1`
   from `Basic.img/ShowLevel/0/1`, mirrors `]` from `[`, and derives `,`/`.` at
   the rendered scale. Taught overrides still fix a misread shape.
8. **Levels use the EXP atlas, widened.** At Ideal the level digit is 7×7 and
   matches the EXP 5×7 glyph stretched to 7×7 at distance 0. `lvNumber` is a
   complete set but a different design and scores far worse; leave it unused.
9. **Name/job are font-seeded automatically.** The engine renders Arial at the
   measured size, stores the native glyphs in `identityAtlas` (`source: font`),
   and reads both lines; fingerprints stay as the exact fallback. The one
   ambiguity — `l` and `I` are the same 1 px bar — is resolved by case context
   (`EurobIood` → `Euroblood`).
10. **Map/street names come from the client's table.** The label is Arial Bold
    12 px, but per-glyph segmentation is unusable. Render the `mapName` /
    `streetName` strings from `String_000.wz/Map.img`, filter by rendered width,
    and compare the whole line with a column-pattern DTW. No teaching is needed;
    taught fingerprints still win when present.

---

## Minimap states

The minimap zooms through three states, and the label is drawn differently in
each:

| State | Chrome | Name rendering |
|---|---|---|
| Expanded | `#99BBCC` panel, 59 px tall, icon + name band | two lines, Arial Bold 12 white on `#99BBCC` |
| Title-bar only | `#99BBCC` panel, 16 px tall | none |
| Folded | `#DDDDDD` bar, 18 px tall | one line `Street :Map`, Arial Regular 12, black on light |

The `WORLD` badge (`#DD9911` border, `#FFCC55` fill, 34×10) is present in all
three and anchors detection; the surrounding fill classifies the state. The
folded line is matched against rendered `"{street} :{name}"` candidates after
inverting the crop, which scores 0.000 on both captured folded bars.

## Map and street names

The minimap's two lines are the client's own `mapName` and `streetName` strings
from `Data/String/String_000.wz` → `Map.img/<region>/<map id>`. The current
client has **347 map names and 11 unique streets**; the count is
client-version-specific. The saved samples are:

| Map id | mapName | streetName |
|---|---|---|
| `VictoriaIsland/010003007` | Kerning City Civic Center | Victoria Road |
| `VictoriaIsland/010003070` | Sunset Sky | Victoria Road |

The label font is **Arial Bold 12 px**, anti-aliased with a 1 px shadow. A
per-glyph atlas fails because thresholding fragments the anti-aliased strokes into
2–3 px stems. The reader bypasses segmentation:

1. Render each client name at Arial Bold 12 (GDI, anti-aliased).
2. Keep candidates whose rendered width is within 8 px of the live line.
3. Score the whole line with a column-pattern DTW. The correct name scores
   0.236 / 0.346 / 0.268 on the three saved lines; a 0.05 margin over the
   runner-up and a 0.55 cap reject ambiguous lines.

The same pass handles the street line. Taught fingerprints still win when they
exist; the client table is the fallback for untaught maps. The map card shows
`map table N names` from `get_mscw_status`.

The public map list at `meowdb.com/msclassic/maps/all` lists 453 maps including
Ossyria/El Nath and instanced/upcoming content; the client's own table is the
authoritative live set and matches both samples exactly.

The name index is built at 12–24 px and the matcher tries each size, so the
supported 1366×768 and 1920×1080 Ideal labels are covered.

---

## Reproducing

### WZ reading

The stock `@tybys/wz` (v1.7.1) needs two small patches to read this client:

1. Add a `RawData` case to `extractMore` in
   `lib/cjs-modern/WzImageProperty.js` (return the opaque bytes; this is where
   the TTF lives).
2. Stub `FileVersionInfo.getVersionInfo` in `lib/cjs-modern/util/node.js` to
   return zeros, so `_getMapleStoryVerFromExe` does not throw when the native
   version-info module is absent.

Open with `WzMapleVersion.CLASSIC`. Directory images are listed after
`parseDirectory()`; image properties after `parseImage()`. A canvas is exported
with:

```js
const bmp = await canvasProp.getLinkedWzCanvasBitmap()
const jimp = bmp.getCanvas()
await jimp.writeAsync(outPath)   // jimp.bitmap.width / .height
```

### Extracting the TTF

`FONT_DATA` starts with a 7-byte wrapper; the TTF begins at offset 7. Read the
sfnt table directory there (19 tables), take `max(offset + length)` as the font
length, and write those bytes out. The result is a normal `NotoSansKR-Medium.ttf`.

### Seeding in the app

The calibration window has a **Game assets** row. **Locate…** opens a folder
picker and stores the path as `mscwDir`; **Load digits** (or the next Freeze)
calls `seed_atlas`, which detects the rendered digit height from the EXP crop
and seeds the profile's EXP atlas from the client. The profile records
`source: "game"` and the UI shows `digits auto · 5x7` (or `8x11`). A misread
shape can still be corrected from the shape list, which stores a taught
override.

The app reads the archive with the `wz_reader` crate
(`WzMapleVersion::CLASSIC`), so the `@tybys/wz` patches below are only needed
for the standalone diagnostic scripts.

Useful ignored diagnostics (set `MSCOUT_MSCW_DIR` to override the default
install path):

```powershell
# every glyph set: node names, raw size, ink size
cargo test --manifest-path src-tauri/Cargo.toml --lib game_font::tests::probe_glyph_sets -- --ignored --nocapture
# live level digit vs every client set
cargo test --manifest-path src-tauri/Cargo.toml --lib game_font::tests::probe_level_source -- --ignored --nocapture
# segmented EXP shape sizes per captured frame
cargo test --manifest-path src-tauri/Cargo.toml --lib game_font::tests::probe_exp_shapes -- --ignored --nocapture
# proof: seed from empty atlases and read every captured frame
cargo test --manifest-path src-tauri/Cargo.toml --lib game_font::tests::reseed_reads_all_captured_frames -- --ignored --nocapture
# map/street name table: exact names and whole-line match
cargo test --manifest-path src-tauri/Cargo.toml --lib game_font::tests::probe_string_map_names -- --ignored --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --lib font_seed::tests::match_saved_names -- --ignored --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --lib engine::tests::reads_saved_map_frames -- --ignored --nocapture
```

### Capturing a frame

```powershell
$env:MSCOUT_CROP='750,733,84,16'      # preset EXP box, e.g. 1366x768 @ ideal
$env:MSCOUT_FRAME='live-frame.bmp'
cargo test --manifest-path src-tauri/Cargo.toml --lib capture::tests::dumps_exp_crop -- --ignored --nocapture
```

The shipped presets in `src-tauri/presets/` hold boxes for 1366×768 in all three
UI sizes and 1920×1080 Ideal; see [PRESETS.md](PRESETS.md).

---

## Where fonts live

### In the game client

| Path | Role |
|---|---|
| `Data/UI/Font/Font_000.wz` | Noto Sans KR Medium TTF + language config |
| `Data/UI/_Canvas/_Canvas_000.wz` | HUD digit/glyph bitmap atlases |
| `Data/UI/UI_000.wz` | UI layout data |

Find the game folder via Steam: `steamapps/common/MapleStory/MSCW` (the example
install is `D:\SteamLibrary\steamapps\common\MapleStory\MSCW`). The local
reference copies used during this research lived in `fonts/` (gitignored); they
are not part of the app and were removed after the clean-state audit.
Re-extract from the game/Windows when needed.

### In Windows

| Font | File |
|---|---|
| Arial (Regular) | `C:\Windows\Fonts\arial.ttf` |
| Arial Bold | `C:\Windows\Fonts\arialbd.ttf` |
| Gulim + Dotum (Korean) | `C:\Windows\Fonts\gulim.ttc` |
| Noto Sans CJK | `C:\Windows\Fonts\NotoSans{HK,JP,KR,SC,TC}-VF.ttf` |

## The Nexon brand font

`NEXON_Maplestory.zip` from `brand.nexon.com` contains `Maplestory Light/Bold`
TTF and OTF. It was tested and is **not** the HUD font: at ~10 px it gets the
sizes right (including a 7×7 `%`), but the glyph designs differ — `1` has a base
serif, `4` uses a left stem instead of the diagonal, `%` is a different shape.
It is a branding typeface, potentially useful for the overlay's own UI, not for
recognition.

## Licensing

- **Arial** is proprietary (Monotype, bundled with Windows) — local reference
  only.
- **Noto Sans KR Medium** is OFL and redistributable; the embedded TTF extracted
  from `Font_000.wz` can be used as a dependency if needed.
- **Nexon brand fonts** (Maplestory Light/Bold) are proprietary; do not bundle.
- **Game bitmap atlases** are Nexon assets. Use them locally for reference and
  testing; do not commit or redistribute them.

## Findings and open questions

Resolved:

- **`1`, `]`, `,`, `.`**: the archive has **no such nodes** in
  `EXPBar/number`. `1` comes from `Basic.img/ShowLevel/0/1`; `]` mirrors `[`;
  `,`/`.` are derived at the rendered scale. There is nothing else to find.
- **Level digits**: the live digit is the EXP glyph widened to 7×7 (triangle at
  threshold 64), verified at distance 0 for `5` and pixel-exact for `3` and `5`.
  Because the level sits on pale tiles, the reader falls back to bright-ink
  segmentation, which separates the 7×7 digit from its tile frame.
  `Basic.img/LevelNo` is the **old** tile-framed design. `status/lvNumber` is a
  complete modern set but is a different design: it scored 78 against the live
  Ideal digit, so it is not the source.
- **`7` shape**: the asset and the rendered glyph are both 5×7. The rendered `7`
  differs by about one stroke pixel (distance ~15 on the design set only). Seeding
  now learns that exact rendering, so the live read is 0.0 for every segment.
  The *cause* is the client's scale/trim, and no longer matters.
- **Map label face**: Arial Bold 12 px, identified by whole-line DTW. The names
  are the client's `String_000.wz/Map.img` table, matched without segmentation.

Still open (none block reading):

- **Level seeds at the live height only.** The level atlas is seeded from the
  measured live level digit (7×7 at Ideal), not a per-size sweep; a very different
  level size would need its own seed pass. The level is optional and rarely
  changes size.
