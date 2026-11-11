//! Everything that differs between Mac, Linux and Windows lives in this folder.
//! The rest of the program calls these functions and never checks the system.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

use std::path::PathBuf;

/// What `setup` needs to know to hook the account shortcuts into a shell.
#[derive(Debug, Clone)]
pub struct ShellHook {
    /// File that gets the line, e.g. `~/.zshrc`.
    pub file: PathBuf,
    /// Lines to add (exactly, once).
    pub lines: Vec<String>,
}

/// Find a program on PATH, the way the shell would.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in executable_extensions() {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
