//! Frame grabs for thumbnails, with a separate headless libmpv instance
//! per grab, so the player is never touched.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use libmpv2::Mpv;
use libmpv2::events::Event;

/// Why no frame was grabbed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrabError {
    /// It took too long, e.g. a slow share. Worth trying again later.
    TimedOut,
    /// The file could not be opened or decoded.
    Failed(String),
}

impl std::fmt::Display for GrabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut => f.write_str("timed out"),
            Self::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for GrabError {}

impl From<libmpv2::Error> for GrabError {
    fn from(err: libmpv2::Error) -> Self {
        Self::Failed(crate::mpv::describe(&err))
    }
}

type Result<T> = std::result::Result<T, GrabError>;

/// Where in the file to grab, then where to try again when that frame is
/// (nearly) black, e.g. a fade between scenes.
const POSITIONS: &[&str] = &["10%", "30%"];
/// A JPEG of a flat black frame at thumbnail size is a few hundred bytes;
/// anything this small is treated as black.
const BLACK_JPEG_BYTES: u64 = 1_000;

/// Grabs one frame about 10% into `url` and writes it as a JPEG `width`
/// pixels wide to `out`. Blocks until done, failing after `timeout` (a
/// slow share) or when the file cannot be decoded. Run it off the UI
/// thread.
pub fn grab_frame(url: &str, out: &Path, width: u32, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last = Err(GrabError::Failed("no frame".into()));
    for position in POSITIONS {
        last = grab_at(url, out, width, position, deadline);
        match &last {
            Ok(bytes) if *bytes > BLACK_JPEG_BYTES => return Ok(()),
            Ok(_) => {} // Black: try further in, keeping this one if not.
            Err(_) => break,
        }
    }
    last.map(|_| ())
}

/// One grab at `position`. Returns the size of the written JPEG.
fn grab_at(url: &str, out: &Path, width: u32, position: &str, deadline: Instant) -> Result<u64> {
    let work = WorkDir::new(out)?;
    let outdir = work.0.to_string_lossy().into_owned();
    let scale = format!("scale=w={width}:h=-2");
    let options: [(&str, &str); 17] = [
        ("config", "no"),
        ("terminal", "no"),
        ("load-scripts", "no"),
        ("ytdl", "no"),
        ("input-default-bindings", "no"),
        ("idle", "no"),
        ("vo", "image"),
        ("vo-image-format", "jpg"),
        ("vo-image-jpeg-quality", "82"),
        ("vo-image-outdir", &outdir),
        ("vf", &scale),
        ("ao", "null"),
        ("aid", "no"),
        ("sid", "no"),
        ("hwdec", "no"),
        ("frames", "1"),
        ("start", position),
    ];
    let mpv = Mpv::with_initializer(|init| {
        for (k, v) in options {
            init.set_option(k, v)?;
        }
        Ok(())
    })?;
    mpv.command("loadfile", &[url])?;

    let mut error = None;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(GrabError::TimedOut);
        }
        match mpv.wait_event(left.as_secs_f64().min(1.0)) {
            Some(Ok(Event::EndFile(_) | Event::Shutdown)) => break,
            Some(Err(err)) => {
                error = Some(err);
                break;
            }
            _ => {}
        }
    }
    drop(mpv);

    let frame = fs::read_dir(&work.0)
        .ok()
        .and_then(|dir| {
            dir.filter_map(|e| e.ok())
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|x| x == "jpg"))
        })
        .ok_or_else(|| match error {
            Some(err) => GrabError::from(err),
            None => GrabError::Failed("no frame".into()),
        })?;
    let bytes = fs::metadata(&frame).map(|m| m.len()).unwrap_or(0);
    fs::rename(&frame, out)
        .map_err(|e| GrabError::Failed(format!("cannot save thumbnail: {e}")))?;
    Ok(bytes)
}

/// A scratch folder next to `out` for mpv to write into, removed on drop.
struct WorkDir(PathBuf);

impl WorkDir {
    fn new(out: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = out.parent().unwrap_or(Path::new("."));
        let dir = parent.join(format!(
            ".grab-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir)
            .map_err(|e| GrabError::Failed(format!("cannot create {}: {e}", dir.display())))?;
        Ok(Self(dir))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grabs from mpv's built-in test source, so no media file is needed.
    #[test]
    fn grabs_a_frame_from_a_generated_video() {
        let dir = std::env::temp_dir().join(format!("enthuse-thumb-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.jpg");
        let url = "av://lavfi:testsrc=duration=10:size=640x360:rate=25";
        grab_frame(url, &out, 256, Duration::from_secs(30)).unwrap();
        let bytes = fs::read(&out).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "a JPEG");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_fails() {
        let dir =
            std::env::temp_dir().join(format!("enthuse-thumb-missing-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.jpg");
        assert!(grab_frame("/no/such/file.mkv", &out, 256, Duration::from_secs(10)).is_err());
        assert!(!out.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
