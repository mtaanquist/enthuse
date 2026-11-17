//! Core logic for Enthuse: sources, watch state, natural sorting and
//! next-file selection.
//!
//! This crate has no UI or libmpv dependency so that all of its logic can be
//! unit tested on any platform.

pub mod natsort;
pub mod paths;
pub mod source;
pub mod storage;
pub mod time;
