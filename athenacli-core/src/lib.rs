//! athenacli-core: all reusable logic for the Athena CLI (config, auth,
//! query lifecycle, sync execution wrapper, output rendering). Kept separate
//! from the `athenacli` binary so it can be unit-tested without a TTY.

pub mod athena;
pub mod auth;
pub mod cancel;
pub mod completion;
pub mod config;
pub mod exec;
pub mod output;
pub mod parse;
pub mod prompt;
pub mod special;
pub mod style;
