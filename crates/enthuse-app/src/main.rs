//! Enthuse desktop app: Slint UI and wiring.

fn main() {
    match enthuse_core::paths::AppDirs::from_system() {
        Some(dirs) => println!("Enthuse: data in {}", dirs.data_dir().display()),
        None => eprintln!("Enthuse: could not determine a home directory"),
    }
}
