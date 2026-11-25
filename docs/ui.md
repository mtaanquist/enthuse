# UI spec

This file describes the intent of the Enthuse UI: the look, the screens and the states the mockups don't show. The code is the reference for exact values: colors, type, spacing, sizes and radii live in `crates/enthuse-app/ui/theme.slint` (`Theme` and `Icons`), and the base components in `ui/components/`. When the two disagree, fix whichever is wrong and keep them in step.

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

All values are tokens in `Theme` (`ui/theme.slint`); components never use raw values. The intent behind them:

- **Colors.** Dark, layered backgrounds (`bg`, `sidebar`, `surface`, `surface-raised`), two border strengths, four text strengths, and one accent (`accent`, periwinkle) for primary actions, progress and the new-file dot. Overlays on video use translucent fills (`overlay-*`) and scrims, never blur. Text on `bg` and `surface` meets 4.5:1 for every text token; keep that true when adding colors. The accent is a single token so it can change later or become a user setting.
- **Type.** Inter, bundled under the OFL. A small set of styles: title, brand, section, body (strong and normal), label, meta and chip. Subtitles are drawn by mpv, never by the UI.
- **Spacing and shape.** A 4px-based spacing scale, 48px content padding (24px when narrow), 44px minimum touch targets, 56px transport and 60px PiP play buttons. Rounded corners throughout, fully round buttons and chips. Shadows only on things that float over other content (PiP, toasts, tooltips, menus), never on tiles.
- **Icons.** Our own SVGs on a 24px grid with a 1.75px stroke and round caps, drawn white and tinted with `colorize`, in `ui/icons/`.

## Components

Sizes are logical pixels. Every interactive component has the states listed under "States" below.

| Component | Size | Content and notes |
| --- | --- | --- |
| `NavItem` | 48px high, full sidebar width minus 32px | Icon 20px + label. Active: `surface-raised` background, `text`, weight 600. Inactive: transparent, `text-muted`. |
| `Sidebar` | 232px wide | Logo and app name, NavItems (Home, Libraries, Recent), spacer, Settings at the bottom. |
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
3. "Favourites", when there are any: starred libraries, then starred folders (with "Library / parent" under the name), as SourceTiles. Hidden when empty.
4. "Libraries" with "Manage" (to the Libraries page) and "Add library" buttons: grid of SourceTiles, 3 per row, favourites first, then the user's order. Libraries hidden from Home (the eye on the Libraries page) are left out of Home altogether, Favourites too.

Tapping a card resumes playback. Tapping a library opens it in the folder browser. A library tile's menu has "Add to / Remove from favourites" and "Manage libraries…"; a Favourites tile's menu only unstars it. Home is for watching: arranging libraries happens on the Libraries page.

### Libraries

The sidebar's Libraries item opens the list of all libraries, also those left off Home, with "Add library". Libraries are arranged here. Each row has a drag handle, its name and path, a star, an eye (shown on Home, or crossed out when left off), up and down buttons (within its group: favourites stay first) and a settings button. Tapping the row opens the folder browser at its root. Drag the handle to reorder; with a finger, rest on it a moment first, since a quick swipe scrolls the list.

The settings button opens the library's dialog over a scrim: its name (empty goes back to the folder's name), its own "Resume where you left off" (Same as Playback, On, Off), and "Remove library", which asks again since it forgets the watch state. Changes save at once, the name on Done or Enter. Escape, the scrim or Done close it. Later sources (SMB address and credentials) add their settings here. Coming back to Libraries from another section returns to the folder last browsed; clicking it again while there goes to the list. A library just added opens right away.

### Folder browser

Sidebar, then:
1. Header: back button, breadcrumb, a star (the folder shown as a favourite; at a library's root, the library), list or grid toggle, sort button ("Sort: Name", natural sort by default).
2. Folder title and summary ("8 episodes, 4 watched").
3. Primary action on the right: "Resume episode N" when a file in the folder has a resume position, else "Play next: E06" for the first unwatched file, else "Play from start". Hidden when the folder has no video files.
4. File list: folders first, then video files, in natural order. Non-video files are hidden.

The grid view shows the same items as 16:9 cards with the same badges.

### Playback

Video fills the window (drawn by libmpv under the Slint UI). Overlay, shown on mouse move or tap and hidden after 3 s without input while playing:
1. Top: back to folder, title and episode line, PiP button.
2. Bottom: Scrubber, then TransportBar.
3. UpNextCard when the file is within its last 20 s, a next file exists in the folder and auto-next is on (Settings). Counts down 10 s of playback (it pauses with the video), then marks the file watched and plays the next one. Cancel hides it for this file, and playback then stops at the end instead of going on. Seeking back out of the last 20 s hides it and restarts the countdown next time. Files of 20 s or less get no card. In PiP there is no card; the next file starts at the end as before, if auto-next is on.

Subtitles are rendered by mpv inside the video frame. The UI never draws them, and the overlay must not cover the bottom subtitle area when it can be avoided.

Keys: Space play or pause, Left and Right seek 10 s, Up and Down volume, F full screen, M mute, Escape closes a menu, then leaves full screen, then goes back. A tap on the video shows or hides the controls, and a double click toggles full screen. While the controls show, subtitles move up above them (mpv's `sub-pos`).

### Picture-in-picture

A separate borderless, always-on-top, freely resizable window that reuses the same player (see `docs/design.md`). Default size 560x315, bottom right of the current screen with a 48px margin. Aspect ratio is locked while resizing. Hover (or tap) shows PipControls; the progress line at the bottom stays visible. Drag anywhere outside a button to move the window. On Wayland, where always-on-top can't be requested, it is a normal borderless window (the README has a KDE window rule for it).

- The main window itself becomes the PiP player, so the video never moves between windows (#52). Corners are rounded (12px) where the OS allows it (Windows 11); elsewhere they stay square. Its title is "Enthuse PiP" so window manager rules can match it.
- Subtitles are drawn 1.7 times larger in PiP so they stay readable, and go back to normal on exit.
- Double-click, back-to-window or Escape returns to the full window. Close stops playback.
- The browser opens in a second window where the main window was, with a now-playing bar at the bottom: a seekable progress line along its top edge, back 10 s, play/pause, forward 30 s, title and time, "Back to video" and stop. Below 640px wide the seek buttons go and "Back to video" becomes an icon.

### Thumbnails

Video rows, grid cards, ContinueCards and the UpNextCard show a frame grabbed about 10% into the file (30% if that frame is black; the black one stays if that fails). Until one is ready, or when none can be made (an empty or broken file), they show the video icon.

- One background worker grabs frames one at a time with a headless libmpv, so browsing never waits on it. Home cards and Up next go first, then the folder being browsed in listing order; leaving a folder drops its pending grabs.
- Thumbnails are 256px wide JPEGs in the cache folder (`$XDG_CACHE_HOME/enthuse/thumbnails`, `%LOCALAPPDATA%\Enthuse\cache\thumbnails`), keyed by path, size and modification time, so a replaced file gets a new one. The ones used longest ago go when the cache passes 200 MB (checked at start and every 200 jobs). Files mpv can read but finds no frame in (empty, broken) are remembered and not tried again until they change; a grab that times out (20 s) or cannot read the file is tried again next session. A job stuck on a dead share is given up after 40 s so the rest keep coming, and Home asks for none from shares it found offline.
- Squared pixels: anamorphic video (a 16:9 DVD) is scaled to its display aspect before the thumbnail is made.
- Thumbnails for files on network shares can be turned off in Settings (`network_thumbnails`).


Decided 2026-11-17:
- Main window: keep the native OS frame and request a dark title bar. No custom title bar for now.
- PiP: frameless (`no-frame: true`), with our own controls and drag-to-move.
- Fullscreen playback hides the frame.

### Recent

Sidebar, then the title "Recent" and up to 50 recently played files, watched or not, most recent first. Rows look like folder rows: thumbnail, "Harbor Lights, S02E06 Lantern Rock", "Harbor Lights / Season 2, 3 h ago", and the Watched or time-left badge. Tapping a row plays or resumes it. Files of removed sources are left out. When nothing has been played: "Nothing played yet."

### Settings

Sidebar, then one column (at most 820px wide) of grouped cards:
1. Playback:
   - "Resume where you left off" switch. It is the global default; folders can override it from the Resume button in Browse.
   - "Play the next file automatically" switch. Off: no Up next card, and playback returns to the browser when a file ends. The next button still works.
   - "Mark as watched": a segmented control (Percentage, Time left, Whichever first) and steppers for the share played (50% to 98%) and the time left (30 s to 15 min). Playing to the end always counts. The time-left test waits until at most a quarter of the file is left, so a short clip is not marked watched early.
2. Folder settings: every folder with its own settings, as "Kids" over "NAS / Shows / Kids, resume off", with a "Clear" button. Editing stays in the browser.
3. Thumbnails: "Thumbnails for network shares" switch (`network_thumbnails`).
4. About: version, GPL-3.0, "Not affiliated with Firecore or Infuse."

Every change is saved at once; there is no Save button.

## States not shown in the mockups

| Situation | Behavior |
| --- | --- |
| Hover | Tiles, rows and nav items: background one step lighter (`surface` becomes `surface-raised`, transparent becomes `surface`). Buttons: brighten 10%. Animate 120ms. |
| Pressed | Darken 10% and no animation delay. |
| Keyboard focus | 2px accent outline, 2px offset, on every focusable element (drawn inside rows that sit in a clipped card). Tab moves between elements. Arrow keys move within a list, grid, the Home cards and tiles, and the sidebar; with nothing focused, an arrow key focuses the first visible item. Enter or Space activates, Escape goes back. Buttons that stay on screen (steppers, switches, segments) keep focus after a press. |
| Disabled | 40% opacity, no hover. |
| Loading a folder | Keep the header, show 6 placeholder rows (`surface`, no shimmer). Show them only if loading takes over 150ms. |
| Empty folder | Centered message "No videos in this folder" in `text-muted`, with a "Back" button. |
| No libraries yet (first run) | Home shows one large prompt in place of the libraries grid: "Add a folder or network share to get started" with the "Add library" primary button. Continue watching is hidden. |
| Source offline or unreachable | Tile shows the kind chip as "Offline" and dimmed content. Opening it shows "Can't reach NAS media" with "Retry" and "Edit source". |
| Credentials needed (SMB) | Dialog with user name, password, "Remember" and "Connect" (see #21). |
| File missing or failed to open | Toast at the bottom: "Couldn't play E05" with the reason from the player, and the browser stays where it was. |
| Playback error mid-file | Overlay stays visible with the message and "Retry" and "Back". |
| Buffering | 32px spinner in the center over the video; controls stay usable. |
| Long names | Titles ellipsize at the end on one line. Resting the pointer on the row for 0.7 s shows the full name in a tooltip under it (`TipText`). The tip is drawn by a layer over the screens, so it never catches clicks. |
| Narrow window | Below 900px wide the sidebar collapses to a 72px icon rail, Continue watching shows 2 or 3 cards, libraries 2 per row. |

## Slint notes

- `.slint` files are compiled into Rust at build time (`slint-build`). Keep them in `crates/enthuse-app/ui/`.
- Build our own components from `Rectangle`, `Text`, `Image`, `TouchArea` and layouts, reading values from `Theme`. Use the built-in `fluent-dark` style only for the few standard widgets we keep, such as `ScrollView` and `LineEdit`.
- Everything in the mockups is supported: rounded corners, borders, drop shadows, linear gradients, opacity, clipping and `animate`. Slint has no backdrop blur, so overlays use translucent fills, not frosted glass.
- Keep logic (sorting, badges, next file, resume) in `enthuse-core`. The UI only displays the state it is given.
- Tokens live in `crates/enthuse-app/ui/theme.slint` (`Theme` and `Icons`), base components in `ui/components/`. Run `cargo run -p enthuse-app --example gallery` to see them all in one window.
- Inter is bundled from `ui/fonts/` under the SIL Open Font License (`ui/fonts/OFL.txt`). Slint (1.18 and 1.19) can't turn on OpenType features, so times don't use tabular figures yet; give time labels a fixed width so they don't shift the layout.
