# Upgrade fixtures

Profiles reconstructed from the per-release `Profile` structs tagged `v0.1.0`,
`v0.2.0`, `v0.5.0` and `v0.6.0` — one file per shape the format went through.

They are **schema samples, not captures**: only the fields that release wrote are
present, and the hex payloads are synthetic repeating patterns. The file name
encodes the release that wrote it plus the `data/profiles/` name the file stands
in for (`[release]-<W>x<H>[@mode].json`).

See `docs/DATA.md` ("Upgrade fixtures") and the `profile::tests` module in
`src-tauri/src/profile.rs`. These files sit outside `testdata/` on purpose: the
regression frame walker must never read them as image pairs.
