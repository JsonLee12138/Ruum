//! `ruum-core`: scanning, layout, space accounting and deletion, with no
//! UI dependency.
//!
//! The split exists so the parts that must be *correct* — size accounting,
//! hardlink de-duplication, aspect-ratio layout, and what may be deleted — can
//! be built and tested without GPUI, a display, or a GPU.

pub mod access;
pub mod auth;
pub mod classify;
pub mod deployment;
pub mod export;
pub mod filter;
pub mod git;
pub mod identity;
pub mod insights;
#[cfg(target_os = "macos")]
mod macos;
pub mod mcp;
#[cfg(windows)]
mod mft;
pub mod payload;
pub mod remote;
pub mod remote_progress;
pub mod remote_trash;
#[cfg(unix)]
mod remote_walk;
pub mod removal;
pub mod scan;
pub mod scan_preview;
pub mod scan_threads;
pub mod size;
pub mod space;
pub mod tree;
pub mod treemap;
#[cfg(windows)]
mod windows;
