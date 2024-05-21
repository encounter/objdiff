use cfg_if::cfg_if;
use const_format::formatcp;
use self_update::{cargo_crate_version, update::ReleaseUpdate};

pub const OS: &str = std::env::consts::OS;
cfg_if! {
    if #[cfg(target_arch = "aarch64")] {
        cfg_if! {
            if #[cfg(any(windows, target_os = "macos"))] {
                pub const ARCH: &str = "arm64";
            } else {
                pub const ARCH: &str = std::env::consts::ARCH;
            }
        }
    } else if #[cfg(target_arch = "arm")] {
        pub const ARCH: &str = "armv7l";
    } else {
        pub const ARCH: &str = std::env::consts::ARCH;
    }
}
pub const GITHUB_USER: &str = "encounter";
pub const GITHUB_REPO: &str = "objdiff";
pub const BIN_NAME_NEW: &str =
    formatcp!("objdiff-gui-{}-{}{}", OS, ARCH, std::env::consts::EXE_SUFFIX);
pub const BIN_NAME_OLD: &str = formatcp!("objdiff-{}-{}{}", OS, ARCH, std::env::consts::EXE_SUFFIX);
pub const RELEASE_URL: &str =
    formatcp!("https://github.com/{}/{}/releases/latest", GITHUB_USER, GITHUB_REPO);

pub fn build_updater() -> self_update::errors::Result<Box<dyn ReleaseUpdate>> {
    self_update::backends::github::Update::configure()
        .repo_owner(GITHUB_USER)
        .repo_name(GITHUB_REPO)
        // bin_name is required, but unused?
        .bin_name(BIN_NAME_NEW)
        .no_confirm(true)
        .show_output(false)
        .current_version(cargo_crate_version!())
        .build()
}
