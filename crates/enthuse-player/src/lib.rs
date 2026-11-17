//! Playback for Enthuse: the `Player` trait and its libmpv implementation.
//!
//! The trait is intentionally small so another backend (for example libVLC)
//! can be added later. Playlist and auto-next logic do not live here; see
//! `enthuse-core`.
