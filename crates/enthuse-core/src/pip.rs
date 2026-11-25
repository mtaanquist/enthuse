//! Window geometry for PiP: moving and resizing from a pointer drag while
//! keeping the video's aspect ratio, and fitting a new video. Pure
//! functions on physical pixels, so they can be unit tested.
//! A new video gets the same window area in its own shape, or the size
//! the user chose for its orientation, anchored to the nearest screen
//! corner (#96).

/// A window rectangle in physical screen pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Which edges a resize moves. Matches the UI's numbering: 0 = north, then
/// clockwise to 7 = north-west.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Edges {
    pub fn from_direction(direction: i32) -> Self {
        let d = direction.rem_euclid(8);
        Self {
            top: matches!(d, 7 | 0 | 1),
            right: matches!(d, 1..=3),
            bottom: matches!(d, 3..=5),
            left: matches!(d, 5..=7),
        }
    }
}

/// The window after resizing `start` by dragging `edges` by (`dx`, `dy`).
/// With an `aspect` (width / height) the result keeps it; the edges
/// opposite the dragged ones stay put. Never smaller than `min_w` x `min_h`.
pub fn resize(
    start: Rect,
    edges: Edges,
    dx: i32,
    dy: i32,
    aspect: Option<f64>,
    min_w: i32,
    min_h: i32,
) -> Rect {
    let mut w = start.w as f64;
    let mut h = start.h as f64;
    if edges.right {
        w += dx as f64;
    }
    if edges.left {
        w -= dx as f64;
    }
    if edges.bottom {
        h += dy as f64;
    }
    if edges.top {
        h -= dy as f64;
    }
    // Dragged past the opposite edge: the minimum size below takes over.
    w = w.max(1.0);
    h = h.max(1.0);
    let horizontal = edges.left || edges.right;
    let vertical = edges.top || edges.bottom;

    if let Some(a) = aspect.filter(|a| a.is_finite() && *a > 0.0) {
        // Follow the dimension the user dragged further, relative to size.
        let by_width = match (horizontal, vertical) {
            (true, false) => true,
            (false, true) => false,
            _ => (w - start.w as f64).abs() >= (h - start.h as f64).abs() * a,
        };
        if by_width {
            h = w / a;
        } else {
            w = h * a;
        }
        // Grow to the minimum size along the ratio.
        let scale = (min_w as f64 / w).max(min_h as f64 / h).max(1.0);
        w *= scale;
        h *= scale;
    } else {
        w = w.max(min_w as f64);
        h = h.max(min_h as f64);
    }

    let (w, h) = (w.round() as i32, h.round() as i32);
    Rect {
        x: if edges.left {
            start.x + start.w - w
        } else {
            start.x
        },
        y: if edges.top {
            start.y + start.h - h
        } else {
            start.y
        },
        w,
        h,
    }
}

/// A video's orientation. PiP remembers a size for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
}

impl Orientation {
    /// Square and wider videos count as landscape.
    pub fn of(aspect: f64) -> Self {
        if aspect < 1.0 {
            Self::Portrait
        } else {
            Self::Landscape
        }
    }
}

/// How close two aspect ratios must be to count as the same, relative.
/// Covers rounding to whole pixels, so refitting a window to its own
/// ratio never changes it.
const SAME_ASPECT: f64 = 0.01;

/// The PiP size for a video with `aspect` (width / height): the area of
/// `like`, in the video's shape. If `like` already has about that shape,
/// it is kept as is. The result is at least `min` and at most `max`, both
/// along the ratio; `min` wins if they conflict.
pub fn fit_area(
    like: (i32, i32),
    aspect: f64,
    min: (i32, i32),
    max: Option<(i32, i32)>,
) -> (i32, i32) {
    let (lw, lh) = (like.0.max(1) as f64, like.1.max(1) as f64);
    if !(aspect.is_finite() && aspect > 0.0) {
        return like;
    }
    let (mut w, mut h) = if ((lw / lh) / aspect - 1.0).abs() <= SAME_ASPECT {
        (lw, lh)
    } else {
        let area = lw * lh;
        ((area * aspect).sqrt(), (area / aspect).sqrt())
    };
    if let Some((mw, mh)) = max {
        let scale = (mw.max(1) as f64 / w).min(mh.max(1) as f64 / h);
        if scale < 1.0 {
            w *= scale;
            h *= scale;
        }
    }
    let scale = (min.0 as f64 / w).max(min.1 as f64 / h);
    if scale > 1.0 {
        w *= scale;
        h *= scale;
    }
    (w.round() as i32, h.round() as i32)
}

/// How far the window may be off the size we set, in physical pixels,
/// before it counts as resized by the user (window systems may round).
const RESIZE_SLACK: i32 = 2;

/// Whether a PiP window now `current` in size, which we last set to
/// `expected` for a video with `aspect`, was resized by the user. A size
/// of the other orientation is not counted: that is a resize of ours the
/// window system has not reported yet.
pub fn user_resized(expected: (i32, i32), current: (i32, i32), aspect: f64) -> bool {
    let off = (current.0 - expected.0).abs() > RESIZE_SLACK
        || (current.1 - expected.1).abs() > RESIZE_SLACK;
    off && current.0 > 0
        && current.1 > 0
        && aspect.is_finite()
        && aspect > 0.0
        && Orientation::of(current.0 as f64 / current.1 as f64) == Orientation::of(aspect)
}

/// The window `current` resized to `size`. The corner nearest the
/// screen's matching corner stays put, so a PiP window in a screen corner
/// stays in it, and the window is kept on the screen where it fits.
pub fn anchor(current: Rect, size: (i32, i32), screen: Option<Rect>) -> Rect {
    let (w, h) = size;
    let (keep_right, keep_bottom) = match screen {
        Some(s) => (
            (s.x + s.w) - (current.x + current.w) < current.x - s.x,
            (s.y + s.h) - (current.y + current.h) < current.y - s.y,
        ),
        None => (false, false),
    };
    let mut x = if keep_right {
        current.x + current.w - w
    } else {
        current.x
    };
    let mut y = if keep_bottom {
        current.y + current.h - h
    } else {
        current.y
    };
    if let Some(s) = screen {
        // Right and bottom first, so a window larger than the screen
        // keeps its top-left corner on it.
        x = x.min(s.x + s.w - w).max(s.x);
        y = y.min(s.y + s.h - h).max(s.y);
    }
    Rect { x, y, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect = Rect {
        x: 100,
        y: 100,
        w: 480,
        h: 270,
    };
    const WIDE: Option<f64> = Some(16.0 / 9.0);

    #[test]
    fn directions_map_to_edges() {
        let e = Edges::from_direction(3);
        assert!(e.right && e.bottom && !e.left && !e.top);
        let e = Edges::from_direction(7);
        assert!(e.left && e.top && !e.right && !e.bottom);
        let e = Edges::from_direction(0);
        assert!(e.top && !e.left && !e.right && !e.bottom);
    }

    #[test]
    fn free_resize_without_aspect() {
        let r = resize(R, Edges::from_direction(2), 20, 50, None, 192, 108);
        assert_eq!(r, Rect { w: 500, ..R });
        // Dragging the left edge moves x.
        let r = resize(R, Edges::from_direction(6), 20, 0, None, 192, 108);
        assert_eq!(
            r,
            Rect {
                x: 120,
                w: 460,
                ..R
            }
        );
    }

    #[test]
    fn edge_resize_keeps_aspect() {
        let r = resize(R, Edges::from_direction(2), 160, 0, WIDE, 192, 108);
        assert_eq!((r.w, r.h), (640, 360));
        assert_eq!((r.x, r.y), (100, 100));
        let r = resize(R, Edges::from_direction(0), 0, -90, WIDE, 192, 108);
        assert_eq!((r.w, r.h), (640, 360));
        // Top edge: the bottom stays put.
        assert_eq!(r.y + r.h, R.y + R.h);
    }

    #[test]
    fn corner_resize_follows_larger_drag() {
        let r = resize(R, Edges::from_direction(3), 160, 10, WIDE, 192, 108);
        assert_eq!((r.w, r.h), (640, 360));
        let r = resize(R, Edges::from_direction(3), 0, 90, WIDE, 192, 108);
        assert_eq!((r.w, r.h), (640, 360));
        // North-west: the bottom-right corner stays put.
        let r = resize(R, Edges::from_direction(7), -160, 0, WIDE, 192, 108);
        assert_eq!((r.x + r.w, r.y + r.h), (R.x + R.w, R.y + R.h));
    }

    #[test]
    fn resize_respects_minimum() {
        let r = resize(R, Edges::from_direction(3), -1000, -1000, WIDE, 192, 108);
        assert_eq!((r.w, r.h), (192, 108));
        let r = resize(R, Edges::from_direction(3), -1000, -1000, None, 192, 108);
        assert_eq!((r.w, r.h), (192, 108));
        // A tall video is limited by the minimum width.
        let r = resize(
            R,
            Edges::from_direction(3),
            -1000,
            -1000,
            Some(0.5),
            192,
            108,
        );
        assert_eq!((r.w, r.h), (192, 384));
    }

    const MIN: (i32, i32) = (192, 108);

    #[test]
    fn orientation() {
        assert_eq!(Orientation::of(9.0 / 16.0), Orientation::Portrait);
        assert_eq!(Orientation::of(1.0), Orientation::Landscape);
        assert_eq!(Orientation::of(2.4), Orientation::Landscape);
    }

    #[test]
    fn fit_keeps_area() {
        // Landscape to portrait: same area, upright.
        let (w, h) = fit_area((480, 270), 9.0 / 16.0, MIN, None);
        assert_eq!((w, h), (270, 480));
        // 16:9 to 4:3.
        let (w, h) = fit_area((480, 270), 4.0 / 3.0, MIN, None);
        assert!(((w * h) as f64 / (480.0 * 270.0) - 1.0).abs() < 0.01);
        assert!(((w as f64 / h as f64) - 4.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn fit_round_trip_does_not_shrink() {
        // The reported bug: portrait, landscape, portrait again.
        let portrait = 9.0 / 16.0;
        let start = fit_area((560, 315), portrait, MIN, None);
        let mut size = start;
        for _ in 0..10 {
            size = fit_area(size, 16.0 / 9.0, MIN, None);
            size = fit_area(size, portrait, MIN, None);
        }
        assert!((size.0 - start.0).abs() <= 1 && (size.1 - start.1).abs() <= 1);
    }

    #[test]
    fn fit_same_shape_is_unchanged() {
        assert_eq!(fit_area((481, 271), 16.0 / 9.0, MIN, None), (481, 271));
        assert_eq!(fit_area((333, 592), 9.0 / 16.0, MIN, None), (333, 592));
    }

    #[test]
    fn fit_respects_min_and_max() {
        // A very wide video from a small window: grows to the min height.
        let (w, h) = fit_area((200, 112), 4.0, MIN, None);
        assert_eq!(h, 108);
        assert_eq!(w, 432);
        // A tall video from a large window: shrinks to fit the screen.
        let (w, h) = fit_area((1600, 900), 9.0 / 16.0, MIN, Some((1824, 984)));
        assert_eq!(h, 984);
        assert_eq!(w, 554);
        // Min wins over max.
        assert_eq!(
            fit_area((480, 270), 16.0 / 9.0, MIN, Some((100, 50))),
            (192, 108)
        );
    }

    #[test]
    fn fit_ignores_bad_aspect() {
        assert_eq!(fit_area((480, 270), 0.0, MIN, None), (480, 270));
        assert_eq!(fit_area((480, 270), f64::NAN, MIN, None), (480, 270));
    }

    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };

    #[test]
    fn anchor_keeps_nearest_screen_corner() {
        let corner = Rect {
            x: 1392,
            y: 762,
            w: 480,
            h: 270,
        };
        let r = anchor(corner, (270, 480), Some(SCREEN));
        assert_eq!((r.x + r.w, r.y + r.h), (1872, 1032));
        let r = anchor(R, (270, 480), Some(SCREEN));
        assert_eq!((r.x, r.y), (R.x, R.y));
        // Without a screen the top-left corner stays.
        let r = anchor(corner, (270, 480), None);
        assert_eq!((r.x, r.y), (corner.x, corner.y));
    }

    #[test]
    fn anchor_stays_on_screen() {
        // Top-left anchored near the bottom edge: pushed up to fit.
        let r = Rect {
            x: 100,
            y: 500,
            w: 480,
            h: 270,
        };
        let r2 = anchor(r, (270, 480), Some(SCREEN));
        assert!(r2.y + r2.h <= 1080 && r2.y >= 0);
        // Larger than the screen: top-left on it.
        let r3 = anchor(r, (2000, 1200), Some(SCREEN));
        assert_eq!((r3.x, r3.y), (0, 0));
    }

    #[test]
    fn anchor_on_offset_screen() {
        // A second monitor left of and above the primary.
        let screen = Rect {
            x: -1920,
            y: -200,
            w: 1920,
            h: 1080,
        };
        let corner = Rect {
            x: -528,
            y: 562,
            w: 480,
            h: 270,
        };
        let r = anchor(corner, (270, 480), Some(screen));
        assert_eq!((r.x + r.w, r.y + r.h), (-48, 832));
        let left = Rect {
            x: -1872,
            y: -152,
            ..corner
        };
        let r = anchor(left, (270, 480), Some(screen));
        assert_eq!((r.x, r.y), (-1872, -152));
    }

    #[test]
    fn fit_same_aspect_boundary() {
        // Within 1%: kept as is. Beyond: refitted to the exact ratio.
        assert_eq!(fit_area((484, 270), 16.0 / 9.0, MIN, None), (484, 270));
        let (w, h) = fit_area((500, 270), 16.0 / 9.0, MIN, None);
        assert!(((w as f64 / h as f64) - 16.0 / 9.0).abs() < 0.01);
    }

    #[test]
    fn fit_degenerate_inputs() {
        // A zero-size window still gets at least the minimum.
        assert_eq!(fit_area((0, 0), 16.0 / 9.0, MIN, None), (192, 108));
        // A zero max falls back to the minimum.
        assert_eq!(
            fit_area((480, 270), 16.0 / 9.0, MIN, Some((0, 0))),
            (192, 108)
        );
    }

    #[test]
    fn detects_user_resize() {
        let wide = 16.0 / 9.0;
        assert!(!user_resized((480, 270), (480, 270), wide));
        // Rounding by the window system is not a resize.
        assert!(!user_resized((480, 270), (481, 269), wide));
        assert!(user_resized((480, 270), (640, 360), wide));
        // Still the old portrait size: our resize has not landed yet.
        assert!(!user_resized((480, 270), (270, 480), wide));
        assert!(!user_resized((480, 270), (640, 360), 0.0));
    }
}
