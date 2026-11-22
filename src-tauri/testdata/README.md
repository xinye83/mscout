# Test data

The tracked test data for MScout's readers: lossless crops of real Classic
World client frames, each with the values it must read as, plus the
release-shaped profiles the upgrade tests load. Only tests read this directory;
the app never opens it. It ships under the "test data only" exception in
[the shipping boundary](../../docs/SHIPPING.md#test-fixtures-shipped-as-test-data-only)
(D28): no runtime atlases, fonts, name lists or player profiles.

## Layout

```
testdata/
  frames/<WxH@mode>/
    pair.json                 client size, UI Size, settings-panel evidence
    settings-panel.png        the open Graphics panel (evidence only)
    <scene>/                  one capture moment, e.g. euroblood-lv35-parched-land
      scene.json              what this moment reads as, and its crops
      hud-main.png
      minimap-expanded.png  minimap-titlebar.png  minimap-folded.png ...
  crops/<WxH@mode>/           focused crops, read on their own
    exp-21865.png  + exp-21865.json
    map-tree-1.png + map-tree-1.json
  profiles/upgrade/           profile files as older releases wrote them
```

- **`<WxH@mode>`** is the preset/profile key: `<clientW>x<clientH>` plus `""`
  for Default, `@ideal` or `@filtered`. The loader checks the name against
  `pair.json`.
- **A scene owns its truth.** A pair can hold any number of scenes, so a new
  character, level, job or map at an existing size is a new scene directory,
  not a new pair. Name it `<character>-lv<level>-<map>`.
- **A crop owns its truth**, in a sidecar with the same stem. The file name
  starts with its kind: `exp-`, `level-`, `name-`, `job-` or `map-`.
- **Ids are file stems.** Tests find an image with
  `corpus::fixture_image(pair, id)`: a crop by its stem, a scene image by its
  id (`<scene>/<id>` when the id repeats across scenes).

Every file has to be indexed. A PNG no manifest names, a sidecar without its
PNG, or a stray file in a pair or scene directory fails the hermetic tier.

## Manifests (version 3)

`pair.json`:

```jsonc
{
  "version": 3,
  "clientW": 1920, "clientH": 1080, "uiMode": "ideal",
  "settingsDisplay": { "resolution": "1920 x 1080", "uiSize": "Ideal Ratio" },
  "settingsPanel": { "file": "settings-panel.png", "crop": { "x": 650, "y": 255, "w": 405, "h": 525 } }
}
```

`settingsDisplay` is what the client's own Graphics panel says; it must agree
with `clientW`/`clientH`/`uiMode`, and the panel PNG must not be a flat colour.

`<scene>/scene.json`:

```jsonc
{
  "version": 3,
  "added": "2026-11-15",          // the day the crops entered the corpus
  "client": "…",                  // optional: the client build, when known
  "verified": true,               // a person checked every value against the pixels
  "truth": {
    "name": "Euroblood", "job": "Page", "level": 35,
    "expText": "123,044[70.63%]", "exp": 123044, "pct": 70.63,
    "street": "Victoria Road", "map": "East Rocky Mountain III"
  },
  "images": [
    { "id": "hud-main", "kind": "hud", "file": "hud-main.png", "crop": { "x": 560, "y": 1036, "w": 574, "h": 44 } },
    { "id": "minimap-expanded", "kind": "minimap", "state": "expanded", "holdout": true,
      "file": "minimap-expanded.png", "crop": { "x": 18, "y": 12, "w": 200, "h": 210 } }
  ]
}
```

- Crops are in **client coordinates** (the same space as presets and profile
  boxes; see [D13](../../docs/DECISIONS.md)) and must fit the client.
- Exactly one `hud` image and at least one `minimap` image. `state` is
  `expanded`, `titlebar`, `folded` or `hidden`. Every minimap state becomes one
  frame case: the HUD and that minimap pasted onto a black client-sized frame.
- `expText` must parse to `exp` and `pct`.
- All minimap images of a scene come from the same map, so `street`/`map` hold
  for each of them.

`crops/<pair>/<stem>.json`:

```jsonc
{
  "version": 3,
  "kind": "exp",                  // exp | level | name | job | map
  "added": "2026-11-16",
  "verified": true,
  "readCrop": { "x": 650, "y": 15, "w": 155, "h": 18 },   // optional; default the whole PNG
  "truth": { "expText": "2,056[1.06%]", "exp": 2056, "pct": 1.06 }
}
```

The truth needs the fields its kind reads: `exp` → `expText`/`exp`/`pct`;
`level` → `level`; `name` → `name`; `job` → `job`; `map` → `street`/`map`.

## Adding test data

**From a misread (the usual case).** In Calibration, freeze the frame and click
**Save diagnostic**. The file in `data/diagnostics/` carries the HUD strip
around every read box, losslessly. Import it:

```sh
MSCOUT_DIAGNOSTIC=../data/diagnostics/read-….json \
  cargo test --lib corpus::import_diagnostic -- --ignored --nocapture
```

This writes `name-`, `job-`, `exp-` and `level-` crops under `crops/<pair>/`
with what MScout **read**, marked `"verified": false`, and never overwrites a
file. The gate refuses unverified truth, so the import is finished only when
you have checked each value against the pixels, corrected the wrong one (the
reason you saved the diagnostic), renamed a file whose name carries the wrong
value, and set `"verified": true`. Then point the regression test at the crop.

**A new scene.** Needs a full client frame: the HUD and the minimap in each
state you want, captured on one map.

1. Capture the client area only (no window chrome), at the size and UI Size of
   the pair. At least one capture per pair has `OPTIONS > Graphics` open, for
   `settings-panel.png`.
2. Crop in client coordinates with no resampling, scaling or lossy encoding,
   and save PNG. Leave a few pixels of margin around every readout; a clipped
   glyph is a fixture bug.
3. Write `scene.json` by reading the frame, not from memory, and keep the source
   frame out of this directory and out of git.

**A new pair** is a new `frames/<WxH@mode>/` with `pair.json`, its settings
panel and at least one scene. Every pair is read on the client tier: a size
with no shipped preset reads with the 1366×768 boxes moved with the HUD bar
(`preset.rs`), on the 1x picture (`native.rs`).

## The frozen hold-out

One minimap state is the frozen hold-out, marked `"holdout": true`:
`frames/1920x1080@ideal/euroblood-lv35-east-rocky-mountain-3`, `expanded`,
frozen on 2026-11-16. The flag is the record; `exactly_one_frozen_holdout`
fails on zero or on two, and a flag on a HUD image is rejected when the corpus
loads.

The hold-out is read like every other case and `accuracy_report` publishes its
result. It is not an input to tuning: do not move a box, threshold, atlas,
matcher or map table because it reads wrong, do not re-crop, re-label or
replace it, and do not move the flag. If its result changes, that is a finding
to report. It was frozen retrospectively — every frame here was looked at while
the 0.6 readers were tuned — so it is not an unbiased accuracy estimate; see
[ACCURACY.md](../../docs/ACCURACY.md).

## Tests

The loader and the hermetic checks are in [`src/corpus.rs`](../src/corpus.rs);
the reads are in [`src/regression.rs`](../src/regression.rs). Run from
`src-tauri`.

**Hermetic** (`cargo test --lib`, any machine): every pair, scene and crop
validated as above; `minimap_state_matches_ground_truth` (the detector names
each case's state); `settings_panel_is_evidence_only` (the panel is never
composed into a frame); `exactly_one_frozen_holdout`.

**Client tier** (`cargo test --lib regression -- --ignored --nocapture`, needs
the MSCW install and Windows Arial): `reads_every_fixture_from_empty_atlases`
and `reads_follow_the_manifest_minimap_state` seed every atlas from the frame
itself and require `engine::read_frame` to reproduce the scene's truth exactly;
`accuracy_report` scores every case exact / rejected / wrong without asserting
the counts. Focused crops are read by their own targeted tests.

The MSCW folder comes from `MSCOUT_MSCW_DIR`, else from the app's saved setting
(`data/settings.json` → `mscwDir`), which tests copy into their temporary data
folder. Without it, expanded and folded cases read the map as `unknown`.

`MSCOUT_CASE=<pair>` or `<pair>/<scene>` narrows either tier, and
`MSCOUT_STATE=<state>` narrows it to one minimap state.

## Profiles

`profiles/upgrade/` holds one profile per shape the format went through
(`0.1.0`, `0.2.0`, `0.5.0`, `0.6.0`), reconstructed from each release's
`Profile` struct. They are schema samples, not captures; see its README and
`profile::tests`.
