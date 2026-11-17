//! Core logic for Enthuse: sources, watch state, natural sorting and
//! next-file selection.
//!
//! This crate has no UI or libmpv dependency so that all of its logic can be
//! unit tested on any platform.

pub mod folder;
pub mod natsort;
pub mod next;
pub mod paths;
pub mod resume;
pub mod settings;
pub mod source;
pub mod storage;
pub mod time;
