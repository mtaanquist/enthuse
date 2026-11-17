//! Enthuse desktop app: Slint UI and wiring.
//!
//! Currently the render spike (#17): plays one file or URL with mpv drawn
//! underneath a Slint button. `--self-test` plays a built-in test pattern and
//! checks a snapshot of the window, for CI.

mod selftest;
mod video;

use std::process::ExitCode;
use std::sync::Arc;

use enthuse_player::libmpv2::Mpv;

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

    let mpv = Arc::new(Mpv::with_initializer(|init| {
        // Render through the render API into our GL context.
        init.set_option("vo", "libmpv")?;
        init.set_option("keep-open", "yes")?;
        if self_test {
            // CI runners have no audio device and only software GL.
            init.set_option("ao", "null")?;
            init.set_option("hwdec", "no")?;
            // Allow Mesa's software OpenGL (llvmpipe), which mpv rejects by default.
            init.set_option("gpu-sw", "yes")?;
            // mpv 0.37's full renderer on llvmpipe often draws only black
            // (#27); the basic path is reliable and still exercises the
            // render API and the compositing under Slint.
            init.set_option("gpu-dumb-mode", "yes")?;
            // Print mpv's log so CI failures can be diagnosed.
            init.set_option("terminal", "yes")?;
            init.set_option("msg-level", "all=info")?;
        } else {
            init.set_option("hwdec", "auto-safe")?;
        }
        Ok(())
    })?);

    let window = SpikeWindow::new()?;
    let capture = video::ScreenCapture::default();
    // mpv needs the render context before a file is loaded, and the render
    // context needs Slint's GL context, which exists once the window is shown.
    {
        let mpv = mpv.clone();
        let source = source.to_owned();
        video::attach(
            &window,
            mpv.clone(),
            move || {
                if let Err(err) = mpv.command("loadfile", &[&source]) {
                    eprintln!("enthuse: cannot open {source}: {err}");
                }
            },
            capture.clone(),
        )?;
    }

    {
        let mpv = mpv.clone();
        let weak = window.as_weak();
        window.on_toggle_pause(move || {
            let paused = !mpv.get_property::<bool>("pause").unwrap_or(false);
            let _ = mpv.set_property("pause", paused);
            if let Some(w) = weak.upgrade() {
                w.set_paused(paused);
            }
        });
    }

    let outcome = if self_test {
        selftest::start(&window, mpv.clone(), capture)
    } else {
        Default::default()
    };

    window.run()?;
    Ok(outcome.exit_code())
}
