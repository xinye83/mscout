# MScout

<p align="center">
  <img src="src-tauri/icons/icon.png" alt="MScout icon" width="128">
</p>

A read-only companion overlay for **MapleStory Classic World** (Nexon, live since 2026-10-21, v1.2.0 as of 2026-11). It reads what is on the game screen — today that is your EXP readout — records it, and charts it. Further readouts (damage, potions, per-map stats) are intended to join it.

> **This is a vibecoded project.** It was built by pairing with an AI coding agent rather than written by hand, line by line. The behaviour is tested against the real game and the Rust side has a unit-test suite, but expect the rough edges of a fast, AI-assisted codebase: uneven comments, the occasional over-engineered abstraction, and design choices that were reasoned through in conversation rather than reviewed by a second human. Read it accordingly.

It never touches the game client: no memory reading, no injection, no input simulation. Screen pixels in, numbers out. See [docs/SPEC.md §1](docs/SPEC.md) for the full constraint list.

**Status: v0.1.0 release.** The overlay, tray, hotkeys, window targeting, calibration, WGC capture, EXP parsing and storage all work. Samples are real rows read from the game screen; there is no demo data path left. See [docs/ROADMAP.md](docs/ROADMAP.md).

## Requirements

- Windows 11 (Windows Graphics Capture; Windows 10 2004+ should work, untested)
- Node 26 via `fnm`
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

1. Attach the game window (`Ctrl+Alt+T`, or tray → *Choose game window…*).
2. Open calibration: tray → *Calibrate boxes…*, or the **Calibrate** button in the picker.
3. **Freeze frame**, then drag boxes on the preview: EXP text, level digits, and optionally map label, character name and job. **Save boxes**. Draw the EXP box with a few pixels of slack on the right — the number grows and gains a digit at 100,000, which would otherwise clip the trailing `]` and make parsing fail.
4. Type exactly what the EXP box shows (for example `86,813 [55.81%]`) and press **Learn digits**. The card shows what is learned and which digits are still missing.
5. Teach the map, character name and job once by typing their labels.
6. **Test parse** reads the frozen frame with the saved boxes — it must match what is on screen before you rely on it.

Moving the game window is harmless: boxes are relative to the client area. The minimap is detected automatically each second, so moving or resizing it does not invalidate map names. Calibration is stored **per client size** in `data/profiles/<W>x<H>.json`, so each resolution keeps its own set — changing resolution switches profiles rather than breaking anything, but a size you have never calibrated needs one calibration pass. Digits are learned per font size: the EXP font, the level tiles and the HP/MP numbers are separate variants.

The two-digit EXP percentage is shown as `%.2f`. The prominent EXP/hour and time-to-level are both derived from the **5-minute average**: they show `—` until a full 5-minute window of the current session exists, again whenever that average is `0`, and the ETA stays `—` while idle.

## Where data lives

- Development: `./data/` in the project root
- Packaged: `data/` next to the executable
- Override: `MSCOUT_DATA_DIR` environment variable

Sample format and invariants are documented in [docs/DATA.md](docs/DATA.md).

## Documentation

| File | Purpose |
|---|---|
| [docs/SETUP.md](docs/SETUP.md) | Install dependencies, run from source, build a release |
| [CHANGELOG.md](CHANGELOG.md) | What each version added, changed and fixed |
| [docs/SPEC.md](docs/SPEC.md) | Behavioral spec — what v1 must do |
| [docs/ROADMAP.md](docs/ROADMAP.md) | What is built, what is next, acceptance criteria |
| [docs/DECISIONS.md](docs/DECISIONS.md) | Decision log — why things are the way they are |
| [docs/DATA.md](docs/DATA.md) | File formats, profiles, schema, invariants |
| [docs/DEV.md](docs/DEV.md) | Environment gotchas, testing, layout |

## Repository layout

```
src/                 React UI (live panel, history, calibration, chart, rates)
src-tauri/src/       Rust: capture, OCR/matching, engine loop, profiles, tray/hotkeys
src-tauri/           Tauri config, capabilities, app icons
scripts/             dev-only mock sample generator
docs/                setup, roadmap, decisions, data format, dev notes
data/                runtime data (gitignored — personal, machine-specific)
```

## Dev helper

`node scripts/gen-mock.mjs` writes three days of plausible samples into `data/samples/` so the chart and history window can be exercised without the game. Delete `data/samples/*.jsonl` afterwards so mock rows never mix with real captures.

## License

[MIT](LICENSE) © 2026 Xin Ye.

Use it, modify it, redistribute it, sell it — just keep the copyright notice and the warranty disclaimer. The simplest licence that keeps the project usable by anyone.
