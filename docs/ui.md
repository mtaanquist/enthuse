# UI spec

This is the reference for building the Enthuse UI in Slint. The mockups in `docs/mockups/` show the target look; this file gives the exact values and covers the states the mockups don't show. Once `theme.slint` and the base components exist, the code is the reference and this file should only describe intent.

Design direction: dark, flat and minimal, with one accent color. Touch-friendly. Clearly our own look (see the branding rule in `CLAUDE.md`).

Tracked in #42.

## Mockups

| File | Screen |
| --- | --- |
| [1-home.png](mockups/1-home.png) | Home: continue watching and sources |
| [2-folder-browser.png](mockups/2-folder-browser.png) | Folder browser with watched, resume and new badges |
| [3-playback.png](mockups/3-playback.png) | Playback with overlay controls and the up-next card |
| [4-pip.png](mockups/4-pip.png) | Picture-in-picture window with hover controls |
| [5-slint-primer.png](mockups/5-slint-primer.png) | How the design maps to Slint |

The mockups are drawn at 1280x800 logical pixels (exported at 2x). Titles, file names and sources in them are made up.

## Tokens

Put these in one `export global Theme` in `theme.slint`. Components never use raw values.

### Colors

| Token | Value | Use |
| --- | --- | --- |
| `bg` | `#0f1115` | Window background |
| `sidebar` | `#13161b` | Side navigation background |
| `surface` | `#171a20` | Tiles, cards, highlighted rows, code blocks |
| `surface-raised` | `#1f232b` | Selected nav item, icon wells, chips |
| `line` | `#22262e` | Borders on tiles, sidebar divider |
| `line-strong` | `#2a2f38` | Outline buttons, active segment, watched badge |
| `text` | `#eceef2` | Primary text |
| `text-secondary` | `#c9ced8` | Section headings |
| `text-muted` | `#9aa3b2` | Metadata, inactive nav, paths |
| `text-faint` | `#8a93a3` | Row metadata, badge labels |
| `accent` | `#8ea2ff` | Primary buttons, progress, new-file dot, logo |
| `on-accent` | `#0f1115` | Text and icons on the accent |
| `scrim-top` | `rgba(0,0,0,0.65)` to transparent | Gradient behind the player header |
| `scrim-bottom` | transparent to `rgba(0,0,0,0.78)` | Gradient behind the player controls |
| `overlay-button` | `rgba(255,255,255,0.12)` | Round buttons on top of video |
| `overlay-card` | `rgba(18,20,25,0.92)` | Cards on top of video (up next, menus) |
| `danger` | `#f28b82` | Errors (not in the mockups) |

Accent alternatives that were offered: `#5fd4c4`, `#f2b36b`, `#e58fb6`. The accent is a single token so it can change later, or become a user setting.

Text on `bg` and `surface` meets 4.5:1 for all text tokens above. Keep that true when adding colors.

### Type

Font: Inter, bundled with the app (`import "fonts/Inter-*.ttf";` in Slint). Use tabular figures for times.

| Style | Size | Weight | Use |
| --- | --- | --- | --- |
| `title` | 28px | 700, letter-spacing -0.02em | Screen titles ("Home", "Season 2") |
| `brand` | 18px | 700 | App name in the sidebar |
| `section` | 17px | 600 | Section headings, player title |
| `body-strong` | 15px | 600 | Tile titles, row titles, nav (active) |
| `body` | 15px | 500 | Nav (inactive), watched row titles |
| `label` | 14px | 500 | Buttons, breadcrumbs, secondary lines |
| `meta` | 13px | 400 | Metadata, times |
| `chip` | 12px | 500 | Source kind chips, "Up next in 8 s" |
| `subtitle` | n/a | n/a | Drawn by mpv, never by the UI |

### Spacing, size and shape

- Spacing scale: 4, 8, 12, 16, 20, 24, 28, 36, 48 px.
- Content padding: 48px left and right, 28 to 36px top.
- Minimum touch target: 44x44px. Primary transport button: 56px. PiP play button: 60px.
- Radii: 6px (small thumbnails), 10px (nav items, cards, thumbnails), 12px (tiles, PiP window), 14px (overlay cards), fully round for buttons and chips.
- Shadows: only on floating windows and cards over other content (PiP: `0 18px 48px rgba(0,0,0,0.45)`). No shadows on tiles.
- Icons: 24px grid, 1.75px stroke, round caps and joins, drawn at 20 to 22px. Use our own SVGs, kept in `crates/enthuse-app/ui/icons/`.

## Components

Sizes are logical pixels. Every interactive component has the states listed under "States" below.

| Component | Size | Content and notes |
| --- | --- | --- |
| `NavItem` | 48px high, full sidebar width minus 32px | Icon 20px + label. Active: `surface-raised` background, `text`, weight 600. Inactive: transparent, `text-muted`. |
| `Sidebar` | 232px wide | Logo and app name, NavItems (Home, Browse, Recent), spacer, Settings at the bottom. |
| `IconButton` | 44px round (48px over video) | Outline variant (`line-strong` border) on app screens, filled variant (`overlay-button`) over video. Needs an accessible label. |
| `PillButton` | 40 to 48px high | Variants: primary (accent fill, `on-accent` text), outline, overlay. Optional leading icon. |
| `SegmentedControl` | 44px high | List or grid view. Selected segment `line-strong`, others transparent with `text-muted` icon. |
| `Breadcrumb` | single line | Path segments separated by "/". Last segment `text`, weight 600. Earlier segments are tappable. Truncate from the left when too long. |
| `ContinueCard` | 16:9 thumbnail, 4 per row | Thumbnail (frame grab now, poster later per #10), round play badge 36px, 4px progress bar at the bottom, title and "S02E05, 31 min left". |
| `SourceTile` | 84px high, 3 per row | Icon well 44px (local, share or media server icon), name, detail (path or address, ellipsized), kind chip (Local, SMB, Plex, Jellyfin). |
| `FileRow` | 64px high | Thumbnail 80x45, title, meta ("44 min, 1080p, HEVC"), state badge on the right (see below). Folder rows use a folder icon in place of the thumbnail. |
| `StateBadge` | 140px wide area, right aligned | Watched: "Watched" + check in a 24px `line-strong` circle, row title dimmed. Resume: "31 min left" in accent + 3px progress bar on the thumbnail, row highlighted with `surface`. New: "New" + 10px accent dot. |
| `Scrubber` | 24px hit area, 4px track | Track `rgba(255,255,255,0.22)`, buffered `rgba(255,255,255,0.4)`, played accent, 16px white thumb. Elapsed time left, remaining time (negative) right. |
| `TransportBar` | 56px | Play/pause (56px white), back 10 s, forward 30 s, next file, volume, spacer, subtitles and audio pill, speed pill, fullscreen. |
| `UpNextCard` | 340px wide | Thumbnail 96x54, "Up next in N s", next title, "Play now" (primary) and "Cancel". Top right under the player header so it never covers subtitles. |
| `PipControls` | covers the PiP window | Scrim `rgba(0,0,0,0.42)`, back-to-window and close (44px) at the top corners, back 10 s, play/pause (60px), forward 30 s in the center, 4px progress line at the bottom edge, resize grip bottom right. |

## Screens

### Home

Sidebar, then:
1. Title "Home" and a search button.
2. "Continue watching": up to 4 cards in one row, most recent first. Hidden when empty.
3. "Sources" with an "Add source" button: grid of SourceTiles, 3 per row.

Tapping a card resumes playback. Tapping a source opens the folder browser at its root.

### Folder browser

Sidebar, then:
1. Header: back button, breadcrumb, list or grid toggle, sort button ("Sort: Name", natural sort by default).
2. Folder title and summary ("8 episodes, 4 watched").
3. Primary action on the right: "Resume episode N" when a file in the folder has a resume position, else "Play next: E06" for the first unwatched file, else "Play from start". Hidden when the folder has no video files.
4. File list: folders first, then video files, in natural order. Non-video files are hidden.

The grid view shows the same items as 16:9 cards with the same badges.

### Playback

Video fills the window (drawn by libmpv under the Slint UI). Overlay, shown on mouse move or tap and hidden after 3 s without input while playing:
1. Top: back to folder, title and episode line, PiP button.
2. Bottom: Scrubber, then TransportBar.
3. UpNextCard when the file is within its last 20 s and a next file exists in the folder. Counts down 10 s of playback (it pauses with the video), then marks the file watched and plays the next one. Cancel hides it for this file, and playback then stops at the end instead of going on. Seeking back out of the last 20 s hides it and restarts the countdown next time. Files of 20 s or less get no card. In PiP there is no card; the next file starts at the end as before.

Subtitles are rendered by mpv inside the video frame. The UI never draws them, and the overlay must not cover the bottom subtitle area when it can be avoided.

Keys: Space play or pause, Left and Right seek 10 s, Up and Down volume, F full screen, M mute, Escape closes a menu, then leaves full screen, then goes back. A tap on the video shows or hides the controls, and a double click toggles full screen. While the controls show, subtitles move up above them (mpv's `sub-pos`).

### Picture-in-picture

A separate borderless, always-on-top, freely resizable window that reuses the same player (see `docs/design.md`). Default size 560x315, bottom right of the current screen with a 48px margin. Aspect ratio is locked while resizing. Hover (or tap) shows PipControls; the progress line at the bottom stays visible. Drag anywhere outside a button to move the window. On Wayland, where always-on-top can't be requested, it is a normal borderless window.

## Window chrome

Decided 2026-11-17:
- Main window: keep the native OS frame and request a dark title bar. No custom title bar for now.
- PiP: frameless (`no-frame: true`), with our own controls and drag-to-move.
- Fullscreen playback hides the frame.

## States not shown in the mockups

| Situation | Behavior |
| --- | --- |
| Hover | Tiles, rows and nav items: background one step lighter (`surface` becomes `surface-raised`, transparent becomes `surface`). Buttons: brighten 10%. Animate 120ms. |
| Pressed | Darken 10% and no animation delay. |
| Keyboard focus | 2px accent outline, 2px offset, on every focusable element. Arrow keys move through grids and lists, Enter activates, Escape goes back. |
| Disabled | 40% opacity, no hover. |
| Loading a folder | Keep the header, show 6 placeholder rows (`surface`, no shimmer). Show them only if loading takes over 150ms. |
| Empty folder | Centered message "No videos in this folder" in `text-muted`, with a "Back" button. |
| No sources yet (first run) | Home shows one large prompt in place of the sources grid: "Add a folder or network share to get started" with the "Add source" primary button. Continue watching is hidden. |
| Source offline or unreachable | Tile shows the kind chip as "Offline" and dimmed content. Opening it shows "Can't reach NAS media" with "Retry" and "Edit source". |
| Credentials needed (SMB) | Dialog with user name, password, "Remember" and "Connect" (see #21). |
| File missing or failed to open | Toast at the bottom: "Couldn't play E05" with the reason from the player, and the browser stays where it was. |
| Playback error mid-file | Overlay stays visible with the message and "Retry" and "Back". |
| Buffering | 32px spinner in the center over the video; controls stay usable. |
| Long names | Titles ellipsize at the end on one line; full name in a tooltip on hover. |
| Narrow window | Below 900px wide the sidebar collapses to a 72px icon rail, Continue watching shows 2 or 3 cards, sources 2 per row. |

## Slint notes

- `.slint` files are compiled into Rust at build time (`slint-build`). Keep them in `crates/enthuse-app/ui/`.
- Build our own components from `Rectangle`, `Text`, `Image`, `TouchArea` and layouts, reading values from `Theme`. Use the built-in `fluent-dark` style only for the few standard widgets we keep, such as `ScrollView` and `LineEdit`.
- Everything in the mockups is supported: rounded corners, borders, drop shadows, linear gradients, opacity, clipping and `animate`. Slint has no backdrop blur, so overlays use translucent fills, not frosted glass.
- Keep logic (sorting, badges, next file, resume) in `enthuse-core`. The UI only displays the state it is given.
- Tokens live in `crates/enthuse-app/ui/theme.slint` (`Theme` and `Icons`), base components in `ui/components/`. Run `cargo run -p enthuse-app --example gallery` to see them all in one window.
- Inter is bundled from `ui/fonts/` under the SIL Open Font License (`ui/fonts/OFL.txt`). Slint 1.19 can't turn on OpenType features, so times don't use tabular figures yet; give time labels a fixed width so they don't shift the layout.
