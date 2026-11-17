//! Enthuse desktop app: Slint UI and wiring.
//!
//! Plays one file or URL with the playback screen drawn over the video.
//! `--self-test` plays a built-in test pattern and checks a snapshot of the
//! window, for CI.

mod playback;
mod selftest;
mod video;

use std::process::ExitCode;
use std::sync::Arc;

use enthuse_player::{MpvPlayer, Player};

slint::include_modules!();

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let self_test = args.iter().any(|a| a == "--self-test");
    let source = match (self_test, args.iter().find(|a| !a.starts_with("--"))) {
        (true, _) => selftest::SOURCE.to_owned(),
        (false, Some(path)) => path.clone(),
        (false, None) => {
            eprintln!("usage: enthuse <file-or-url> | enthuse --self-test");
            return ExitCode::from(2);
        }
    };

    match run(&source, self_test) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("enthuse: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(source: &str, self_test: bool) -> Result<ExitCode, Box<dyn std::error::Error>> {
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
    let capture = video::ScreenCapture::default();
    // mpv needs the render context before a file is loaded, and the render
    // context needs Slint's GL context, which exists once the window is shown.
    {
        let playback = playback.clone();
        let source = source.to_owned();
        let title = title_for(&source);
        video::attach(
            &window,
            player.clone(),
            move || playback.open(&source, &title, None),
            capture.clone(),
        )?;
    }
    // Without a browser yet, "back" closes the app.
    window.on_back(|| {
        let _ = slint::quit_event_loop();
    });

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
