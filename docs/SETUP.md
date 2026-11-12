# Setup — from a clean machine to a running app

Everything here is Windows-specific: the app captures a game window with Windows Graphics Capture and overlays it with a native window. It does not build or run on macOS or Linux in this version.

Verified on Windows 11 with Node 26.13.0, npm 12.2.2, Rust 1.99.1 and WebView2 157. Older versions likely work; those are the ones actually exercised.

---

## 1. Prerequisites

| Requirement | Why | How to check |
|---|---|---|
| **Windows 10 2004+ / Windows 11** | Windows Graphics Capture | `winver` |
| **WebView2 runtime** | renders the UI | preinstalled on Windows 11; see below |
| **Node.js 20+** (26 recommended) | front end + build tooling | `node --version` |
| **Rust stable (MSVC)** | the native core | `rustc --version` |
| **Visual Studio Build Tools 2022** | MSVC linker + Windows SDK | — |

### WebView2

Windows 11 ships it. If the app shows a blank window or `WebView2Loader` fails, install the Evergreen Runtime from Microsoft's WebView2 download page. Check an existing install:

```powershell
Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' | Select-Object pv
```

### Rust

Install via [rustup](https://rustup.rs). The default host toolchain on Windows is `stable-x86_64-pc-windows-msvc`; keep it.

```powershell
rustup default stable-msvc
rustc --version
cargo --version
```

No `rust-toolchain.toml` is pinned, so any recent stable works.

### Visual Studio Build Tools

Install **Visual Studio Build Tools 2022** with the **Desktop development with C++** workload. That pulls in the MSVC compiler, the Windows SDK and CMake, all of which the `windows` and `tauri` crates need. If `cargo build` fails with `link.exe not found`, this step is missing.

---

## 2. Install dependencies

From the repository root:

```powershell
npm install
```

That installs the front end (React 19, uPlot, Vite) and the Tauri CLI. Rust crates are fetched on the first build, not now.

If you use `fnm` for Node and `npm` is not found in a fresh shell:

```powershell
fnm env --use-on-cd --shell powershell | Out-String | Invoke-Expression
```

Other environment traps — `NODE_ENV=production` hiding devDependencies, npm blocking `esbuild`'s install script, and Cargo not being on `PATH` — are documented in [DEV.md](DEV.md).

---

## 3. Run it

```powershell
npm run tauri dev
```

That starts the Vite dev server on `http://localhost:1420`, compiles the Rust crate in debug mode (the first run takes several minutes), and opens the overlay plus a tray icon. Leave the terminal open; closing it stops the app.

The overlay starts click-through (every click goes to the game). Use the hotkeys:

| Keys | Action |
|---|---|
| `Ctrl+Alt+T` | Choose the game window |
| `Ctrl+Alt+M` | Toggle interactive (clickable) |
| `Ctrl+Alt+V` | Show / hide the overlay |
| `Ctrl+Alt+H` | History window |

Then run a one-time calibration — attach the window, freeze a frame, draw the boxes, learn the EXP digits, teach the map/name/job. Steps and pitfalls are in the README's *Calibrating* section.

---

## 4. Build a release binary

```powershell
npx tauri build --no-bundle   # single exe, no installer
npx tauri build               # NSIS installer
```

- Release exe: `src-tauri/target/release/mscout.exe` (self-contained: the front end is embedded).
- The **debug** exe is not self-contained — it needs `npm run dev` running alongside it.

The release build sets `codegen-units = 1`, `lto = true` and `strip = true`, so it is slow to compile and fast to run.

---

## 5. Where data goes

| Context | Path |
|---|---|
| Development | `./data/` in the repository root |
| Packaged | `data/` next to the executable |
| Override | `MSCOUT_DATA_DIR` environment variable |

Equivalent for **portable** use: copy the release exe plus its `data/` folder anywhere.

`data/` is gitignored because it contains personal state — your calibration bitmaps, character name, and captured play samples. See [DATA.md](DATA.md) for formats, and `data/profiles/<W>x<H>.json` for the per-resolution calibration.

---

## 6. Common problems

| Symptom | Cause |
|---|---|
| `npm` not recognised | Node not on `PATH` — see the `fnm env` line above |
| `link.exe not found` | Visual Studio Build Tools / C++ workload missing |
| Blank window | WebView2 runtime missing |
| Overlay shows `not calibrated for WxH` | that client size has no profile yet — run calibration |
| Overlay shows `unknown` map | that map was never taught at this resolution |
| Build fails with `EBUSY` on a DLL | a previous instance is still running; quit it from the tray first |
| Capture fails on fullscreen | the game must run windowed or borderless; exclusive fullscreen cannot be captured without hooking, which is out of scope |

---

## 7. Requirements on the game side

The helper never touches the game process: no memory reading, no injection, no input. It reads pixels via the OS compositor. That imposes two rules on the game:

- Run **windowed or borderless**. Exclusive fullscreen cannot be captured or overlaid.
- Run at a **client size you have calibrated**. Changing resolution switches to that size's profile; a size never calibrated needs one calibration pass.

See [SPEC.md §1](SPEC.md) for the full constraint list and the reasoning behind it.
