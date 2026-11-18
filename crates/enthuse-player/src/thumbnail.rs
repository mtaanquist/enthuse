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
    /// The file could not be read, maybe for now: a share that dropped, a
    /// locked file. Worth trying again later.
    Unreadable(String),
    /// The file was read but has no frame to show: empty, broken, or not
    /// a video mpv knows. Trying again won't help until it changes.
    NoFrame(String),
}

impl std::fmt::Display for GrabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut => f.write_str("timed out"),
            Self::Unreadable(why) | Self::NoFrame(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for GrabError {}

impl From<libmpv2::Error> for GrabError {
    fn from(err: libmpv2::Error) -> Self {
        use libmpv2_sys as sys;
        let why = crate::mpv::describe(&err);
        match err {
            libmpv2::Error::Raw(
                sys::mpv_error_MPV_ERROR_UNKNOWN_FORMAT | sys::mpv_error_MPV_ERROR_NOTHING_TO_PLAY,
            ) => Self::NoFrame(why),
            _ => Self::Unreadable(why),
        }
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
    let mut saved = false;
    for position in POSITIONS {
        match grab_at(url, out, width, position, deadline) {
            Ok(bytes) if bytes > BLACK_JPEG_BYTES => return Ok(()),
            // Black: try further in, keeping this one if that fails.
            Ok(_) => saved = true,
            Err(_) if saved => return Ok(()),
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// One grab at `position`. Returns the size of the written JPEG.
fn grab_at(url: &str, out: &Path, width: u32, position: &str, deadline: Instant) -> Result<u64> {
    let work = WorkDir::new(out)?;
    let outdir = work.0.to_string_lossy().into_owned();
    // Square pixels first, so anamorphic video (a 16:9 DVD) isn't
    // squashed, then down to `width`.
    let scale = format!("lavfi=[scale=trunc(ih*dar/2)*2:ih,setsar=1,scale={width}:-2]");
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
            None => GrabError::NoFrame("no frame".into()),
        })?;
    let bytes = fs::metadata(&frame).map(|m| m.len()).unwrap_or(0);
    fs::rename(&frame, out)
        .map_err(|e| GrabError::Unreadable(format!("cannot save thumbnail: {e}")))?;
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
            .map_err(|e| GrabError::Unreadable(format!("cannot create {}: {e}", dir.display())))?;
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
    fn anamorphic_video_gets_square_pixels() {
        let dir = std::env::temp_dir().join(format!("enthuse-thumb-sar-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.jpg");
        // 720x480 shown as 16:9.
        let url = "av://lavfi:testsrc=duration=10:size=720x480:rate=25,setsar=32/27";
        grab_frame(url, &out, 256, Duration::from_secs(30)).unwrap();
        let bytes = fs::read(&out).unwrap();
        assert_eq!(jpeg_size(&bytes), Some((256, 144)));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Width and height from a JPEG's start-of-frame marker.
    fn jpeg_size(b: &[u8]) -> Option<(u16, u16)> {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                return None;
            }
            let marker = b[i + 1];
            let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
            if (0xC0..=0xC2).contains(&marker) {
                let h = u16::from_be_bytes([b[i + 5], b[i + 6]]);
                let w = u16::from_be_bytes([b[i + 7], b[i + 8]]);
                return Some((w, h));
            }
            i += 2 + len;
        }
        None
    }

    #[test]
    fn a_missing_file_fails() {
        let dir =
            std::env::temp_dir().join(format!("enthuse-thumb-missing-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.jpg");
        let missing = grab_frame("/no/such/file.mkv", &out, 256, Duration::from_secs(10));
        assert!(
            matches!(missing, Err(GrabError::Unreadable(_))),
            "{missing:?}"
        );
        assert!(!out.exists());
        let empty = dir.join("empty.mkv");
        fs::write(&empty, b"").unwrap();
        let empty = grab_frame(&empty.to_string_lossy(), &out, 256, Duration::from_secs(10));
        assert!(matches!(empty, Err(GrabError::NoFrame(_))), "{empty:?}");
        assert!(!out.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
