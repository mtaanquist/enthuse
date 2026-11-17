//! Enthuse desktop app: Slint UI and wiring.
//!
//! Currently the render spike (#17): plays one file or URL with mpv drawn
//! underneath a Slint button. `--self-test` plays a built-in test pattern and
//! checks a snapshot of the window, for CI.

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

    // Drain player events on the UI thread whenever mpv signals some.
    {
        let weak_player = Arc::downgrade(&player);
        player.set_wakeup(Arc::new(move || {
            let weak_player = weak_player.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(p) = weak_player.upgrade() {
                    while let Some(_event) = p.poll_event() {}
                }
            });
        }));
    }

    let window = SpikeWindow::new()?;
    let capture = video::ScreenCapture::default();
    // mpv needs the render context before a file is loaded, and the render
    // context needs Slint's GL context, which exists once the window is shown.
    {
        let p = player.clone();
        let source = source.to_owned();
        video::attach(
            &window,
            player.clone(),
            move || {
                if let Err(err) = p.open(&source, None) {
                    eprintln!("enthuse: cannot open {source}: {err}");
                }
            },
            capture.clone(),
        )?;
    }

    {
        let player = player.clone();
        let weak = window.as_weak();
        window.on_toggle_pause(move || {
            let paused = !player.is_paused();
            let _ = player.set_paused(paused);
            if let Some(w) = weak.upgrade() {
                w.set_paused(paused);
            }
        });
    }

    let outcome = if self_test {
        selftest::start(&window, player.clone(), capture)
    } else {
        Default::default()
    };

    window.run()?;
    Ok(outcome.exit_code())
}
