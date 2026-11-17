# MScout

<p align="center">
  <img src="src-tauri/icons/icon.png" alt="MScout icon" width="128">
</p>

A read-only companion overlay for **MapleStory Classic World**. It reads supported HUD values, records them, and charts EXP progress. It never touches the game client: no memory reading, no injection, no input simulation. Screen pixels in, numbers out.

> **This is a vibecoded project.** It was built by pairing with an AI coding agent rather than written by hand, line by line. The behaviour is tested against the real game and the Rust side has a unit-test suite, but expect the rough edges of a fast, AI-assisted codebase: uneven comments, the occasional over-engineered abstraction, and design choices that were reasoned through in conversation rather than reviewed by a second human. Read it accordingly.

See [docs/SPEC.md](docs/SPEC.md) for supported behaviour and constraints, [docs/PRESETS.md](docs/PRESETS.md) for the current preset/support matrix, and [docs/ROADMAP.md](docs/ROADMAP.md) for what is built and what is next.

## Requirements

- Windows 11 (Windows Graphics Capture; Windows 10 2004+ should work, untested)
- Node.js 20+ (26 used in development)
- Rust stable (MSVC toolchain) + Visual Studio Build Tools 2022 (C++ workload) + Windows SDK
- WebView2 runtime (preinstalled on Windows 11)

Full install-from-scratch instructions, including the failing-linker and missing-runtime cases, are in **[docs/SETUP.md](docs/SETUP.md)**.

## Quick start

```powershell
fnm env --use-on-cd --shell powershell | Out-String | Invoke-Expression
npm install
npm run tauri dev
```

Hotkeys:

| Keys | Action |
|---|---|
| `Ctrl+Alt+M` | Toggle interactive (overlay is click-through by default) |
| `Ctrl+Alt+V` | Show / hide overlay |
| `Ctrl+Alt+H` | Open history window |
| `Ctrl+Alt+T` | Open target-window picker (attach to the game) |

Tray menu offers the same actions plus Quit.

## Calibrating

Calibration is a one-time pass per client size and UI Size. Presets ship for
1366×768 and 1920×1080 in all three UI sizes; the full flow, preset support and
troubleshooting are in
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
| [docs/SPEC.md](docs/SPEC.md) | Behavioral spec |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Current release, scheduled/deferred work, known limitations |
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

`node scripts/gen-mock.mjs` writes three days of plausible samples into `data/samples/` so the chart and history window can be exercised without the game. Delete `data/samples/*.jsonl` afterwards so mock rows never mix with real captures.

## License

[MIT](LICENSE) © 2026 Xin Ye.

Use it, modify it, redistribute it, sell it — just keep the copyright notice and the warranty disclaimer. The simplest licence that keeps the project usable by anyone.

Third-party notices, including the MIT attribution for the mpl-ocr ideas used by
the 0.6.0 readers, are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
