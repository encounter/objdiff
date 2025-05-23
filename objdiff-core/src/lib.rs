#![allow(clippy::too_many_arguments)]
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

#[cfg(feature = "any-arch")]
pub mod arch;
#[cfg(feature = "bindings")]
pub mod bindings;
#[cfg(feature = "build")]
pub mod build;
#[cfg(feature = "config")]
pub mod config;
#[cfg(feature = "any-arch")]
pub mod diff;
#[cfg(feature = "build")]
pub mod jobs;
#[cfg(feature = "any-arch")]
pub mod obj;
#[cfg(feature = "any-arch")]
pub mod util;
