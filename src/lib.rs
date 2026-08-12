//! The modules left public are the ones an embedder -- today, the integration tests -- drives the
//! linter through: load a [`config::Config`], inspect with [`engine`], read [`diagnostic`] types
//! back. Everything else is an implementation detail, kept private so that the cop registry and
//! the output layer stay free to change.

mod cli;
pub mod config;
pub mod cop_name;
pub mod diagnostic;
mod directives;
pub mod engine;
mod formatter;
mod magic_comment;
mod nul_bytes;
mod ruby_version;
pub mod rules;
pub mod source;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Full RuboCop release Sonicop mirrors; the JSON metadata reports this verbatim.
pub const RUBOCOP_COMPAT_FULL_VERSION: &str = "1.89.0";
/// `MAJOR.MINOR` form used where RuboCop itself omits the patch level (docs URLs, `-V`). Spelled
/// out rather than sliced from the full version because `&str` slicing is not const at this
/// crate's MSRV; the test below is what keeps the two from drifting apart.
pub const RUBOCOP_COMPAT_VERSION: &str = "1.89";

pub use cli::run;
/// Re-exported because [`config::Config::target_ruby_version`] hands it out.
pub use ruby_version::RubyVersion;

#[cfg(test)]
mod tests {
    use super::{RUBOCOP_COMPAT_FULL_VERSION, RUBOCOP_COMPAT_VERSION};

    #[test]
    fn both_compat_versions_name_the_same_release() {
        assert_eq!(
            RUBOCOP_COMPAT_FULL_VERSION
                .rsplit_once('.')
                .map(|(short, _)| short),
            Some(RUBOCOP_COMPAT_VERSION)
        );
    }
}
