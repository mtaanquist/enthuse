//! Enthuse desktop app: Slint UI and wiring.
//!
//! `enthuse [file-or-folder...] [url]`: opens files and folders like a drop
//! on the window (see [`external`]): a folder opens in the browser without
//! becoming a library, a file or URL starts playing right away.
//! `--self-test` plays a built-in test pattern and checks a snapshot of the
//! window, for CI.

mod bg;
mod browser;
mod external;
mod home;
mod libraries;
mod nav;
mod pip;
mod playback;
mod pointer;
mod recent;
mod selftest;
mod settings;
mod thumbs;
mod video;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
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
        println!("usage: enthuse [file-or-folder...] [url] | enthuse --self-test");
        return ExitCode::SUCCESS;
    }
    let self_test = args.iter().any(|a| a == "--self-test");
    // Files and folders on disk open like a drop; anything else is a URL.
    let (paths, urls): (Vec<&String>, Vec<&String>) = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .partition(|a| Path::new(a).exists());
    let start = if self_test {
        Start::Url(selftest::SOURCE.to_owned())
    } else if !paths.is_empty() {
        Start::Paths(paths.into_iter().map(PathBuf::from).collect())
    } else {
        urls.first()
            .map_or(Start::Nothing, |u| Start::Url(u.to_string()))
    };

    match run(start, self_test) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("enthuse: {err}");
            ExitCode::FAILURE
        }
    }
}

/// What to show first, from the command line.
enum Start {
    Nothing,
    /// Files and folders, opened as if dropped on the window.
    Paths(Vec<PathBuf>),
    /// Played as given, without saving any state.
    Url(String),
}

fn run(start: Start, self_test: bool) -> Result<ExitCode, Box<dyn std::error::Error>> {
    // The video underlay needs Slint's OpenGL renderer.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into())
        // Dark native title bar to match the UI (docs/ui.md, "Window
        // chrome"). Windows honors it; on Linux it is a hint to the
        // window manager or the client-side decorations.
        .with_winit_window_attributes_hook(|attributes| {
            attributes.with_theme(Some(slint::winit_030::winit::window::Theme::Dark))
        })
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
    {
        let weak = window.as_weak();
        window.global::<KeyFocus>().on_restore(move || {
            if let Some(w) = weak.upgrade() {
                w.invoke_focus_keys();
            }
        });
    }
    let playback = playback::Playback::new(&window, player.clone());
    let browser = browser::Browser::new(&window, store.clone());
    let home = home::Home::new(&window, store.clone(), browser.clone());
    let nav = nav::Nav::new(
        &window,
        player.clone(),
        playback.clone(),
        browser.clone(),
        store.clone(),
    );
    home.set_nav(&nav);
    let recent = recent::Recent::new(&window, store.clone(), browser.clone());
    recent.set_nav(&nav);
    let settings = settings::Settings::new(&window, store.clone(), browser.clone());
    let libraries = libraries::Libraries::new(&window, store.clone(), browser.clone());
    {
        let nav = nav.clone();
        settings.on_change(move || nav.reload_settings());
    }
    // No thumbnails in the self-test, which has no real files and must not
    // write to the user's cache.
    let thumb_dir = (!self_test)
        .then(AppDirs::from_system)
        .flatten()
        .map(|dirs| dirs.thumbnail_dir());
    let thumbs = thumbs::Thumbs::new(store.clone(), thumb_dir);
    browser.set_thumbs(&thumbs);
    home.set_thumbs(&thumbs);
    nav.set_thumbs(&thumbs);
    recent.set_thumbs(&thumbs);
    {
        let thumbs = thumbs.clone();
        settings.on_change(move || thumbs.settings_changed());
    }

    let capture = video::ScreenCapture::default();
    // mpv needs the render context before a file is loaded, and the render
    // context needs Slint's GL context, which exists once the window is shown.
    // Files picked in the browser come later, so only the start waits.
    let opener: Rc<RefCell<Option<Rc<external::Opener>>>> = Default::default();
    {
        let playback = playback.clone();
        let plays_file = match &start {
            Start::Nothing => false,
            Start::Paths(paths) => !paths.iter().any(|p| p.is_dir()),
            Start::Url(_) => true,
        };
        if plays_file {
            window.set_page(nav::PAGE_PLAYER);
        }
        let opener = opener.clone();
        let output = video::VideoOutput::new(player.clone(), move || match start {
            Start::Nothing => {}
            Start::Paths(paths) => {
                if let Some(opener) = opener.borrow().as_ref() {
                    opener.open(&paths);
                }
            }
            Start::Url(url) => {
                // An error shows on the player's overlay.
                let _ = playback.open(&url, &title_for(&url), None);
            }
        });
        output.attach(&window, Some(capture.clone()))?;
    }
    let pip = pip::Pip::new(&window, browser.clone(), player.clone(), store.clone())?;
    *opener.borrow_mut() = Some(external::Opener::new(
        &window,
        browser.clone(),
        nav.clone(),
        pip.clone(),
    ));
    playback.attach_side(pip.side());
    home.attach_side(pip.side());
    settings.attach_side(pip.side());
    libraries.attach_side(pip.side());
    recent.attach_side(pip.side());
    {
        let (pip, playback) = (pip.clone(), playback.clone());
        nav.set_on_leave_player(Box::new(move || {
            pip.exit();
            playback.leave_fullscreen();
        }));
    }

    let outcome = if self_test {
        playback.disable_auto_hide();
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
