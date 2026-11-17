# Enthuse: design notes and decisions

This document records why Enthuse exists, what it should do, and the technical decisions made before the first line of code. Update it when a decision changes, and link the GitHub issue where the change was discussed.

## Why

No Windows or Linux player does what Infuse does on iOS and macOS. Infuse has never been released for Windows. The alternatives tried (Plex Desktop/HTPC, Plezy, Kodi with PlexKodiConnect, VLC, MPV front ends, Jellyfin clients) each miss part of the experience.

The parts of Infuse that matter most:

- A file browser inside the app for network shares, not only a scanned library.
- Playing a file and continuing automatically to the next file in the same folder.
- Resuming where you left off.
- Optional Plex integration.

## Goals

- Runs on Windows and Linux.
- Native UI. No CEF, Electron or system web view (rules out Tauri and Dioxus-on-webview).
- Browse local folders and network shares, play, resume, auto-next.
- Picture-in-picture window with no size limit and subtitles visible.
- Touch-friendly.
- Later: metadata and posters (TMDB), Plex libraries with watch-state sync, built-in SMB client, network share discovery.

## Non-goals (for now)

- macOS, mobile or TV platforms.
- Being a media server. Enthuse is a client.
- DVD/Blu-ray menus.

## Decisions

### Language and UI: Rust + Slint

Options considered: .NET (WinUI 3, WPF, Avalonia), Flutter, and Rust (Slint, egui, iced, GPUI, Tauri/Dioxus).

- Rust is the preferred language for desktop apps.
- WinUI 3 is Windows-only, has no built-in way to host a native video window (no HwndHost equivalent), and its built-in compact overlay (PiP) is capped at small preset sizes.
- Flutter with media_kit was a strong option, but Rust was preferred.
- Slint was picked because it renders natively, looks polished, handles touch, and supports an OpenGL underlay: the video is drawn underneath and the Slint UI on top. That's what's needed for overlay controls.
- egui is the fallback for quick experiments. It's simpler, but looks more like a tool than a consumer app.
- iced renders through wgpu, which makes OpenGL video interop harder. GPUI is still mostly shaped around the Zed editor.

### Playback: libmpv (render API), behind a `Player` trait

libmpv and libVLC were compared.

- Codecs: effectively the same. Both decode through FFmpeg.
- libmpv advantages: better scaling and HDR-to-SDR tone mapping (this matters on SDR displays, where the player does the conversion), proper Dolby Vision profile 5/8 handling (profile 5 can show green/purple tints in players without support), very configurable, and good Rust bindings (`libmpv2` crate) including the render API.
- libVLC advantages: built-in cross-platform SMB (`smb://`) and network discovery, DVD/Blu-ray menus, and LibVLCSharp is excellent for .NET. Its Rust bindings are thin and less maintained.
- Since the project is Rust, libmpv wins. A small `Player` trait (facade) keeps the door open to a libVLC backend for comparison.

Facade rules:
- Keep the trait small: open, play, pause, seek, position, duration, volume, tracks, events.
- Use our own `Track` model instead of library track IDs.
- Allow an escape hatch (`set_option(key, value)`) for backend-specific tuning.
- Playlist and auto-next logic lives in our code, not in the backend's playlist, so behavior is identical across backends.

### Video embedding: render to texture, not a child window

- A native child window (HWND/X11 window) is easiest, but UI can't draw over it (the "airspace" problem), which breaks overlay controls.
- So we use mpv's OpenGL render API and draw into Slint's underlay. UI and overlays then composite on top of the video.

### Subtitles

The player draws subtitles into the video frame. The UI never draws them. This way subtitles appear everywhere the video does, including PiP. That's the bug in Plex's app this avoids. Consider a larger subtitle scale while in PiP, since mpv scales subtitles with the window size.

### Picture-in-picture

- The main window turns into a borderless, always-on-top, freely resizable mini player and back, reusing the same player instance. Don't hand playback over to a new player.
- Not a second window (decided 2026-11-17, #49): mpv allows one render context and each window has its own GL context, so moving the video between windows makes mpv reinit and re-seek, which stalls playback for about half a second or more. A separate window stays open as an idea in #52.
- Move the window ourselves where possible: the OS move loop on Windows blocks the event loop and makes the video stutter. Wayland needs the compositor's move.
- Keep the aspect ratio while resizing (on Windows, handle `WM_SIZING`).
- Hover overlay with play/pause and "back to full window".
- Windows and X11 support always-on-top. Wayland doesn't let apps request it (KDE users can set a window rule; GNOME has no option). Degrade gracefully.

### Sources and SMB

- All browsing goes through a `Source` trait.
- First source: `LocalFs`. It covers local folders, Windows UNC paths (`\\nas\share`, using the OS SMB client and `WNetAddConnection2` for credentials) and Linux mounts (fstab/CIFS, GVfs/KIO).
- Later: `SmbSource` with a built-in client (libsmbclient bindings such as `pavao`, or a pure-Rust SMB crate) feeding libmpv through stream callbacks (`mpv_stream_cb_add_ro`). That gives the same `smb://` experience on both OSes without mounts. Getting seeking and buffering right is the main work.
- Network share discovery (mDNS etc.) is later. Adding a share by address comes first, as in Infuse.

### Auto-next and ordering

- When playback ends, open the next video file in the same folder.
- Use natural sorting (episode 10 after episode 9, not after episode 1).
- Watch state and resume positions are stored in SQLite, keyed by source and path.

### Branding

The name nods to Infuse on purpose. The logo, icons, colors and visual style must be clearly our own. Don't copy Infuse's assets or layouts. The README should say it's not affiliated with Firecore.

## Rejected or parked

| Option | Why |
| --- | --- |
| WinUI 3 | Windows-only, no native window hosting, PiP size cap |
| Tauri, Dioxus (webview) | Uses a web view; not native |
| libVLC as primary backend | Weak Rust bindings; image quality behind mpv. Parked as a possible second backend |
| Native child window for video | Airspace problem blocks overlay UI |
| mpv + scripts (autoload.lua, mpv-file-browser) | A useful zero-code way to test the workflow, but not a real app |

## Rough plan

1. Workspace scaffold (`enthuse-core`, `enthuse-player`, `enthuse-app`) and CI on Windows and Linux.
2. `LocalFs` source and folder browser.
3. libmpv player through the render API with Slint underlay, basic controls.
4. Auto-next, resume, watch state (SQLite).
5. PiP window.
6. Later: TMDB metadata and posters, Plex integration, built-in SMB source, share discovery, optional libVLC backend.

## Open questions

- Exact Slint and libmpv render API integration on each platform (check current examples before committing).
- Plex integration scope: browse only, or two-way watch-state sync?
- Packaging: bundled libmpv on Windows, Flatpak or system libmpv on Linux.
