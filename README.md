# Enthuse

Infuse-style media player for Windows and Linux, built with Rust, Slint and libmpv.

Status: early prototype.

See [docs/design.md](docs/design.md) for goals and design decisions.

Enthuse is an independent project and is not affiliated with Firecore or Infuse.

## Building

Requires stable Rust (see `rust-toolchain.toml`) and libmpv.

- **Linux (Debian/Ubuntu):** `sudo apt-get install libmpv-dev libfontconfig1-dev libxkbcommon-dev`, then `cargo build`.
- **Windows (MSVC):** run `.github/scripts/fetch-libmpv.ps1 -Dest C:\libmpv` (needs the GitHub CLI and 7-Zip), set `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-L native=C:\libmpv"`, add `C:\libmpv` to `PATH`, then `cargo build`.

## License

Enthuse is licensed under the GNU General Public License v3.0. See [LICENSE](LICENSE).
