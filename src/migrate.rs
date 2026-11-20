//! Moving an install from before the rename (cc-sessions, up to 2.7) over to
//! Paddock, the first time the new version runs. Each step looks first and does
//! nothing once done, and none of them stops the program when it cannot be done:
//! the old names keep working.
//!
//! - The settings folder moves to its new name. On macOS and Linux a link is left
//!   at the old path, so the line a shell's startup file loads the shortcuts with
//!   keeps working; until the move succeeds the old folder is used (store.rs).
//! - The installer's copy gains the `paddock` command beside `cc-sessions`, and
//!   the app shortcut its new name. The program itself stays where it was.
//! - fish's shortcut file is written under its new name.

use std::fs;

use crate::accounts;
use crate::os;
use crate::update;
use crate::util::{setting, NAME, OLD_NAME};

pub fn run() {
    move_settings();
    if update::managed() {
        add_command();
        rename_shortcut();
    }
    rename_fish_file();
}

fn move_settings() {
    if setting("HOME").is_some() {
        return;
    }
    let (old, new) = (os::old_state_dir(), os::default_state_dir());
    let linked = old.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink());
    if new.exists() || linked || !old.is_dir() {
        return;
    }
    if fs::rename(&old, &new).is_ok() {
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(&new, &old);
    }
}

/// `paddock` beside `cc-sessions`, when the person kept the default command name.
fn add_command() {
    let mut config = accounts::load_config();
    if config.command != OLD_NAME {
        return;
    }
    let exe = os::installed_program();
    if os::command_target(NAME).is_none() && os::install_command(NAME, &exe, None).is_err() {
        return;
    }
    config.command = NAME.to_string();
    let _ = accounts::save_config(&config);
}

fn rename_shortcut() {
    let Some(old) = os::old_shortcut() else { return };
    if !accounts::load_config().shortcut || os::install_shortcut(&os::installed_program()).is_err() {
        return;
    }
    let _ = if old.is_dir() { fs::remove_dir_all(&old) } else { fs::remove_file(&old) };
}

fn rename_fish_file() {
    let Some(old) = os::old_fish_conf() else { return };
    if fs::remove_file(&old).is_ok() {
        if let Ok(all) = accounts::load_accounts(true) {
            let _ = accounts::write_shortcuts(&all);
        }
    }
}
