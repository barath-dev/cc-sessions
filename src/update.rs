//! Keeping the program up to date: ask GitHub for the latest release, and put it in
//! place of the installer's copy once its SHA-256 matches the release's list.
//!
//! Only the copy the installer put in place updates itself; any other (a build from
//! source, a package manager's) is told about the new version and left alone.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};

use crate::accounts;
use crate::os;
use crate::proc;
use crate::server::VERSION;
use crate::store::{self, state_dir, state_file};
use crate::util::{err, now_ms, Error, Result};

const REPO: &str = "barath-dev/cc-sessions";
/// How long a check's answer is trusted before asking again.
const CHECK_EVERY_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub url: String,
    pub notes: String,
}

/// Where releases come from. CC_SESSIONS_UPDATE_BASE points it somewhere else, for the
/// tests and for trying an update against a local copy of a release.
fn base() -> Option<String> {
    std::env::var("CC_SESSIONS_UPDATE_BASE").ok().filter(|b| !b.is_empty()).map(|b| b.trim_end_matches('/').to_string())
}

fn latest_url() -> String {
    match base() {
        Some(base) => format!("{base}/latest.json"),
        None => format!("https://api.github.com/repos/{REPO}/releases/latest"),
    }
}

fn download_url(tag: &str, file: &str) -> String {
    match base() {
        Some(base) => format!("{base}/download/{tag}/{file}"),
        None => format!("https://github.com/{REPO}/releases/download/{tag}/{file}"),
    }
}

/// "v2.6.0" or "2.6.0" as numbers; pre-releases ("2.6.0-beta") are not offered.
pub fn parse_version(tag: &str) -> Option<(u32, u32, u32)> {
    let mut parts = tag.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

pub fn is_newer(tag: &str) -> bool {
    match (parse_version(tag), parse_version(VERSION)) {
        (Some(theirs), Some(ours)) => theirs > ours,
        _ => false,
    }
}

/// The release file for this system, as release.yml names it.
pub fn asset_name() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("cc-sessions-{}-{arch}{}", os::OS_NAME, if os::IS_WINDOWS { ".exe" } else { "" })
}

/// Whether this is the copy the installer put in place, the only one that updates itself.
pub fn managed() -> bool {
    let (Ok(exe), Ok(installed)) = (std::env::current_exe().and_then(fs::canonicalize), fs::canonicalize(os::installed_program())) else {
        return false;
    };
    exe == installed
}

fn work_dir() -> Result<PathBuf> {
    let dir = state_dir().join("update");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

// ------------------------------------------------------------------ remembered

fn remembered() -> Value {
    match store::read_json_or_null(&state_file("update.json")) {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    }
}

fn remember(changes: Value) {
    let mut state = remembered();
    if let (Some(state), Some(changes)) = (state.as_object_mut(), changes.as_object()) {
        for (key, value) in changes {
            state.insert(key.clone(), value.clone());
        }
    }
    let _ = store::write_json(&state_file("update.json"), &state, false);
}

/// What the page shows about updates.
pub fn status() -> Value {
    let state = remembered();
    let latest = state.get("latest").and_then(Value::as_str);
    json!({
        "current": VERSION,
        "latest": latest,
        "available": latest.is_some_and(is_newer),
        "auto": accounts::load_config().auto_update,
        "managed": managed(),
        "checkedAt": state.get("checkedAt"),
        "error": state.get("error"),
        "notesUrl": state.get("notesUrl"),
        "justUpdated": state.get("justUpdated"),
    })
}

/// The newer version a past check found, without asking again.
pub fn known_newer() -> Option<String> {
    remembered().get("latest").and_then(Value::as_str).filter(|tag| is_newer(tag)).map(String::from)
}

/// Whether a day has passed since the last check.
pub fn due() -> bool {
    let checked = remembered().get("checkedAt").and_then(Value::as_i64).unwrap_or(0);
    now_ms() - checked > CHECK_EVERY_MS
}

// ------------------------------------------------------------------- checking

fn latest() -> Result<Release> {
    let file = work_dir()?.join("latest.json");
    os::download(&latest_url(), &file, Duration::from_secs(30))?;
    let text = fs::read_to_string(&file)?;
    let _ = fs::remove_file(&file);
    let release: Value = serde_json::from_str(&text).map_err(|_| Error("GitHub's answer about the latest release could not be read.".into()))?;
    let tag = release.get("tag_name").and_then(Value::as_str).filter(|t| parse_version(t).is_some());
    let Some(tag) = tag else {
        return err("GitHub did not name a latest release.");
    };
    let text = |field: &str| release.get(field).and_then(Value::as_str).unwrap_or("").to_string();
    Ok(Release { tag: tag.to_string(), url: text("html_url"), notes: text("body").chars().take(2000).collect() })
}

/// Ask GitHub now and remember the answer. Returns the release if it is newer than this one.
pub fn check() -> Result<Option<Release>> {
    match latest() {
        Ok(release) => {
            remember(json!({ "checkedAt": now_ms(), "latest": release.tag, "notesUrl": release.url, "notes": release.notes, "error": null }));
            Ok(is_newer(&release.tag).then_some(release))
        }
        Err(e) => {
            remember(json!({ "checkedAt": now_ms(), "error": e.to_string() }));
            Err(e)
        }
    }
}

// ----------------------------------------------------------------- installing

/// Download the release for this system, check it, and put it in place of the installed program.
pub fn install(release: &Release) -> Result<()> {
    if !managed() {
        return err("This copy of cc-sessions was not put in place by its installer, so it does not update itself. Run the install line to update it.");
    }
    let dir = work_dir()?;
    let asset = asset_name();
    let file = dir.join(&asset);
    let sums = dir.join("checksums.txt");
    let result = fetch_and_place(release, &asset, &file, &sums);
    let _ = fs::remove_file(&file);
    let _ = fs::remove_file(&sums);
    match &result {
        Ok(()) => remember(json!({
            "error": null,
            "justUpdated": { "from": VERSION, "to": release.tag, "at": now_ms(), "notesUrl": release.url },
        })),
        Err(e) => remember(json!({ "error": e.to_string() })),
    }
    result
}

fn fetch_and_place(release: &Release, asset: &str, file: &Path, sums: &Path) -> Result<()> {
    os::download(&download_url(&release.tag, asset), file, Duration::from_secs(300))?;
    os::download(&download_url(&release.tag, "checksums.txt"), sums, Duration::from_secs(30))?;
    let expected = fs::read_to_string(sums)?
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let (hash, name) = (parts.next()?, parts.next()?);
            (name.trim_start_matches('*') == asset).then(|| hash.to_lowercase())
        })
        .ok_or_else(|| Error(format!("The release's checksum list does not include {asset}.")))?;
    if sha256_hex(&fs::read(file)?) != expected {
        return err("The download does not match the release's checksum. Nothing was changed.");
    }
    // It must at least run and say what it is before it replaces anything.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o755))?;
    }
    let out = proc::run(Command::new(file).arg("--version"), Duration::from_secs(15))?;
    if !out.stdout.trim_start().starts_with("cc-sessions") {
        return err("The downloaded program did not start properly. Nothing was changed.");
    }
    os::replace_program(file, &os::installed_program())
}

/// What an update leaves behind: Windows' renamed old program, a half-written new one.
pub fn tidy() {
    let installed = os::installed_program();
    let _ = fs::remove_file(installed.with_extension("old.exe"));
    let _ = fs::remove_file(installed.with_extension("new"));
}

/// The server's daily round: check, and install if allowed. True when a new program is in place.
pub fn round() -> bool {
    if !due() {
        return false;
    }
    match check() {
        Ok(Some(release)) if accounts::load_config().auto_update && managed() => install(&release).is_ok(),
        _ => false,
    }
}

// --------------------------------------------------------------------- sha-256

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01,
    0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
    0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08,
    0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 of some bytes, as lowercase hex (FIPS 180-4), to check downloads against the release's list.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 64];
        for (word, bytes) in w.iter_mut().zip(block.chunks(4)) {
            *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for (k, word) in K.iter().zip(w.iter()) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(*k).wrapping_add(*word);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (total, part) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *total = total.wrapping_add(part);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // A million 'a's crosses many blocks.
        assert_eq!(sha256_hex(&vec![b'a'; 1_000_000]), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(parse_version("v2.10.0"), Some((2, 10, 0)));
        assert_eq!(parse_version("2.6.0"), Some((2, 6, 0)));
        assert_eq!(parse_version("v2.6.0-beta.1"), None, "pre-releases are not offered");
        assert_eq!(parse_version("v2.6"), None);
        assert_eq!(parse_version("v2.6.0.1"), None);
        assert!(is_newer("v99.0.0"));
        assert!(!is_newer(&format!("v{VERSION}")), "the same version is not newer");
        assert!(!is_newer("v0.1.0"), "never back to an older one");
        assert!(!is_newer("nonsense"));
    }

    #[test]
    fn asset_names_match_the_release_files() {
        let name = asset_name();
        assert!(name.starts_with(&format!("cc-sessions-{}-", os::OS_NAME)), "{name}");
        assert!(name.ends_with("-arm64") || name.ends_with("-x86_64") || name.ends_with("-arm64.exe") || name.ends_with("-x86_64.exe"), "{name}");
    }
}
