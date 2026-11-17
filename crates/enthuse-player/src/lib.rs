//! Playback for Enthuse: the `Player` trait and its libmpv implementation.
//!
//! The trait is intentionally small so another backend (for example libVLC)
//! can be added later. Playlist and auto-next logic do not live here; see
//! `enthuse-core`.

pub mod gl;

/// Re-exported for the render spike (#17). The `Player` trait (#5) will
/// replace direct use of libmpv in the app.
pub use libmpv2;
