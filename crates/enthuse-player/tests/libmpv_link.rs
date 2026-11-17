//! Proves that libmpv is found at build and run time on every CI platform.

#[test]
fn create_and_destroy_mpv_handle() {
    let mpv = libmpv2::Mpv::with_initializer(|init| {
        // No window, no audio device: this must work on headless CI runners.
        init.set_option("vo", "null")?;
        init.set_option("ao", "null")?;
        Ok(())
    })
    .expect("libmpv initializes");

    let version: String = mpv.get_property("mpv-version").expect("mpv-version");
    assert!(version.starts_with("mpv "), "unexpected version: {version}");
    println!("linked against {version}");
}
