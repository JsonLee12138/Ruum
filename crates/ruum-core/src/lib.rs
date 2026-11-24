//! `ruum-core`: scanning, layout, space accounting and deletion, with no
//! UI dependency.
//!
//! The split exists so the parts that must be *correct* — size accounting,
//! hardlink de-duplication, aspect-ratio layout, and what may be deleted — can
//! be built and tested without GPUI, a display, or a GPU.

pub mod access;
#[cfg(feature = "service")]
pub mod ai_cli;
#[cfg(feature = "service")]
pub mod auth;
#[cfg(feature = "service")]
pub mod autostart;
pub mod classify;
#[cfg(feature = "service")]
pub mod cli_install;
#[cfg(feature = "service")]
pub mod confirm;
#[cfg(feature = "service")]
pub mod credentials;
pub mod deployment;
#[cfg(feature = "service")]
pub mod desktop_launch;
pub mod docker_adapter;
pub mod export;
pub mod filter;
pub mod git;
pub mod i18n;
pub mod identity;
pub mod insights;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(feature = "service")]
pub mod mcp;
#[cfg(windows)]
mod mft;
#[cfg(feature = "service")]
pub mod notify;
pub mod payload;
pub mod remote;
pub mod remote_progress;
#[cfg(unix)]
mod remote_walk;
pub mod removal;
pub mod scan;
pub mod scan_preview;
pub mod scan_threads;
pub mod size;
pub mod space;
#[cfg(feature = "service")]
pub mod ssh_agent;
#[cfg(feature = "service")]
pub mod ssh_session;
#[cfg(feature = "service")]
pub mod storage;
#[cfg(feature = "service")]
pub mod terminal_launcher;
pub mod trash_adapter;
pub mod tree;
pub mod treemap;
#[cfg(feature = "service")]
pub mod updater;
#[cfg(feature = "service")]
pub mod watchdog;
#[cfg(feature = "service")]
pub mod watchdog_config;
#[cfg(feature = "service")]
pub mod watchdog_service;
#[cfg(feature = "service")]
pub mod webhook;
#[cfg(windows)]
mod windows;
