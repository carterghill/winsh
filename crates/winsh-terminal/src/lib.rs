//! Terminal state and Windows pseudoconsole hosting. No shell language lives here.

pub mod input;
pub mod terminal;

#[cfg(windows)]
#[allow(unsafe_code)]
pub mod session;

#[cfg(windows)]
#[allow(unsafe_code)]
pub mod window;
