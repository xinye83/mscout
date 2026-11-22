# MScout

<p align="center">
  <img src="src-tauri/icons/icon.png" alt="MScout icon" width="128">
</p>

A read-only companion overlay for **MapleStory Classic World**. It reads supported HUD values, records them, and charts EXP progress. It never touches the game client: no memory reading, no injection, no input simulation. Screen pixels in, numbers out.

> **This is a vibecoded project.** It was built by pairing with an AI coding agent rather than written by hand, line by line. The behaviour is tested against the real game and the Rust side has a unit-test suite, but expect the rough edges of a fast, AI-assisted codebase: uneven comments, the occasional over-engineered abstraction, and design choices that were reasoned through in conversation rather than reviewed by a second human. Read it accordingly.

See [docs/SPEC.md](docs/SPEC.md) for supported behaviour and constraints, [docs/PRESETS.md](docs/PRESETS.md) for the current preset/support matrix, and [docs/ROADMAP.md](docs/ROADMAP.md) for open work.

## Getting started

MScout runs on Windows 10 2004+ / 11 and builds with Node.js, Rust (MSVC) and
the Visual Studio Build Tools; the UI needs the WebView2 runtime.
**[docs/SETUP.md](docs/SETUP.md)** takes a clean machine to a running app, with
the hotkeys and the first calibration. [docs/DEV.md](docs/DEV.md) covers the
traps of this machine setup, the commands and the tests.

## Calibrating

Most setups need no calibration. MScout reads the game's own 1x picture, so the
1366×768 boxes fit any resolution, and above 768 lines it detects the UI Size
from the game. Calibration remains for correcting a box; the full flow, preset
support and troubleshooting are in
[docs/SETUP.md §3](docs/SETUP.md#calibrate) and
[docs/PRESETS.md](docs/PRESETS.md). The overlay reports when a pair needs
calibration; it never opens the calibration window by itself.

## Where data lives

- Development: `./data/` in the project root
- Packaged: `data/` next to the executable
- Override: `MSCOUT_DATA_DIR` environment variable

Sample format and invariants are documented in [docs/DATA.md](docs/DATA.md).

## Documentation

| File | Purpose |
|---|---|
| [docs/SETUP.md](docs/SETUP.md) | Install dependencies, run from source, build a release |
| [docs/PRESETS.md](docs/PRESETS.md) | Preset and support matrix, reset behavior, limitations |
| [CHANGELOG.md](CHANGELOG.md) | What each version added, changed and fixed |
| [docs/SPEC.md](docs/SPEC.md) | Behaviour rules: constraints, data integrity, rate math, states, known limitations |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Open work |
| [docs/DECISIONS.md](docs/DECISIONS.md) | Decision log — why things are the way they are |
| [docs/DATA.md](docs/DATA.md) | File formats, profiles, schema, invariants |
| [docs/ACCURACY.md](docs/ACCURACY.md) | Measured accuracy of the tracked fixture corpus, with the commands and its limits |
| [docs/TEXT_RENDERING.md](docs/TEXT_RENDERING.md) | Developer reference for HUD glyphs, fonts and text matching |
| [docs/SHIPPING.md](docs/SHIPPING.md) | What the app ships vs what it reads from the client/Windows |
| [docs/DEV.md](docs/DEV.md) | Environment gotchas, testing, layout |

## Repository layout

```
src/                 React UI (live panel, history, calibration, chart, rates)
src-tauri/src/       Rust: capture, OCR/matching, engine loop, profiles, tray/hotkeys
src-tauri/           Tauri config, capabilities, app icons
scripts/             dev-only mock sample generator
docs/                setup, presets, spec, roadmap, decisions, data, dev notes, rendering reference
data/                runtime data (gitignored — personal, machine-specific)
```

## Dev helper

`node scripts/gen-mock.mjs` writes three days of plausible samples into `data-mock/samples/` (or `$MSCOUT_DATA_DIR/samples/`) so the chart and history window can be exercised without the game. Point the app at that folder with `$env:MSCOUT_DATA_DIR = "$PWD\data-mock"` before `npm run tauri dev`. The script refuses the real `data/` folder and never overwrites an existing day file, so real captures are never touched.

## License

[MIT](LICENSE) © 2026 Xin Ye.

Use it, modify it, redistribute it, sell it — just keep the copyright notice and the warranty disclaimer. The simplest licence that keeps the project usable by anyone.

Third-party notices, including the MIT attribution for the mpl-ocr ideas used by
the 0.6.0 readers, are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
