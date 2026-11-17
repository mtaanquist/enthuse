//! Enthuse desktop app: Slint UI and wiring.
//!
//! `enthuse [folder...] [file-or-url]`: folders become browser sources, and a
//! file or URL starts playing right away. `--self-test` plays a built-in test
//! pattern and checks a snapshot of the window, for CI.

mod bg;
mod browser;
mod nav;
mod pip;
mod pip_geometry;
mod playback;
mod pointer;
mod selftest;
mod video;

use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use enthuse_core::paths::AppDirs;
use enthuse_core::storage::Store;

use enthuse_player::{MpvPlayer, Player};

slint::include_modules!();

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
            // mpv 0.37's full renderer on llvmpipe often draws only black
            // (#27); the basic path is reliable and still exercises the
            // render API and the compositing under Slint.
            ("gpu-dumb-mode", "yes"),
            // Print mpv's log so CI failures can be diagnosed.
            ("terminal", "yes"),
            ("msg-level", "all=info"),
        ]
    } else {
        &[]
    };
    let player: Arc<dyn Player> = Arc::new(MpvPlayer::new(options)?);

    let store = Rc::new(open_store(self_test));
    let window = AppWindow::new()?;
    let playback = playback::Playback::new(&window, player.clone());
    let browser = browser::Browser::new(&window, store.clone());
    for folder in folders {
        browser.add_local(folder);
    }
    let nav = nav::Nav::new(&window, player.clone(), playback.clone(), browser, store);

    let capture = video::ScreenCapture::default();
    // mpv needs the render context before a file is loaded, and the render
    // context needs Slint's GL context, which exists once the window is shown.
    // Files picked in the browser come later, so only the start file waits.
    {
        let playback = playback.clone();
        let start = play.map(|url| {
            window.set_page(nav::PAGE_PLAYER);
            let title = title_for(&url);
            (url, title)
        });
        let output = video::VideoOutput::new(player.clone(), move || {
            if let Some((url, title)) = start {
                playback.open(&url, &title, None);
            }
        });
        output.attach(&window, Some(capture.clone()))?;
    }
    let pip = pip::Pip::new(&window);
    {
        let pip = pip.clone();
        nav.set_on_leave_player(Box::new(move || pip.exit()));
    }

    let outcome = if self_test {
        selftest::start(&window, player.clone(), capture)
    } else {
        Default::default()
    };

    window.run()?;
    nav.save_position();
    Ok(outcome.exit_code())
}

/// Opens the database in the user's data folder. The self-test, or a
/// failure to open it, uses a throwaway in-memory database instead.
fn open_store(self_test: bool) -> Store {
    if !self_test {
        let opened = AppDirs::from_system()
            .ok_or_else(|| "no home folder".to_string())
            .and_then(|dirs| Store::open(&dirs.database_path()).map_err(|e| e.to_string()));
        match opened {
            Ok(store) => return store,
            Err(err) => {
                eprintln!("enthuse: cannot open the database, nothing will be saved: {err}")
            }
        }
    }
    Store::open_in_memory().expect("in-memory SQLite database")
}

/// File name for display, from a path or URL.
fn title_for(source: &str) -> String {
    source
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(source)
        .to_owned()
}
