# Accuracy — the tracked fixture corpus

What the capture readers actually score on the committed corpus, published with
the command that produced the numbers and the limits of the measurement stated
next to them.

Re-run **2026-11-21** on `0.7.0` plus the post-release storage, guard and reader fixes, with the local build runner's temp data folder: unchanged, 18/18 exact and the hold-out exact.

Last measured in full below: **2026-11-16**, MScout `0.6.0` (no version bump), corpus at
[`../src-tauri/testdata`](../src-tauri/testdata): 6 `WxH@mode` pairs, 18 frame
cases. This is one dated run of one small corpus — not a release claim, and not a
general accuracy estimate for the reader.

## What is measured

`regression::accuracy_report` ([`../src-tauri/src/regression.rs`](../src-tauri/src/regression.rs))
is an `#[ignore]`d test in the client-dependent tier. It reads every frame case
the way the gate does — atlases seeded from the frame itself, no saved profile
read or written, `engine::read_frame` over the preset for the case's `WxH@mode` —
and classifies the result against the manifest:

| verdict | meaning |
|---|---|
| **exact** | every manifest field was reproduced: `expText`, `exp`, `pct` (±0.005), `level`, `name`, `job`, plus `map`/`map_hidden` as the case's minimap state implies — the `<street>\n<map>` label and not hidden for `expanded`/`folded`, `unknown` and hidden for `titlebar`, `unknown` and not hidden for `hidden` |
| **rejected** | the reader (or its seeding) refused to produce a reading, e.g. `could not segment the EXP readout` |
| **wrong** | a reading was produced and at least one field disagrees; the report prints which |
| no preset | the pair has no preset, so there is nothing to read — counted separately, never as a pass |

The report prints one line per case (with the disagreeing fields), the per-pair
counts and the frozen hold-out line. It deliberately asserts nothing about the
counts: a known wrong stays a printed wrong instead of becoming a silent pass, and
an improvement cannot be manufactured by editing an expectation here.

Covered by the counts: the 18 frame cases — 6 pairs × 3 minimap states
(`expanded`, `titlebar`, `folded`).

Not covered by the counts: the focused crops in `crops/1920x1080@filtered/`
(`exp-2056-hud`, `exp-21865`, `exp-26487`, `exp-36818`, `map-tree-1`,
`map-deep-valley`). They are validated hermetically and read by
their own targeted tests, not by this report.

## Measured counts — 2026-11-16

### Run 1 — app's MSCW setting available

An expanded/folded map name is matched against the client table reached through
the app's *saved* MSCW setting (`data/settings.json` → `mscwDir`), not through
`MSCOUT_MSCW_DIR` (see [Corpus limits](#corpus-limits)). A debug build on this
machine resolves the data directory to `<project root>/data`, and the worktree
had no `settings.json`, so this run points `MSCOUT_DATA_DIR` at a throwaway
directory holding only that one setting. The whole run is one block, so the
command reproduces the table below on its own:

```powershell
$env:MSCOUT_MSCW_DIR = 'D:\SteamLibrary\steamapps\common\MapleStory\MSCW'
$data = Join-Path $env:TEMP 'mscout-accuracy-data'
New-Item -ItemType Directory -Force -Path $data | Out-Null
[System.IO.File]::WriteAllText("$data\settings.json", '{"mscwDir":"D:\\SteamLibrary\\steamapps\\common\\MapleStory\\MSCW"}')
$env:MSCOUT_DATA_DIR = $data
cargo test --manifest-path src-tauri/Cargo.toml --lib regression::accuracy_report -- --ignored --nocapture
```

| pair | exact | rejected | wrong | no preset | cases |
|---|---:|---:|---:|---:|---:|
| `1366x768` | 3 | 0 | 0 | 0 | 3 |
| `1366x768@filtered` | 3 | 0 | 0 | 0 | 3 |
| `1366x768@ideal` | 3 | 0 | 0 | 0 | 3 |
| `1920x1080` | 3 | 0 | 0 | 0 | 3 |
| `1920x1080@filtered` | 3 | 0 | 0 | 0 | 3 |
| `1920x1080@ideal` | 3 | 0 | 0 | 0 | 3 |
| **total** | **18** | **0** | **0** | **0** | **18** |

Hold-out line from the same run:

```
hold-out : 1920x1080@ideal/expanded — exact
```

Run 1 needed the app's saved MSCW setting as well as `MSCOUT_MSCW_DIR`, because an
expanded/folded map name is matched against the client table reached through
`data/settings.json` → `mscwDir` (see [Corpus limits](#corpus-limits)); that is why
the throwaway `data` directory is part of the command above. Nothing in the
repository, the tracked corpus, or the app's real settings store was written. No
`MSCOUT_CASE`/`MSCOUT_STATE` filter was set, so the report printed
`filters : none (whole corpus)`.

### Run 2 — the same corpus with no saved MSCW setting

The identical command without `data/settings.json` (a fresh clone, or a worktree
that has never run the app):

| pair | exact | rejected | wrong | no preset | cases |
|---|---:|---:|---:|---:|---:|
| `1366x768` | 1 | 0 | 2 | 0 | 3 |
| `1366x768@filtered` | 1 | 0 | 2 | 0 | 3 |
| `1366x768@ideal` | 1 | 0 | 2 | 0 | 3 |
| `1920x1080` | 1 | 0 | 2 | 0 | 3 |
| `1920x1080@filtered` | 1 | 0 | 2 | 0 | 3 |
| `1920x1080@ideal` | 1 | 0 | 2 | 0 | 3 |
| **total** | **6** | **0** | **12** | **0** | **18** |

```
hold-out : 1920x1080@ideal/expanded — wrong map "unknown" != "Victoria Road\nEast Rocky Mountain III"
```

Every one of the 12 wrongs is the same field: `map "unknown" != "<street>\n<map>"`
on the `expanded` and `folded` states. The EXP readout, EXP value, percentage,
level, name and job were exact in all 18 cases; the six `titlebar` cases are exact
because that state asserts the map is hidden, so it never exercises the label
reader. Run 2 is **not** a second accuracy figure for the reader — it is the same
corpus with one input missing, and it is listed so nobody reads Run 1 as something
a fresh clone reproduces by default.

## The hold-out is retrospective

One state is the frozen hold-out: **`1920x1080@ideal`, `expanded`** (case key
`1920x1080@ideal/euroblood-lv35-east-rocky-mountain-3/expanded` since the 0.8
test-data layout), marked `"holdout": true` in
[its `scene.json`](../src-tauri/testdata/frames/1920x1080@ideal/euroblood-lv35-east-rocky-mountain-3/scene.json)
on 2026-11-16. `exactly_one_frozen_holdout` (hermetic tier) fails if the corpus has
zero or more than one. The manifest flag is the one record of which case it is;
moving it is forbidden by the rules below, not by a second copy of the key in the
test. A `holdout` flag on a HUD image is rejected when the corpus loads instead
of being silently ignored. The accuracy report always names the hold-out.

**It was frozen after the fact, not before the work.** Every frame in this corpus
was captured, cropped and labelled during the `0.5` → `0.6` work, and every one of
them — this state included — was looked at while the Filtered EXP, level and
identity readers were tuned. From 2026-11-16 the flag reserves the case: no box,
threshold, atlas, matcher, map table or fallback may be changed because the
hold-out reads wrong, and the frame may not be re-cropped, re-labelled or
replaced when the reader changes. If its result changes, that is a finding to
report.

What that does **not** buy: the hold-out is not historically independent of the
tuning that preceded it, it is one 200×210 crop on one pair, and its "exact" above
is therefore not a clean generalisation estimate. It is a reservation for future
work, and this document says so on purpose.

## Corpus limits

- **Size.** 18 frame cases from 6 pairs, all cropped from a handful of saved
  frames. Ten wrong readings in one game session would say more than these 18.
- **One character.** Every pair is level 35, name `Euroblood`, job `Page`. Level
  digit variety is one number (`35`), and identity variety is two short strings.
- **Few distinct EXP readouts.** The six pairs use five EXP strings
  (`106,643[61.21%]`, `108,508[62.28%]`, `109,284[62.73%]`, `123,044[70.63%]`,
  `127,028[72.91%]`). The live Filtered digit sweep that found the `8`-as-`6`/`9`
  glyph gap was a separate live probe; **that specific failure is not representable
  in this corpus**, which is why it is still listed in
  [ROADMAP → Known limitations](ROADMAP.md#known-limitations) while the corpus
  reads exactly.
- **Three map labels.** `Victoria Road` / `Parched Land` (all `1366x768` pairs),
  `Hidden Street` / `Free Market <8>` (`1920x1080` Default and Filtered), and
  `Victoria Road` / `East Rocky Mountain III` (`1920x1080@ideal`). Map accuracy
  here is three labels deep; a fourth map outside the 347-name client table is
  untested.
- **Two resolutions.** 1366×768 and 1920×1080 in three UI Sizes each. 1920×1200
  has neither preset nor fixture, so it is invisible to these counts.
- **Map labels need the app's saved MSCW setting.** The expanded/folded label
  path resolves the client's map/street table through `data/settings.json` →
  `mscwDir`; `MSCOUT_MSCW_DIR` seeds glyphs, it does not feed that lookup. With no
  saved setting the label reader returns `unknown` and 12 of 18 cases read
  "wrong" for that one field (Run 2). This is an environment dependency of the
  corpus, not a reading regression, and it is the honest reason Run 2 exists.
- **Per-case, not per-field, verdicts.** A case is one verdict. A single wrong
  field in one 200×210 crop makes the whole case `wrong`, so the counts
  under-state everything that did read correctly and cannot be read as per-field
  accuracy.
- **Windows-only verification.** Windows Arial, the MSCW install and
  `Windows.Media.Ocr` are all required. Nothing here was measured on another OS,
  and no live game frame is part of the counts: the two live tests in the tier
  (`reads_live_1920`, `probe_live_minimap`) capture the running client and are not
  used by the report.
- **One build, one client version.** The numbers come from the client installed at
  `D:\SteamLibrary\steamapps\common\MapleStory\MSCW` on 2026-11-16. The map table
  is version-specific (347 names / 11 streets in this build); another client
  version can move map results without any code change.

## What these numbers do not say

- They do not say the reader is 100% accurate. 18 exact reads over one character,
  three maps and one client version is a smoke test, not a measurement of field
  accuracy.
- They do not say the Filtered glyph gaps are fixed. The known `8` → `6`/`9` EXP
  error and the percentage `3` → `8` misread are found by the live digit sweep and
  remain listed in [ROADMAP → Known limitations](ROADMAP.md#known-limitations).
- They do not say the hold-out is unbiased — see
  [The hold-out is retrospective](#the-hold-out-is-retrospective).

## Reproducing

The full Run 1 command — `MSCOUT_MSCW_DIR` plus the throwaway `data` directory
that carries the app's MSCW setting — is in
[Run 1](#run-1--apps-mscw-setting-available) above:

```powershell
$env:MSCOUT_MSCW_DIR='D:\SteamLibrary\steamapps\common\MapleStory\MSCW'
cargo test --manifest-path src-tauri/Cargo.toml --lib regression::accuracy_report -- --ignored --nocapture
```

This short form is Run 2 unless a `data/settings.json` with `mscwDir` is present
(set `MSCOUT_DATA_DIR` as in Run 1 for the 18/0/0 numbers). Add `MSCOUT_CASE=<pair>`
(or `MSCOUT_CASE=<pair>/<state>`) and `MSCOUT_STATE=<expanded|titlebar|folded|hidden>`
to narrow the report; the printed `filters` line records what was active so a
narrowed run cannot be mistaken for a whole-corpus one. The hermetic tier is
`cargo test --manifest-path src-tauri/Cargo.toml --lib`
(no game install, no `MSCW` needed).
