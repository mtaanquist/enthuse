# Enthuse

Infuse-style media player for Windows and Linux, built with Rust, Slint and libmpv.

Status: early release (0.x). Things may change between versions, including settings and stored data. Downloads are on the [Releases](https://github.com/mtaanquist/enthuse/releases) page.

See [docs/design.md](docs/design.md) for goals and design decisions.

Enthuse is an independent project and is not affiliated with Firecore or Infuse.

## Building

Requires stable Rust (see `rust-toolchain.toml`) and libmpv.

- **Linux (Debian/Ubuntu):** `sudo apt-get install libmpv-dev libfontconfig1-dev libxkbcommon-dev`, then `cargo build`.
- **Windows (MSVC):** run `.github/scripts/fetch-libmpv.ps1 -Dest C:\libmpv` (needs the GitHub CLI and 7-Zip), set `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-L native=C:\libmpv"`, add `C:\libmpv` to `PATH`, then `cargo build`.

## Picture-in-picture on Wayland

Wayland does not let apps keep a window on top or place it, so the PiP player opens as a normal borderless window. On KDE Plasma you can keep it on top with a window rule: System Settings, Window Management, Window Rules, Add New. Set "Window title" to Exactly Match "Enthuse PiP" (the PiP window's title), then add the property "Keep above other windows", set to Force. The window only gets that title when it turns into the PiP player, so whether the rule applies depends on KWin re-checking rules on a title change; treat this as best effort. On X11 and Windows PiP stays on top by itself.

## License

Enthuse is licensed under the GNU General Public License v3.0. See [LICENSE](LICENSE).
