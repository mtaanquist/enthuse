//! Headless tests of MpvPlayer against FFmpeg-generated media (no files,
//! no window, no audio device).

use std::time::{Duration, Instant};

use enthuse_player::{EndReason, MpvPlayer, Player, PlayerEvent, TrackKind};

fn headless() -> MpvPlayer {
    MpvPlayer::new(&[("vo", "null"), ("ao", "null"), ("hwdec", "no")]).expect("mpv starts")
}

/// Polls events until `pred` matches one, or panics after `timeout`.
fn wait_for(
    player: &MpvPlayer,
    timeout: Duration,
    mut pred: impl FnMut(&PlayerEvent) -> bool,
) -> Vec<PlayerEvent> {
    let deadline = Instant::now() + timeout;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        while let Some(e) = player.poll_event() {
            let done = pred(&e);
            seen.push(e);
            if done {
                return seen;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out; events seen: {seen:?}");
}

const TWO_SECONDS: &str = "av://lavfi:testsrc=duration=2:size=320x240:rate=25";

#[test]
fn plays_to_end_of_file() {
    let player = headless();
    player.open(TWO_SECONDS, None).unwrap();
    let events = wait_for(&player, Duration::from_secs(20), |e| {
        matches!(e, PlayerEvent::Ended(_))
    });
    assert!(events.contains(&PlayerEvent::FileLoaded), "{events:?}");
    assert!(
        events.contains(&PlayerEvent::Ended(EndReason::EndOfFile)),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, PlayerEvent::PositionChanged(Some(p)) if *p > 0.0)),
        "{events:?}"
    );
}

#[test]
fn stop_is_not_end_of_file() {
    let player = headless();
    player.open(TWO_SECONDS, None).unwrap();
    wait_for(&player, Duration::from_secs(20), |e| {
        *e == PlayerEvent::FileLoaded
    });
    player.stop().unwrap();
    let events = wait_for(&player, Duration::from_secs(20), |e| {
        matches!(e, PlayerEvent::Ended(_))
    });
    assert_eq!(events.last(), Some(&PlayerEvent::Ended(EndReason::Stopped)));
}

#[test]
fn opens_at_start_position_and_seeks() {
    let player = headless();
    player
        .open(
            "av://lavfi:testsrc=duration=30:size=320x240:rate=25",
            Some(10.0),
        )
        .unwrap();
    // Right after FileLoaded, time-pos can still report the pre-seek
    // position, so wait for the first reported position instead.
    let events = wait_for(
        &player,
        Duration::from_secs(20),
        |e| matches!(e, PlayerEvent::PositionChanged(Some(p)) if *p > 1.0),
    );
    player.set_paused(true).unwrap();
    let Some(PlayerEvent::PositionChanged(Some(pos))) = events.last() else {
        unreachable!()
    };
    assert!(*pos >= 9.0, "started at {pos}; events: {events:?}");

    player.seek(20.0).unwrap();
    wait_for(
        &player,
        Duration::from_secs(20),
        |e| matches!(e, PlayerEvent::PositionChanged(Some(p)) if *p >= 19.0),
    );
}

#[test]
fn lists_and_selects_tracks() {
    let player = headless();
    player.open(TWO_SECONDS, None).unwrap();
    wait_for(&player, Duration::from_secs(20), |e| {
        *e == PlayerEvent::FileLoaded
    });
    player.set_paused(true).unwrap();
    let tracks = player.tracks();
    let video = tracks
        .iter()
        .find(|t| t.kind == TrackKind::Video)
        .expect("video track");
    assert!(video.selected);
    assert!(!tracks.iter().any(|t| t.kind == TrackKind::Subtitle));

    // Turning subtitles off when there are none is not an error.
    player.select_track(TrackKind::Subtitle, None).unwrap();
}

#[test]
fn bad_url_reports_error() {
    let player = headless();
    player.open("/definitely/not/here.mkv", None).unwrap();
    let events = wait_for(&player, Duration::from_secs(20), |e| {
        matches!(e, PlayerEvent::Ended(_))
    });
    assert!(
        matches!(events.last(), Some(PlayerEvent::Ended(EndReason::Error(_)))),
        "{events:?}"
    );
}

#[test]
fn volume_is_clamped() {
    let player = headless();
    player.set_volume(150.0).unwrap();
    assert_eq!(player.volume(), 100.0);
    player.set_volume(40.0).unwrap();
    assert_eq!(player.volume(), 40.0);
}
