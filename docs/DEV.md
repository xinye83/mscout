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
| `npx tauri build --no-bundle` | Release exe at `src-tauri/target/release/mscout.exe` (~9 min cold) |
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
docs/                setup, spec, roadmap, decisions, data format, dev notes
```

## Testing

`cargo test --manifest-path src-tauri/Cargo.toml --lib` runs the Rust suite: EXP-table anchors, glyph segmentation/matching, thresholding, settings/profile round-trips, and window enumeration.

Some tests are `#[ignore]`d diagnostics that need the game window or a captured frame. They are listed by `cargo test -- --list`:

| Diagnostic | Needs | Writes |
|---|---|---|
| `captures_game_client_area` | game window | — |
| `dumps_a_frame_for_inspection` | game window | `data/debug-frame.bmp` |
| `dumps_overlay` | a target window (`MSCOUT_DUMP_TITLE`) | `data/debug-window.bmp` |
| `detect_minimap` | `data/debug-frame.bmp` | — |
| `learn_from_bmp` | `data/debug-frame.bmp`, `MSCOUT_LEARN_TEXT` | updates the active profile's atlas |
| `teach_map_from_bmp` | `data/debug-frame.bmp`, `MSCOUT_MAP_NAME` | updates the active profile's maps |
| `live_settings_read` | game window | — |

Run one with `cargo test --lib <name> -- --ignored --nocapture`. The front end has no test runner yet; `npm run typecheck` is the gate there.
