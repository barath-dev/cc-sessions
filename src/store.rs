//! The tool's own JSON records: read, write via temp file and rename with the
//! previous version kept as `.bak`, and a lock for read-modify-write.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;

use crate::os;
use crate::util::{path_str, tilde, Error, Result};

pub fn state_dir() -> PathBuf {
    match std::env::var_os("CC_SESSIONS_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => os::default_state_dir(),
    }
}

pub fn state_file(name: &str) -> PathBuf {
    state_dir().join(name)
}

/// `Ok(None)` when the file does not exist. A file that exists but cannot be
/// parsed is an error, so a damaged record is never silently replaced.
pub fn read_json(path: &Path) -> Result<Option<Value>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unreadable(path, &e.to_string())),
    };
    serde_json::from_str(&text).map(Some).map_err(|e| unreadable(path, &e.to_string()))
}

fn unreadable(path: &Path, why: &str) -> Error {
    let shown = tilde(&path_str(path));
    Error(format!("{shown} could not be read ({why}). The previous version is in {shown}.bak."))
}

/// Lenient read for files that are only hints (other programs' records, caches).
pub fn read_json_or_null(path: &Path) -> Value {
    fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

pub fn write_json(path: &Path, data: &Value, backup: bool) -> Result<()> {
    let mut text = serde_json::to_string_pretty(data).map_err(|e| Error(e.to_string()))?;
    text.push('\n');
    write_text(path, &text, backup)
}

pub fn write_text(path: &Path, text: &str, backup: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = sibling(path, ".tmp");
    fs::write(&tmp, text)?;
    if backup && path.exists() {
        fs::copy(path, sibling(path, ".bak"))?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

static THREADS: Mutex<()> = Mutex::new(());

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Run `f` holding the state lock, across threads and processes. Re-entrant
/// within a thread, so an operation may call another that also locks.
pub fn locked<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    if DEPTH.with(|d| d.get()) > 0 {
        return f();
    }
    let _threads = THREADS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    fs::create_dir_all(state_dir())?;
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(state_file(".lock"))?;
    os::lock_exclusive(&file)?;
    DEPTH.with(|d| d.set(1));
    let result = f();
    DEPTH.with(|d| d.set(0));
    result
    // Dropping `file` releases the lock.
}
