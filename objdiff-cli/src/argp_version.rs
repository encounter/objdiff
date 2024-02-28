// Originally from https://gist.github.com/suluke/e0c672492126be0a4f3b4f0e1115d77c
//! Extend `argp` to be better integrated with the `cargo` ecosystem
//!
//! For now, this only adds a --version/-V option which causes early-exit.
use std::ffi::OsStr;

use argp::{parser::ParseGlobalOptions, EarlyExit, FromArgs, TopLevelCommand};

struct ArgsOrVersion<T>(T)
where T: FromArgs;

impl<T> TopLevelCommand for ArgsOrVersion<T> where T: FromArgs {}

impl<T> FromArgs for ArgsOrVersion<T>
where T: FromArgs
{
    fn _from_args(
        command_name: &[&str],
        args: &[&OsStr],
        parent: Option<&mut dyn ParseGlobalOptions>,
    ) -> Result<Self, EarlyExit> {
        /// Also use argp for catching `--version`-only invocations
        #[derive(FromArgs)]
        struct Version {
            /// Print version information and exit.
            #[argp(switch, short = 'V')]
            pub version: bool,
        }

        match Version::from_args(command_name, args) {
            Ok(v) => {
                if v.version {
                    println!(
                        "{} {} {}",
                        command_name.first().unwrap_or(&""),
                        env!("CARGO_PKG_VERSION"),
                        env!("GIT_COMMIT_SHA"),
                    );
                    std::process::exit(0);
                } else {
                    // Pass through empty arguments
                    T::_from_args(command_name, args, parent).map(Self)
                }
            }
            Err(exit) => match exit {
                EarlyExit::Help(_help) => {
                    // TODO: Chain help info from Version
                    // For now, we just put the switch on T as well
                    T::from_args(command_name, &["--help"]).map(Self)
                }
                EarlyExit::Err(_) => T::_from_args(command_name, args, parent).map(Self),
            },
        }
    }
}

/// Create a `FromArgs` type from the current process’s `env::args`.
///
/// This function will exit early from the current process if argument parsing was unsuccessful or if information like `--help` was requested.
/// Error messages will be printed to stderr, and `--help` output to stdout.
pub fn from_env<T>() -> T
where T: TopLevelCommand {
    argp::parse_args_or_exit::<ArgsOrVersion<T>>(argp::DEFAULT).0
}
