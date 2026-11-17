//! Enthuse desktop app: Slint UI and wiring.
//!
//! `enthuse [folder...] [file-or-url]`: folders become browser sources, and a
//! file or URL starts playing right away. `--self-test` plays a built-in test
//! pattern and checks a snapshot of the window, for CI.

mod browser;
mod playback;
mod selftest;
mod video;

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use enthuse_player::{MpvPlayer, Player};

slint::include_modules!();

const PAGE_BROWSER: i32 = 0;
const PAGE_PLAYER: i32 = 1;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("usage: enthuse [folder...] [file-or-url] | enthuse --self-test");
        return ExitCode::SUCCESS;
    }
    let self_test = args.iter().any(|a| a == "--self-test");
    let positional = args.iter().filter(|a| !a.starts_with("--"));
    let (folders, files): (Vec<&String>, Vec<&String>) =
        positional.partition(|a| Path::new(a).is_dir());
    let play = if self_test {
        Some(selftest::SOURCE.to_owned())
    } else {
        files.first().map(|f| f.to_string())
    };
    let folders: Vec<String> = folders.into_iter().cloned().collect();

    match run(&folders, play, self_test) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("enthuse: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(
    folders: &[String],
    play: Option<String>,
    self_test: bool,
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    // The video underlay needs Slint's OpenGL renderer.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into())
        .select()?;

    let options: &[(&str, &str)] = if self_test {
        &[
            // CI runners have no audio device and only software GL.
            ("ao", "null"),
            ("hwdec", "no"),
            // Allow Mesa's software OpenGL (llvmpipe), which mpv rejects by default.
            ("gpu-sw", "yes"),
            // Print mpv's log so CI failures can be diagnosed.
            ("terminal", "yes"),
            ("msg-level", "all=info"),
        ]
    } else {
        &[]
    };
    let player: Arc<dyn Player> = Arc::new(MpvPlayer::new(options)?);

    let window = AppWindow::new()?;
    let playback = playback::Playback::new(&window, player.clone());
    let browser = browser::Browser::new(&window);
    for folder in folders {
        browser.add_local(folder);
    }

    {
        let playback = playback.clone();
        let weak = window.as_weak();
        browser.set_play_handler(Box::new(move |source, path| {
            let Some(w) = weak.upgrade() else { return };
            match source.playable(&path) {
                Ok(url) => {
                    playback.open(&url, path.name().unwrap_or(&url), None);
                    w.set_page(PAGE_PLAYER);
                }
                Err(err) => w.set_browser_error(format!("Cannot play this file: {err}").into()),
            }
        }));
    }
    {
        let player = player.clone();
        let weak = window.as_weak();
        window.on_back(move || {
            let _ = player.stop();
            if let Some(w) = weak.upgrade() {
                w.set_page(PAGE_BROWSER);
            }
        });
    }

    let capture = video::ScreenCapture::default();
    // mpv needs the render context before a file is loaded, and the render
    // context needs Slint's GL context, which exists once the window is shown.
    // Files picked in the browser come later, so only the start file waits.
    {
        let playback = playback.clone();
        let start = play.map(|url| {
            window.set_page(PAGE_PLAYER);
            let title = title_for(&url);
            (url, title)
        });
        video::attach(
            &window,
            player.clone(),
            move || {
                if let Some((url, title)) = start {
                    playback.open(&url, &title, None);
                }
            },
            capture.clone(),
        )?;
    }

    let outcome = if self_test {
        selftest::start(&window, player.clone(), capture)
    } else {
        Default::default()
    };

    window.run()?;
    Ok(outcome.exit_code())
}

/// File name for display, from a path or URL.
fn title_for(source: &str) -> String {
    source
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(source)
        .to_owned()
}
