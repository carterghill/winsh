//! The winsh shell engine, independent of its terminal frontend.

pub mod syntax;

mod arithmetic;
mod builtins;
mod exec;
mod expand;
mod glob;
mod language_builtins;
mod pipeline;
mod platform;
mod redirect;
mod shell;
mod streams;

pub use shell::{Outcome, Shell};
pub use streams::ShellIo;
