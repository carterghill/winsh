//! The winsh shell engine, independent of its terminal frontend.

pub mod syntax;

mod arithmetic;
mod builtins;
mod completion;
mod exec;
mod expand;
mod glob;
mod jobs;
mod language_builtins;
mod pipeline;
mod platform;
mod redirect;
mod shell;
mod streams;
mod utilities;

pub use completion::CompletionContext;
pub use jobs::{interrupt_foreground, stop_foreground};
pub use shell::{Outcome, Shell};
pub use streams::ShellIo;
pub use utilities::Utility;
