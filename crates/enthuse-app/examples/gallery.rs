//! Shows the theme and base components in one window, to check them by eye
//! (for example at 125% scaling on Windows).
//!
//! `cargo run -p enthuse-app --example gallery`
//!
//! With `--snapshot <file.ppm>` it saves the window as a PPM image and quits.

use slint::{ComponentHandle, Timer};

slint::slint! {
    import { Theme, Icons } from "../ui/theme.slint";
    import { ButtonVariant, IconButton, PillButton } from "../ui/components/buttons.slint";
    import { SegmentedControl } from "../ui/components/segmented-control.slint";
    import { NavItem } from "../ui/components/nav-item.slint";

    export component Gallery inherits Window {
        title: "Enthuse components";
        preferred-width: 900px;
        preferred-height: 640px;
        background: Theme.bg;
        default-font-family: Theme.font-family;
        default-font-size: Theme.body-size;

        property <int> nav: 0;
        property <int> view: 0;

        HorizontalLayout {
            Rectangle {
                width: Theme.sidebar-width;
                background: Theme.sidebar;

                VerticalLayout {
                    padding: Theme.space-16;
                    spacing: Theme.space-4;
                    alignment: start;

                    NavItem { icon: Icons.home; text: "Home"; active: root.nav == 0; clicked => { root.nav = 0; } }
                    NavItem { icon: Icons.folder; text: "Browse"; active: root.nav == 1; clicked => { root.nav = 1; } }
                    NavItem { icon: Icons.recent; text: "Recent"; active: root.nav == 2; clicked => { root.nav = 2; } }
                    NavItem { icon: Icons.settings; text: "Settings"; compact: true; active: root.nav == 3; clicked => { root.nav = 3; } }
                }

                Rectangle {
                    x: parent.width - 1px;
                    width: 1px;
                    background: Theme.line;
                }
            }

            VerticalLayout {
                padding-left: Theme.content-padding-x;
                padding-right: Theme.content-padding-x;
                padding-top: Theme.content-padding-top;
                spacing: Theme.space-24;
                alignment: start;

                Text {
                    text: "Components";
                    color: Theme.text;
                    font-size: Theme.title-size;
                    font-weight: Theme.title-weight;
                    letter-spacing: Theme.title-letter-spacing;
                }

                Text { text: "Pill buttons"; color: Theme.text-secondary; font-size: Theme.section-size; font-weight: Theme.section-weight; }
                HorizontalLayout {
                    spacing: Theme.space-12;
                    alignment: start;
                    PillButton { text: "Resume episode 5"; icon: Icons.play; variant: ButtonVariant.primary; size: 48px; }
                    PillButton { text: "Add source"; icon: Icons.add; }
                    PillButton { text: "Sort: Name"; }
                    PillButton { text: "Disabled"; enabled: false; }
                }

                Text { text: "Icon buttons"; color: Theme.text-secondary; font-size: Theme.section-size; font-weight: Theme.section-weight; }
                HorizontalLayout {
                    spacing: Theme.space-12;
                    alignment: start;
                    IconButton { icon: Icons.back; label: "Back"; }
                    IconButton { icon: Icons.search; label: "Search"; }
                    IconButton { icon: Icons.add; label: "Add"; variant: ButtonVariant.primary; }
                    SegmentedControl {
                        segments: [{ icon: Icons.list, label: "List" }, { icon: Icons.grid, label: "Grid" }];
                        selected <=> root.view;
                    }
                }

                Text { text: "Over video"; color: Theme.text-secondary; font-size: Theme.section-size; font-weight: Theme.section-weight; }
                Rectangle {
                    height: 120px;
                    border-radius: Theme.radius-large;
                    background: @linear-gradient(90deg, #3a4a5e 0%, #5e4a3a 100%);
                    clip: true;

                    Rectangle { y: parent.height - self.height; height: 96px; background: Theme.scrim-bottom; }

                    HorizontalLayout {
                        padding: Theme.space-16;
                        spacing: Theme.space-12;
                        alignment: start;
                        IconButton { icon: Icons.play; label: "Play"; variant: ButtonVariant.light; size: Theme.transport-button-size; }
                        IconButton { icon: Icons.back-10; label: "Back 10 seconds"; variant: ButtonVariant.overlay; }
                        IconButton { icon: Icons.forward-30; label: "Forward 30 seconds"; variant: ButtonVariant.overlay; }
                        IconButton { icon: Icons.next; label: "Next"; variant: ButtonVariant.overlay; }
                        IconButton { icon: Icons.volume; label: "Volume"; variant: ButtonVariant.overlay; }
                        PillButton { text: "Subtitles"; icon: Icons.subtitles; variant: ButtonVariant.overlay; }
                        IconButton { icon: Icons.pip; label: "Picture in picture"; variant: ButtonVariant.overlay; }
                        IconButton { icon: Icons.fullscreen; label: "Full screen"; variant: ButtonVariant.overlay; }
                    }
                }

                Text { text: "Icons"; color: Theme.text-secondary; font-size: Theme.section-size; font-weight: Theme.section-weight; }
                HorizontalLayout {
                    spacing: Theme.space-12;
                    alignment: start;
                    for icon in [Icons.play, Icons.pause, Icons.back-10, Icons.forward-30, Icons.next, Icons.volume, Icons.volume-mute,
                                 Icons.subtitles, Icons.audio, Icons.speed, Icons.fullscreen, Icons.fullscreen-exit, Icons.pip, Icons.pip-exit,
                                 Icons.folder, Icons.home]: Image {
                        width: Theme.icon-size-large; height: Theme.icon-size-large; source: icon; colorize: Theme.text-muted;
                    }
                }
                HorizontalLayout {
                    spacing: Theme.space-12;
                    alignment: start;
                    for icon in [Icons.recent, Icons.settings, Icons.search, Icons.add, Icons.minus, Icons.back, Icons.chevron-right, Icons.close,
                                 Icons.check, Icons.list, Icons.grid, Icons.drive, Icons.server, Icons.library, Icons.video, Icons.more]: Image {
                        width: Theme.icon-size-large; height: Theme.icon-size-large; source: icon; colorize: Theme.accent;
                    }
                }

                Text {
                    text: "8 episodes, 4 watched. 1:23:45 / -0:31:07";
                    color: Theme.text-muted;
                    font-size: Theme.meta-size;
                }
            }
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let gallery = Gallery::new()?;
    let mut args = std::env::args().skip(1);
    if let (Some(flag), Some(path)) = (args.next(), args.next())
        && flag == "--snapshot"
    {
        let weak = gallery.as_weak();
        Timer::single_shot(std::time::Duration::from_millis(500), move || {
            let Some(gallery) = weak.upgrade() else {
                return;
            };
            match gallery.window().take_snapshot() {
                Ok(buffer) => {
                    let mut out =
                        format!("P6\n{} {}\n255\n", buffer.width(), buffer.height()).into_bytes();
                    for p in buffer.as_slice() {
                        out.extend([p.r, p.g, p.b]);
                    }
                    if let Err(err) = std::fs::write(&path, out) {
                        eprintln!("gallery: cannot write {path}: {err}");
                    }
                }
                Err(err) => eprintln!("gallery: snapshot failed: {err}"),
            }
            let _ = slint::quit_event_loop();
        });
    }
    gallery.run()
}
