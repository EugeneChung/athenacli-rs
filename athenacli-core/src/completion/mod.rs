//! Context-aware SQL autocompletion: `suggest_type` dispatch (engine),
//! metadata cache, the reedline `Completer`, and the background refresher.

pub mod completer;
pub mod engine;
pub mod metadata;
pub mod refresher;
