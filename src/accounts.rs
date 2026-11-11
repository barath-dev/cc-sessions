//! The account registry and the tool's settings.
//!
//! An account is a Claude Code config folder with a name and an alias. Removing
//! one only sets a flag here: its folder, login and history are never touched.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::os;
use crate::store::{self, state_file};
use crate::util::{err, expand_tilde, home, now_iso, path_str, tilde, Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub name: String,
    pub dir: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub removed: bool,
    #[serde(default, rename = "addedAt")]
    pub added_at: Option<String>,
    #[serde(default, rename = "removedAt")]
    pub removed_at: Option<String>,
}

impl Account {
    pub fn is_default(&self) -> bool {
        Path::new(&self.dir) == default_dir()
    }

    /// The folder to export as `CLAUDE_CONFIG_DIR`. `None` for the default
    /// account: setting the variable to `~/.claude` changes where Claude Code
    /// looks for that account's login, so it must stay unset.
    pub fn config_dir(&self) -> Option<String> {
        (!self.is_default()).then(|| self.dir.clone())
    }
}

pub fn default_dir() -> PathBuf {
    home().join(".claude")
}

// --------------------------------------------------------------------- settings

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// What the person types to start the tool.
    pub command: String,
    pub port: u16,
    pub shortcut: bool,
    #[serde(rename = "manageAliases")]
    pub manage_aliases: bool,
    /// Stop the server after this many minutes with no page open; 0 never stops it.
    #[serde(rename = "idleMinutes")]
    pub idle_minutes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config { command: "cc-sessions".into(), port: 4777, shortcut: false, manage_aliases: true, idle_minutes: 30 }
    }
}

pub fn load_config() -> Config {
    serde_json::from_value(store::read_json_or_null(&state_file("config.json"))).unwrap_or_default()
}

pub fn save_config(config: &Config) -> Result<()> {
    let value = serde_json::to_value(config).map_err(|e| Error(e.to_string()))?;
    store::locked(|| store::write_json(&state_file("config.json"), &value, true))
}

// --------------------------------------------------------------------- registry

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 31
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn valid_alias(alias: &str) -> bool {
    let mut chars = alias.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && alias.len() <= 41
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn looks_like_config_dir(path: &Path) -> bool {
    ["projects", "sessions", ".claude.json", "settings.json"].iter().any(|child| path.join(child).exists())
}

/// `~/.claude-*` folders, sorted.
fn dash_folders() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(home())
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".claude-")))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn suffix_name(path: &Path) -> String {
    let raw = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let cleaned: String = raw
        .trim_start_matches(".claude-")
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' { c } else { '-' })
        .collect();
    cleaned.trim_matches('-').to_string()
}

/// The accounts a first run would register: the folders that already exist.
/// `known_aliases` maps a folder to an alias the person already uses for it.
pub fn discover(known_aliases: &[(PathBuf, String)]) -> Vec<Account> {
    let stamp = now_iso();
    let alias_for = |dir: &Path| known_aliases.iter().find(|(d, _)| d == dir).map(|(_, a)| a.clone());
    let mut accounts = Vec::new();
    if default_dir().is_dir() {
        accounts.push(Account {
            name: "default".into(),
            dir: path_str(&default_dir()),
            alias: alias_for(&default_dir()),
            removed: false,
            added_at: Some(stamp.clone()),
            removed_at: None,
        });
    }
    for path in dash_folders() {
        if !looks_like_config_dir(&path) {
            continue;
        }
        let name = suffix_name(&path);
        if !valid_name(&name) || accounts.iter().any(|a| a.name == name) {
            continue;
        }
        let alias = alias_for(&path).unwrap_or_else(|| format!("claude-{name}"));
        accounts.push(Account {
            name,
            dir: path_str(&path),
            alias: Some(alias),
            removed: false,
            added_at: Some(stamp.clone()),
            removed_at: None,
        });
    }
    accounts
}

fn registry_file() -> PathBuf {
    state_file("accounts.json")
}

pub fn registry_exists() -> bool {
    registry_file().exists()
}

/// Write the registry and regenerate the account shortcuts from it.
pub fn save_all(accounts: &[Account]) -> Result<()> {
    store::write_json(&registry_file(), &json!({ "version": 1, "accounts": accounts }), true)?;
    write_shortcuts(accounts)
}

pub fn write_shortcuts(accounts: &[Account]) -> Result<()> {
    let entries: Vec<(String, Option<String>)> = accounts
        .iter()
        .filter(|a| !a.removed)
        .filter_map(|a| a.alias.clone().map(|alias| (alias, a.config_dir())))
        .collect();
    os::write_account_shortcuts(&entries, load_config().manage_aliases)
}

fn load_all() -> Result<Vec<Account>> {
    match store::read_json(&registry_file())? {
        Some(data) => {
            let list = data.get("accounts").cloned().unwrap_or(Value::Array(vec![]));
            serde_json::from_value(list).map_err(|e| Error(format!("accounts.json is not in the expected form: {e}")))
        }
        None => {
            // First run: register the folders that already exist.
            let accounts = discover(&[]);
            save_all(&accounts)?;
            Ok(accounts)
        }
    }
}

pub fn load_accounts(include_removed: bool) -> Result<Vec<Account>> {
    let all = store::locked(load_all)?;
    Ok(if include_removed { all } else { all.into_iter().filter(|a| !a.removed).collect() })
}

pub fn get_account(name: &str) -> Result<Account> {
    match load_accounts(true)?.into_iter().find(|a| a.name == name) {
        Some(acc) if acc.removed => err(format!("Account '{name}' has been removed. Restore it first.")),
        Some(acc) => Ok(acc),
        None => err(format!("No account named '{name}'.")),
    }
}

fn check_name(name: &str, accounts: &[Account], ignore: Option<usize>) -> Result<()> {
    if !valid_name(name) {
        return err("Name must be lowercase letters, digits and dashes, starting with a letter or digit.");
    }
    for (i, acc) in accounts.iter().enumerate() {
        if Some(i) == ignore || acc.name != name {
            continue;
        }
        return if acc.removed {
            err(format!("'{name}' is the name of a removed account. Restore it, or pick another name."))
        } else {
            err(format!("An account named '{name}' already exists."))
        };
    }
    Ok(())
}

fn check_alias(alias: &str, accounts: &[Account], ignore: Option<usize>) -> Result<()> {
    if !valid_alias(alias) {
        return err("Alias must be letters, digits, dashes or underscores, with no spaces.");
    }
    if alias == "claude" {
        return err("Alias cannot be 'claude': that would hide the claude command itself.");
    }
    for (i, acc) in accounts.iter().enumerate() {
        if Some(i) != ignore && !acc.removed && acc.alias.as_deref() == Some(alias) {
            return err(format!("Alias '{alias}' is already used by account '{}'.", acc.name));
        }
    }
    // The tool's own launcher for this alias (Windows) is not somebody else's command.
    let own = accounts.iter().any(|a| a.alias.as_deref() == Some(alias));
    if !own && os::which(alias).is_some() {
        return err(format!("'{alias}' is already a command on this machine. Pick another alias."));
    }
    Ok(())
}

fn find(accounts: &[Account], name: &str) -> Result<usize> {
    accounts.iter().position(|a| a.name == name).ok_or_else(|| Error(format!("No account named '{name}'.")))
}

/// Register an account. Returns (account, folder_was_created).
pub fn add_account(name: &str, alias: Option<&str>, folder: Option<&str>) -> Result<(Account, bool)> {
    store::locked(|| {
        let mut accounts = load_all()?;
        check_name(name, &accounts, None)?;

        let path = match folder.map(str::trim).filter(|f| !f.is_empty()) {
            Some(f) => expand_tilde(f),
            None => home().join(format!(".claude-{name}")),
        };
        if !path.is_absolute() {
            return err("Folder must be an absolute path, or start with ~.");
        }
        if !path.starts_with(home()) || path == home() {
            return err("Folder must be inside your home folder.");
        }
        if path.exists() && !path.is_dir() {
            return err(format!("{} exists and is not a folder.", tilde(&path_str(&path))));
        }
        if let Some(acc) = accounts.iter().find(|a| Path::new(&a.dir) == path) {
            let state = if acc.removed { "a removed account; restore it instead" } else { "already registered" };
            return err(format!("{} belongs to account '{}', which is {state}.", tilde(&path_str(&path)), acc.name));
        }

        let is_default = path == default_dir();
        let alias = match alias.map(str::trim).filter(|a| !a.is_empty()) {
            Some(a) => Some(a.to_string()),
            None if is_default => None,
            None => Some(format!("claude-{name}")),
        };
        if let Some(a) = &alias {
            check_alias(a, &accounts, None)?;
        }

        let created = !path.exists();
        if created {
            os::create_private_dir(&path)?;
        }
        let acc = Account {
            name: name.to_string(),
            dir: path_str(&path),
            alias,
            removed: false,
            added_at: Some(now_iso()),
            removed_at: None,
        };
        accounts.push(acc.clone());
        save_all(&accounts)?;
        Ok((acc, created))
    })
}

/// Change an account's name and/or alias. The folder is never touched.
///
/// `alias`: `None` keeps a custom alias and lets the `claude-NAME` pattern
/// follow the name; `Some("")` clears it (default account only).
pub fn rename_account(name: &str, new_name: Option<&str>, alias: Option<&str>) -> Result<Account> {
    store::locked(|| {
        let mut accounts = load_all()?;
        let i = find(&accounts, name)?;
        if accounts[i].removed {
            return err(format!("Account '{name}' has been removed. Restore it first."));
        }
        let new_name = new_name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(name).to_string();
        if new_name != name {
            check_name(&new_name, &accounts, Some(i))?;
        }
        let current = accounts[i].alias.clone();
        let new_alias = match alias.map(str::trim) {
            None if current.as_deref() == Some(format!("claude-{name}").as_str()) => Some(format!("claude-{new_name}")),
            None => current.clone(),
            Some("") if accounts[i].is_default() => None,
            Some("") => return err("This account needs an alias, otherwise it cannot be started."),
            Some(a) => Some(a.to_string()),
        };
        if let Some(a) = new_alias.as_deref().filter(|a| Some(*a) != current.as_deref()) {
            check_alias(a, &accounts, Some(i))?;
        }
        accounts[i].name = new_name;
        accounts[i].alias = new_alias;
        save_all(&accounts)?;
        Ok(accounts[i].clone())
    })
}

/// Soft delete: mark the account removed. Its folder and login are untouched.
pub fn remove_account(name: &str) -> Result<Account> {
    store::locked(|| {
        let mut accounts = load_all()?;
        let i = find(&accounts, name)?;
        if accounts[i].removed {
            return err(format!("Account '{name}' is already removed."));
        }
        accounts[i].removed = true;
        accounts[i].removed_at = Some(now_iso());
        save_all(&accounts)?;
        Ok(accounts[i].clone())
    })
}

pub fn restore_account(name: &str, alias: Option<&str>) -> Result<Account> {
    store::locked(|| {
        let mut accounts = load_all()?;
        let i = find(&accounts, name)?;
        if !accounts[i].removed {
            return err(format!("Account '{name}' is not removed."));
        }
        if !Path::new(&accounts[i].dir).is_dir() {
            return err(format!("Its folder {} no longer exists, so it cannot be restored.", tilde(&accounts[i].dir)));
        }
        if let Some(a) = alias.map(str::trim).filter(|a| !a.is_empty()) {
            accounts[i].alias = Some(a.to_string());
        }
        if let Some(a) = accounts[i].alias.clone() {
            check_alias(&a, &accounts, Some(i))?;
        }
        accounts[i].removed = false;
        accounts[i].removed_at = None;
        save_all(&accounts)?;
        Ok(accounts[i].clone())
    })
}

/// `~/.claude-*` folders the registry has never seen.
pub fn unregistered_folders() -> Result<Vec<String>> {
    let known: Vec<PathBuf> = load_accounts(true)?.iter().map(|a| PathBuf::from(&a.dir)).collect();
    Ok(dash_folders().into_iter().filter(|p| !known.contains(p)).map(|p| path_str(&p)).collect())
}

pub const DEFAULT_RETENTION_DAYS: i64 = 30;

/// How long Claude Code itself keeps this account's conversations (its
/// `cleanupPeriodDays`). Reported only; this tool never changes settings.
pub fn retention_days(acc: &Account) -> i64 {
    store::read_json_or_null(&Path::new(&acc.dir).join("settings.json"))
        .get("cleanupPeriodDays")
        .and_then(Value::as_i64)
        .filter(|days| *days > 0)
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_alias_rules() {
        assert!(valid_name("work"));
        assert!(valid_name("kt-2"));
        assert!(!valid_name("Work"));
        assert!(!valid_name("-work"));
        assert!(!valid_name("my work"));
        assert!(!valid_name(""));
        assert!(valid_alias("claude-work"));
        assert!(valid_alias("cw_2"));
        assert!(!valid_alias("claude work"));
        assert!(!valid_alias("-x"));
    }

    #[test]
    fn names_come_from_folder_suffixes() {
        assert_eq!(suffix_name(Path::new("/home/me/.claude-work")), "work");
        assert_eq!(suffix_name(Path::new("/home/me/.claude-My Team")), "my-team");
    }
}
