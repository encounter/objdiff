pub mod arch;
#[cfg(feature = "config")]
pub mod config;
pub mod diff;
pub mod obj;
pub mod util;

#[cfg(not(feature = "any-arch"))]
compile_error!("At least one architecture feature must be enabled.");
