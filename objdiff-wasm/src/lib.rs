#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

mod api;
mod logging;

#[cfg(all(target_os = "wasi", not(feature = "std")))]
mod cabi_realloc;

#[cfg(all(target_family = "wasm", not(feature = "std")))]
#[global_allocator]
static ALLOCATOR: talc::TalckWasm = unsafe { talc::TalckWasm::new_global() };
