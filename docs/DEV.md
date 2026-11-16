# Development notes

See [SETUP.md](SETUP.md) for installing the toolchain and running the app from a clean machine. This file covers what bites you once it is installed.

## Environment gotchas (learned the hard way)

**Node is installed via fnm and is not on PATH by default.** A fresh PowerShell has no `npm`. Fix per shell:

```powershell
fnm env --use-on-cd --shell powershell | Out-String | Invoke-Expression
```

**Cargo is not on PATH for tool shells.** Prepend it:

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
```

**`NODE_ENV=production` is set globally, and npm is configured with `omit=dev`.** Plain `npm install` silently skips all devDependencies. The project `.npmrc` sets `include=dev`; if installs still come up short, run with `$env:NODE_ENV='development'`.

**npm blocks install scripts by default.** `esbuild` needs its postinstall; approve it with `npm install-scripts approve esbuild`.

**Vite watches `src-tauri/target` and crashes on a locked DLL (`EBUSY`).** `vite.config.ts` excludes `src-tauri/**` and `data/**` from the watcher.

## Commands

| Command | What it does |
|---|---|
| `npm run tauri dev` | Vite dev server + debug build + app |
| `npx tauri build --no-bundle` | Release exe at `src-tauri/target/release/mscout.exe` |
| `npx tauri build` | Full NSIS installer |
| `cargo check --manifest-path src-tauri/Cargo.toml` | Fast Rust compile check |
| `npx tsc --noEmit` | Frontend typecheck |
| `node scripts/gen-mock.mjs` | Write mock samples into `data/samples/` |

## Running the built exe

The debug binary expects the Vite server on `http://localhost:1420`; start `npm run dev` first.

Release binary is self-contained (frontend embedded). Useful environment variables:

| Variable | Effect |
|---|---|
| `MSCOUT_DATA_DIR` | Point the app at a different data folder |
| `MSCOUT_OPEN_HISTORY` | Open the history window at startup (dev convenience) |
| `MSCOUT_OPEN_CALIBRATION` | Open the calibration window at startup (dev convenience) |

## Window notes

- Overlay is click-through by default: `set_ignore_cursor_events(true)`.
- Hotkeys are registered through `tauri-plugin-global-shortcut` (OS-level, not low-level hooks).
- History is a second WebviewWindow labelled `history`; the frontend picks its view by window label, not URL query (Tauri cannot resolve asset paths with query strings).
- Capturing a window for debugging: use `PrintWindow` against the specific HWND. Do **not** take full-desktop screenshots — it captures unrelated content.

## Layout

```
src/                 React UI (live panel, history, calibration, chart, rates, EXP table)
src-tauri/src/       Rust: capture, OCR/matching, engine loop, profiles, window/tray/hotkeys
scripts/             dev-only mock sample generator
data/                runtime data (gitignored): samples, settings, per-resolution profiles
docs/                setup, presets, spec, roadmap, decisions, data format, dev notes, rendering research
```

## Testing

`cargo test --manifest-path src-tauri/Cargo.toml --lib` runs the Rust suite: EXP-table anchors, glyph segmentation/matching, thresholding, settings/profile round-trips, and window enumeration.

Some tests are `#[ignore]`d diagnostics that need the game window, the MSCW
install, or a captured frame. They are listed by `cargo test -- --list`. The
MSCW folder defaults to the Steam example path; override it with
`MSCOUT_MSCW_DIR`. The ones most useful after a text-rendering change:

| Diagnostic | Needs | Writes |
|---|---|---|
| `game_font::probe_game_atlas` | MSCW install | — (lists glyph sets) |
| `game_font::seed_live_level` | live game | — (seeds/prints the level read) |
| `game_font::seed_live_profile` | live game | profile when `MSCOUT_SAVE_SEED=1` |
| `game_font::probe_1920_filtered` | Filtered fixture + MSCW | — (EXP/level/name/job/map probe) |
| `map_names::probe_string_jobs` | MSCW install | — (scans String/Etc/UI for job tables) |
| `regression::reads_every_fixture_from_empty_atlases` | MSCW install + Arial | — (full client tier) |
| `regression::reads_follow_the_manifest_minimap_state` | MSCW install + Arial | — (per-state client tier) |
| `regression::reads_live_1920` | live game + MSCW | — (full live read) |
| `regression::probe_live_minimap` | live game | — (live minimap state/labels) |
| `capture::dumps_exp_crop` | live game, `MSCOUT_CROP`, `MSCOUT_FRAME` | `data/<MSCOUT_FRAME>` |

Run one with `cargo test --manifest-path src-tauri/Cargo.toml --lib <name> -- --ignored --nocapture`. Set `MSCOUT_SHOW_CROP=1` to print the thresholded crop. The front end has no test runner yet; `npx tsc --noEmit` is the gate there.
