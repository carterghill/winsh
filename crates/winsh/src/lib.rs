//! The winsh shell engine, independent of its terminal frontend.

pub mod syntax;

mod builtins;
mod exec;
mod expand;
mod pipeline;
mod platform;
mod redirect;
mod shell;
mod streams;

pub use shell::{Outcome, Shell};
pub use streams::ShellIo;
