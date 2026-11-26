//! The winsh shell engine, independent of its terminal frontend.

pub mod syntax;

mod aliases;
mod arithmetic;
mod builtins;
mod completion;
mod controls;
mod exec;
mod expand;
mod glob;
mod jobs;
mod language_builtins;
mod pipeline;
mod platform;
mod prompt;
mod redirect;
mod shell;
mod streams;
mod traps;
mod utilities;

pub use completion::CompletionContext;
pub use jobs::{interrupt_foreground, stop_foreground};
pub use prompt::Prompt;
pub use shell::{Outcome, Shell};
pub use streams::ShellIo;
pub use traps::notify_signal;
pub use utilities::Utility;
