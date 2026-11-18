//! The account registry and the program's settings.
//!
//! An account is a config folder of one tool (Claude Code's `~/.claude-work`,
//! Codex's `~/.codex`) with a name and an alias. Removing one only sets a flag
//! here: its folder, login and history are never touched.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::os;
use crate::shell::Shortcut;
use crate::store::{self, state_file};
use crate::tools::{self, Tool};
use crate::util::{err, expand_tilde, home, now_iso, path_str, tilde, Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub name: String,
    /// Which tool the folder belongs to; Claude Code for accounts registered before there was a choice.
    #[serde(default)]
    pub tool: Tool,
    pub dir: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub removed: bool,
    #[serde(default, rename = "addedAt")]
    pub added_at: Option<String>,
    #[serde(default, rename = "removedAt")]
    pub removed_at: Option<String>,
    /// Picked in the page, as `#rrggbb`. Without one the page assigns a colour by position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl Account {
    /// True for the tool's own folder, `~/.claude` or `~/.codex`.
    pub fn is_default(&self) -> bool {
        Path::new(&self.dir) == self.tool.default_dir()
    }

    /// The folder to export as the tool's variable (`CLAUDE_CONFIG_DIR`,
    /// `CODEX_HOME`). `None` for the default folder: setting the variable to
    /// `~/.claude` changes where Claude Code looks for that account's login, so
    /// it must stay unset.
    pub fn config_dir(&self) -> Option<String> {
        (!self.is_default()).then(|| self.dir.clone())
    }

    /// The shortcut that starts it, or the plain command for a default folder without one.
    pub fn start_command(&self) -> Option<String> {
        self.alias.clone().or_else(|| self.is_default().then(|| self.tool.program().to_string()))
    }
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
    /// Install new releases by itself (only the installer's copy does).
    #[serde(rename = "autoUpdate")]
    pub auto_update: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { command: "cc-sessions".into(), port: 4777, shortcut: true, manage_aliases: true, idle_minutes: 30, auto_update: true }
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

/// A tool's `~/.claude-*` or `~/.codex-*` folders, sorted.
fn dash_folders(tool: Tool) -> Vec<PathBuf> {
    let prefix = tool.folder_prefix();
    let mut found: Vec<PathBuf> = fs::read_dir(home())
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix)))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// Every folder that holds a tool's data: its default folder and its dash folders.
fn tool_folders(tool: Tool) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Some(tool.default_dir()).filter(|d| d.is_dir()).into_iter().collect();
    found.extend(dash_folders(tool).into_iter().filter(|p| tool.looks_like_config_dir(p)));
    found
}

/// The account name a folder suggests: `work` for `~/.claude-work`, `codex-work`
/// for `~/.codex-work` (so it cannot clash with a Claude Code account).
fn suffix_name(path: &Path, tool: Tool) -> String {
    let raw = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let cleaned: String = raw
        .trim_start_matches(&tool.folder_prefix())
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' { c } else { '-' })
        .collect();
    let name = cleaned.trim_matches('-').to_string();
    match tool {
        Tool::Claude => name,
        _ => format!("{}-{name}", tool.id()),
    }
}

/// The account a folder of a tool would be registered as.
fn found_account(tool: Tool, path: &Path, alias: Option<String>, stamp: &str) -> Account {
    let is_default = path == tool.default_dir();
    let name = if is_default { tool.default_name().to_string() } else { suffix_name(path, tool) };
    let alias = alias.or_else(|| (!is_default).then(|| tool.alias_for(&name)));
    Account {
        name,
        tool,
        dir: path_str(path),
        alias,
        removed: false,
        added_at: Some(stamp.to_string()),
        removed_at: None,
        color: None,
    }
}

/// The accounts a first run would register: the folders of every tool that already exist.
/// `known_aliases` maps a folder to an alias the person already uses for it.
pub fn discover(known_aliases: &[(PathBuf, String)]) -> Vec<Account> {
    let stamp = now_iso();
    let alias_for = |dir: &Path| known_aliases.iter().find(|(d, _)| d == dir).map(|(_, a)| a.clone());
    let mut accounts: Vec<Account> = Vec::new();
    for tool in tools::ALL {
        for path in tool_folders(tool) {
            let acc = found_account(tool, &path, alias_for(&path), &stamp);
            if valid_name(&acc.name) && !accounts.iter().any(|a| a.name == acc.name) {
                accounts.push(acc);
            }
        }
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
    let entries: Vec<Shortcut> = accounts
        .iter()
        .filter(|a| !a.removed)
        .filter_map(|a| a.alias.clone().map(|alias| Shortcut { alias, tool: a.tool, config_dir: a.config_dir() }))
        .collect();
    os::write_account_shortcuts(&entries, load_config().manage_aliases)
}

fn load_all() -> Result<Vec<Account>> {
    match store::read_json(&registry_file())? {
        Some(data) => parse_accounts(data.get("accounts").cloned().unwrap_or(Value::Array(vec![]))),
        None => {
            // First run: register the folders that already exist.
            let accounts = discover(&[]);
            save_all(&accounts)?;
            Ok(accounts)
        }
    }
}

/// An account saved without a tool is Claude Code's, unless its folder says Codex: a version
/// from before Codex (2.7 and older) drops the tool when it saves the registry again.
fn parse_accounts(list: Value) -> Result<Vec<Account>> {
    let mut accounts: Vec<Account> = serde_json::from_value(list.clone()).map_err(|e| Error(format!("accounts.json is not in the expected form: {e}")))?;
    for (acc, saved) in accounts.iter_mut().zip(list.as_array().into_iter().flatten()) {
        let folder = Path::new(&acc.dir).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if saved.get("tool").is_none() && (folder == ".codex" || folder.starts_with(&Tool::Codex.folder_prefix())) {
            acc.tool = Tool::Codex;
        }
    }
    Ok(accounts)
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
    if let Some(tool) = tools::ALL.into_iter().find(|t| t.program() == alias) {
        return err(format!("Alias cannot be '{alias}': that would hide the {} command itself.", tool.program()));
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

/// Register an account of a tool. Returns (account, folder_was_created).
pub fn add_account(name: &str, alias: Option<&str>, folder: Option<&str>, tool: Tool) -> Result<(Account, bool)> {
    store::locked(|| {
        let mut accounts = load_all()?;
        check_name(name, &accounts, None)?;

        let path = match folder.map(str::trim).filter(|f| !f.is_empty()) {
            Some(f) => expand_tilde(f),
            None => home().join(format!("{}{}", tool.folder_prefix(), name.strip_prefix("codex-").filter(|_| tool == Tool::Codex).unwrap_or(name))),
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

        if let Some(other) = tools::ALL.into_iter().find(|t| *t != tool && (path == t.default_dir() || is_dash_folder(&path, *t))) {
            return err(format!("{} is a {} folder. Add it as a {} account.", tilde(&path_str(&path)), other.label(), other.label()));
        }
        let is_default = path == tool.default_dir();
        let alias = match alias.map(str::trim).filter(|a| !a.is_empty()) {
            Some(a) => Some(a.to_string()),
            None if is_default => None,
            None => Some(tool.alias_for(name)),
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
            tool,
            dir: path_str(&path),
            alias,
            removed: false,
            added_at: Some(now_iso()),
            removed_at: None,
            color: None,
        };
        accounts.push(acc.clone());
        save_all(&accounts)?;
        Ok((acc, created))
    })
}

/// Change an account's name and/or alias. The folder is never touched.
///
/// `alias`: `None` keeps a custom alias and lets the `claude-NAME` (or
/// `codex-NAME`) pattern follow the name; `Some("")` clears it (default folder only).
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
        let tool = accounts[i].tool;
        let new_alias = match alias.map(str::trim) {
            None if current.as_deref() == Some(tool.alias_for(name).as_str()) => Some(tool.alias_for(&new_name)),
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

/// Give an account a colour of its own (`#rrggbb`), or with `None` or an empty
/// string go back to the one the page assigns.
pub fn set_color(name: &str, color: Option<&str>) -> Result<Account> {
    let color = color.map(|c| c.trim().to_lowercase()).filter(|c| !c.is_empty());
    if let Some(c) = &color {
        if c.len() != 7 || !c.starts_with('#') || !c[1..].chars().all(|ch| ch.is_ascii_hexdigit()) {
            return err("A colour is written as # and six hex digits, like #2f6fd0.");
        }
    }
    store::locked(|| {
        let mut accounts = load_all()?;
        let i = find(&accounts, name)?;
        accounts[i].color = color;
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

fn is_dash_folder(path: &Path, tool: Tool) -> bool {
    path.parent() == Some(home().as_path()) && path.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&tool.folder_prefix()))
}

/// A folder of a tool that the registry has never seen.
#[derive(Debug, Clone, Serialize)]
pub struct Unregistered {
    pub dir: String,
    pub tool: Tool,
    /// The account name it would get.
    pub name: String,
}

/// Folders of every tool that the registry has never seen: `~/.claude-*`, and
/// `~/.codex` and `~/.codex-*` (which a registry from before Codex never looked for).
pub fn unregistered_folders() -> Result<Vec<Unregistered>> {
    let known: Vec<PathBuf> = load_accounts(true)?.iter().map(|a| PathBuf::from(&a.dir)).collect();
    let mut found = Vec::new();
    for tool in tools::ALL {
        let mut folders = if tool == Tool::Claude { dash_folders(tool) } else { tool_folders(tool) };
        folders.retain(|p| !known.contains(p));
        for path in folders {
            let name = if path == tool.default_dir() { tool.default_name().to_string() } else { suffix_name(&path, tool) };
            found.push(Unregistered { dir: path_str(&path), tool, name });
        }
    }
    Ok(found)
}

pub const DEFAULT_RETENTION_DAYS: i64 = 30;

/// How long the tool itself keeps this account's conversations: Claude Code's
/// `cleanupPeriodDays`. `None` when it keeps them until they are deleted by hand,
/// as Codex does. Reported only; this program never changes settings.
pub fn retention_days(acc: &Account) -> Option<i64> {
    match acc.tool {
        Tool::Claude => Some(
            store::read_json_or_null(&Path::new(&acc.dir).join("settings.json"))
                .get("cleanupPeriodDays")
                .and_then(Value::as_i64)
                .filter(|days| *days > 0)
                .unwrap_or(DEFAULT_RETENTION_DAYS),
        ),
        Tool::Codex => None,
    }
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
        assert_eq!(suffix_name(Path::new("/home/me/.claude-work"), Tool::Claude), "work");
        assert_eq!(suffix_name(Path::new("/home/me/.claude-My Team"), Tool::Claude), "my-team");
        assert_eq!(suffix_name(Path::new("/home/me/.codex-work"), Tool::Codex), "codex-work");
    }

    #[test]
    fn registries_from_before_codex_are_claude_code() {
        let acc: Account = serde_json::from_value(serde_json::json!({ "name": "work", "dir": "/h/.claude-work" })).unwrap();
        assert_eq!(acc.tool, Tool::Claude);
        let acc: Account = serde_json::from_value(serde_json::json!({ "name": "codex", "tool": "codex", "dir": "/h/.codex" })).unwrap();
        assert_eq!(acc.tool, Tool::Codex);
        // Saved again by 2.7, which drops the tool: the folder still tells.
        let saved = parse_accounts(serde_json::json!([
            { "name": "work", "dir": "/h/.claude-work" },
            { "name": "codex", "dir": "/h/.codex" },
            { "name": "codex-work", "dir": "/h/.codex-work" },
            { "name": "odd", "tool": "claude", "dir": "/h/.codex-odd" },
        ]))
        .unwrap();
        let tools: Vec<Tool> = saved.iter().map(|a| a.tool).collect();
        assert_eq!(tools, [Tool::Claude, Tool::Codex, Tool::Codex, Tool::Claude]);
    }
}
