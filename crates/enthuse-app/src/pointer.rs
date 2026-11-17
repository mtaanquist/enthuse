//! The mouse pointer's position on screen, for moving and resizing the PiP
//! window ourselves. Window-relative pointer events are not enough: they
//! shift while the window moves, which makes a drag jitter.

/// Where the pointer is, in physical screen pixels, and whether the left
/// button is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pointer {
    pub x: i32,
    pub y: i32,
    pub left_down: bool,
}

/// Reads the pointer, or `None` where that is not possible (Wayland).
pub fn read() -> Option<Pointer> {
    imp::read()
}

#[cfg(target_os = "windows")]
mod imp {
    use super::Pointer;
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    pub fn read() -> Option<Pointer> {
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: plain Win32 calls with a valid out pointer.
        let (ok, keys) = unsafe { (GetCursorPos(&mut p), GetAsyncKeyState(VK_LBUTTON as i32)) };
        (ok != 0).then_some(Pointer {
            x: p.x,
            y: p.y,
            // The high bit is set while the button is down.
            left_down: (keys as u16) & 0x8000 != 0,
        })
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::cell::OnceCell;

    use super::Pointer;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, KeyButMask};
    use x11rb::rust_connection::RustConnection;

    thread_local! {
        /// Our own X connection, opened on first use. `None` without X11.
        static X: OnceCell<Option<(RustConnection, u32)>> = const { OnceCell::new() };
    }

    pub fn read() -> Option<Pointer> {
        X.with(|cell| {
            let (conn, root) = cell
                .get_or_init(|| {
                    let (conn, screen) = x11rb::connect(None).ok()?;
                    let root = conn.setup().roots.get(screen)?.root;
                    Some((conn, root))
                })
                .as_ref()?;
            let reply = conn.query_pointer(*root).ok()?.reply().ok()?;
            Some(Pointer {
                x: reply.root_x.into(),
                y: reply.root_y.into(),
                left_down: reply.mask.contains(KeyButMask::BUTTON1),
            })
        })
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    pub fn read() -> Option<super::Pointer> {
        None
    }
}
