//! cc-sessions: see, close and resume Claude Code sessions across every account
//! on one machine, from a local web page or the command line.
//!
//! Safety rule for the whole program: nothing here deletes or moves anything
//! under an account's config folder. Removing an account sets a flag in the
//! registry, and closing a session only stops its process.

mod accounts;
mod claude;
mod cli;
mod closing;
mod editors;
mod history;
mod os;
mod pins;
mod proc;
mod report;
mod resume;
mod server;
mod sessions;
mod setup;
mod shell;
mod store;
mod titles;
mod update;
mod util;

fn main() {
    std::process::exit(cli::main());
}
