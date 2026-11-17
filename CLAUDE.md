# Enthuse

Infuse-style media player for Windows and Linux. Browse folders on local disks and network shares, play with libmpv, resume where you left off, and continue automatically to the next file in the folder.

See `docs/design.md` for the reasoning behind these choices, rejected options and the rough plan.

## Stack

- Rust (stable, edition 2024), Cargo workspace.
- UI: Slint. Native rendering only; no web views or browser engines.
- Playback: libmpv via its render API (OpenGL), drawn under the Slint UI so controls and overlays can sit on top of the video.
- Storage: SQLite (rusqlite) for sources, watch state and resume positions.

## Layout (planned)

- `crates/enthuse-core`: library, sources, watch state, natural sorting, next-file logic. No UI or libmpv dependency, fully unit tested.
- `crates/enthuse-player`: the `Player` trait and the libmpv implementation. Keep the trait small so another backend (e.g. libVLC) can be added later.
- `crates/enthuse-app`: Slint UI and wiring.

## Rules

- Must build and run on both Windows and Linux. Put platform-specific code behind `cfg(target_os = ...)` in small, isolated modules.
- Sources go through the `Source` trait. `LocalFs` covers local folders, Linux mounts and Windows UNC paths. A built-in SMB source comes later.
- Playlist and auto-next logic live in `enthuse-core`, not in libmpv's playlist, so behavior is the same for any backend.
- Let the player draw subtitles. Do not render subtitles in the UI layer.
- PiP is a separate borderless, always-on-top, freely resizable window that reuses the same player. Wayland cannot guarantee always-on-top; degrade gracefully.
- Keep the UI touch-friendly.
- Branding: the name nods to Infuse on purpose, but the logo, icons, colors and visual style must stay clearly our own. Do not copy Infuse's assets, layouts or look.

## Before opening a PR

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

## Migrations

Database schema changes go in numbered migrations (rusqlite_migration). Never edit a migration that has been merged to `main`; add a new one.

## Do not touch

- Committed migrations (see above).
- CI workflow files, unless the issue is about CI.
