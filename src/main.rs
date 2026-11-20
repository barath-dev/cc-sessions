//! Paddock: see, close and resume Claude Code and Codex sessions across every
//! account on one machine, from a local web page or the command line. It was
//! called cc-sessions before 2.8 (see migrate.rs).
//!
//! Safety rule for the whole program: nothing here deletes or moves anything
//! under an account's config folder. Removing an account sets a flag in the
//! registry, and closing a session only stops its process.

mod accounts;
mod claude;
mod cli;
mod closing;
mod codex;
mod editors;
mod history;
mod loops;
mod migrate;
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
mod tools;
mod update;
mod util;

fn main() {
    std::process::exit(cli::main());
}
