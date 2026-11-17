//! Window geometry for PiP: moving and resizing from a pointer drag while
//! keeping the video's aspect ratio, and fitting a new video. Pure
//! functions on physical pixels, so they can be unit tested.

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

/// The window resized to a new video's `aspect`, never taller than now:
/// the width stays unless that would grow the height, then the height
/// stays. The corner nearest the screen's matching corner stays put, so a
/// PiP window in a screen corner stays in it.
pub fn fit_aspect(current: Rect, aspect: f64, screen: Option<Rect>) -> Rect {
    if !(aspect.is_finite() && aspect > 0.0) {
        return current;
    }
    let mut w = current.w as f64;
    let mut h = w / aspect;
    if h > current.h as f64 {
        h = current.h as f64;
        w = h * aspect;
    }
    let (w, h) = (w.round() as i32, h.round() as i32);
    let (keep_right, keep_bottom) = match screen {
        Some(s) => (
            (s.x + s.w) - (current.x + current.w) < current.x - s.x,
            (s.y + s.h) - (current.y + current.h) < current.y - s.y,
        ),
        None => (false, false),
    };
    Rect {
        x: if keep_right {
            current.x + current.w - w
        } else {
            current.x
        },
        y: if keep_bottom {
            current.y + current.h - h
        } else {
            current.y
        },
        w,
        h,
    }
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

    #[test]
    fn fit_never_grows_taller() {
        // Wider video: same width, less tall.
        let r = fit_aspect(R, 2.4, None);
        assert_eq!((r.w, r.h), (480, 200));
        // Narrower video (4:3): same height, less wide.
        let r = fit_aspect(R, 4.0 / 3.0, None);
        assert_eq!((r.w, r.h), (360, 270));
        // Same ratio: unchanged.
        assert_eq!(fit_aspect(R, 16.0 / 9.0, None), R);
    }

    #[test]
    fn fit_keeps_nearest_screen_corner() {
        let screen = Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let corner = Rect {
            x: 1416,
            y: 786,
            w: 480,
            h: 270,
        };
        let r = fit_aspect(corner, 4.0 / 3.0, Some(screen));
        assert_eq!((r.x + r.w, r.y + r.h), (1896, 1056));
        let r = fit_aspect(R, 4.0 / 3.0, Some(screen));
        assert_eq!((r.x, r.y), (R.x, R.y));
    }
}
