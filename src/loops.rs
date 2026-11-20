//! Sessions that may be going round in circles: the same error back after each fix,
//! edits undoing edits, tokens spent with little changed, and the person saying it
//! still does not work. Facts are gathered while a transcript is indexed (read-only);
//! the verdict compares each session with the person's own usual.
//!
//! It only says "worth a look", with its reasons, and learns from "Not a loop".

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::history::HistoryItem;
use crate::store::{self, state_file};
use crate::util::Result;

/// Only sessions with this much work in them are judged.
const MIN_REPLIES: u32 = 20;
const MIN_FRESH_TOKENS: u64 = 250_000;
const MIN_EDITS: u32 = 5;
/// "The last stretch" of a session, in replies.
const TAIL: usize = 15;
/// The score from which a session is worth a look; each "Not a loop" raises it a little.
const BAR: u32 = 30;
const BAR_STEP: u32 = 2;
const BAR_MAX: u32 = 50;

/// What a transcript says about the work done in it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Work {
    pub replies: u32,
    /// Edits to the project (not to Claude Code's own plans, memory or settings).
    pub edits: u32,
    /// Lines written by those edits, and how many of them were still there at the end.
    pub lines_written: u32,
    pub lines_kept: u32,
    /// Lines of the project as it was that were changed or removed.
    pub lines_replaced: u32,
    /// Edits that put back text an earlier edit took out.
    pub undone: u32,
    /// The file edited most, by name only, and how often.
    pub busiest_file: Option<String>,
    pub busiest_edits: u32,
    /// The error (from running code) that came back most often after an edit, and how often.
    pub repeat_error: Option<String>,
    pub repeats: u32,
    /// Prompts like "still not working" typed after edits had been made, and one of them.
    pub still_prompts: u32,
    pub still_example: Option<String>,
    /// The last stretch: errors, edits, and lines kept by those edits.
    pub tail_errors: u32,
    pub tail_edits: u32,
    pub tail_kept: i64,
}

enum Event {
    Reply,
    Edit { written: u32, undone_lines: u32 },
    Error,
}

/// Collects `Work` while a transcript is read, one record at a time.
#[derive(Default)]
pub struct Gatherer {
    work: Work,
    /// Tool calls that ran a command, so their failures can be told apart from other tools'.
    commands: HashSet<String>,
    /// Lines this session wrote into each file, still there as far as we know.
    written: HashMap<String, HashMap<String, u32>>,
    /// Text each file had taken out of it, to see edits that put it back.
    taken_out: HashMap<String, HashSet<String>>,
    edits_by_file: HashMap<String, u32>,
    /// For each error: how many edits had been made when it was last seen, and how often it came back after one.
    last_seen: HashMap<String, u32>,
    came_back: HashMap<String, u32>,
    /// Each error as it was last written, for showing (the key is the version made to match itself).
    as_written: HashMap<String, String>,
    events: Vec<Event>,
}

fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty())
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

impl Gatherer {
    pub fn reply(&mut self) {
        self.work.replies += 1;
        self.events.push(Event::Reply);
    }

    /// A Claude Code reply: note the commands it ran and the edits it made.
    pub fn assistant(&mut self, rec: &Value) {
        let Some(blocks) = rec.pointer("/message/content").and_then(Value::as_array) else { return };
        for block in blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use")) {
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            let input = block.get("input").cloned().unwrap_or(Value::Null);
            let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            match name {
                "Bash" => {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        self.command(id);
                    }
                }
                "Write" => self.wrote(&text(&input, "file_path"), &text(&input, "content")),
                "Edit" => self.edited(&text(&input, "file_path"), &[(text(&input, "old_string"), text(&input, "new_string"))]),
                "MultiEdit" => {
                    let pairs: Vec<(String, String)> =
                        input.get("edits").and_then(Value::as_array).into_iter().flatten().map(|e| (text(e, "old_string"), text(e, "new_string"))).collect();
                    self.edited(&text(&input, "file_path"), &pairs);
                }
                _ => {}
            }
        }
    }

    /// A tool call that ran a command; its failure is reported to `failed` by the same id.
    pub fn command(&mut self, id: &str) {
        self.commands.insert(id.to_string());
    }

    /// The tool's own notes (plans, memory, settings) are not the project.
    fn own_notes(file: &str) -> bool {
        [".claude", ".codex"].iter().any(|dir| file.contains(&format!("/{dir}")) || file.contains(&format!("\\{dir}")))
    }

    /// An edit to a file: each pair is text taken out and the text put in its place.
    pub fn edited(&mut self, file: &str, pairs: &[(String, String)]) {
        if file.is_empty() || Self::own_notes(file) {
            return;
        }
        let (written, undone_lines) = pairs.iter().fold((0, 0), |(w, u), (old, new)| {
            let (dw, du) = self.edit(file, old, new);
            (w + dw, u + du)
        });
        self.count_edit(file, written, undone_lines);
    }

    /// A file written whole.
    pub fn wrote(&mut self, file: &str, content: &str) {
        if file.is_empty() || Self::own_notes(file) {
            return;
        }
        let (written, undone_lines) = self.write(file, content);
        self.count_edit(file, written, undone_lines);
    }

    fn count_edit(&mut self, file: &str, written: u32, undone_lines: u32) {
        self.work.edits += 1;
        *self.edits_by_file.entry(file.to_string()).or_default() += 1;
        self.events.push(Event::Edit { written, undone_lines });
    }

    fn edit(&mut self, file: &str, old: &str, new: &str) -> (u32, u32) {
        let taken = self.taken_out.entry(file.to_string()).or_default();
        if !new.trim().is_empty() && taken.contains(new.trim()) {
            self.work.undone += 1;
        }
        if !old.trim().is_empty() {
            taken.insert(old.trim().to_string());
        }
        let written = self.written.entry(file.to_string()).or_default();
        let mut undone_lines = 0;
        for line in lines(old) {
            match written.get_mut(line).filter(|n| **n > 0) {
                Some(n) => {
                    *n -= 1;
                    undone_lines += 1;
                }
                None => self.work.lines_replaced += 1,
            }
        }
        let mut count = 0;
        for line in lines(new) {
            *written.entry(line.to_string()).or_default() += 1;
            count += 1;
        }
        self.work.lines_written += count;
        (count, undone_lines)
    }

    fn write(&mut self, file: &str, content: &str) -> (u32, u32) {
        let mut next: HashMap<String, u32> = HashMap::new();
        for line in lines(content) {
            *next.entry(line.to_string()).or_default() += 1;
        }
        let before = self.written.remove(file).unwrap_or_default();
        // Lines this session wrote before and the new version drops were undone; new ones are written.
        let undone_lines = before.iter().map(|(l, n)| n.saturating_sub(*next.get(l).unwrap_or(&0))).sum();
        let count = next.iter().map(|(l, n)| n.saturating_sub(*before.get(l).unwrap_or(&0))).sum();
        self.work.lines_written += count;
        self.written.insert(file.to_string(), next);
        (count, undone_lines)
    }

    /// A Claude Code record with tool results: note failures of commands that ran code.
    pub fn results(&mut self, rec: &Value) {
        let Some(blocks) = rec.pointer("/message/content").and_then(Value::as_array) else { return };
        for block in blocks {
            if !block.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let id = block.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
            let text = match block.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
                _ => String::new(),
            };
            self.failed(id, &text);
        }
    }

    /// The command with this id failed, with this output. Only errors from running code count.
    pub fn failed(&mut self, id: &str, text: &str) {
        if !self.commands.contains(id) {
            return;
        }
        let Some(error) = code_error(text) else { return };
        if let Some(line) = error_line(text) {
            self.as_written.insert(error.clone(), shorten(line.trim(), 110));
        }
        let edits = self.work.edits;
        if self.last_seen.get(&error).is_some_and(|at| edits > *at) {
            *self.came_back.entry(error.clone()).or_default() += 1;
        }
        self.last_seen.insert(error, edits);
        self.events.push(Event::Error);
    }

    /// Something the person typed. "Still not working" after edits is the plainest sign of a loop.
    pub fn prompt(&mut self, text: &str) {
        if self.work.edits > 0 && sounds_stuck(text) {
            self.work.still_prompts += 1;
            if self.work.still_example.is_none() {
                self.work.still_example = Some(shorten(text, 90));
            }
        }
    }

    pub fn finish(mut self) -> Work {
        self.work.lines_kept = self.written.values().flat_map(|w| w.values()).sum();
        if let Some((file, n)) = self.edits_by_file.iter().max_by_key(|(_, n)| **n) {
            self.work.busiest_file = Some(file_name(file));
            self.work.busiest_edits = *n;
        }
        if let Some((error, n)) = self.came_back.iter().max_by_key(|(_, n)| **n) {
            self.work.repeat_error = Some(self.as_written.get(error).cloned().unwrap_or_else(|| error.clone()));
            self.work.repeats = *n;
        }
        // The last stretch: from the TAIL-th last reply on.
        let start = self.events.iter().enumerate().filter(|(_, e)| matches!(e, Event::Reply)).map(|(i, _)| i).rev().nth(TAIL - 1).unwrap_or(0);
        for event in &self.events[start..] {
            match event {
                Event::Error => self.work.tail_errors += 1,
                Event::Edit { written, undone_lines } => {
                    self.work.tail_edits += 1;
                    self.work.tail_kept += i64::from(*written) - i64::from(*undone_lines);
                }
                Event::Reply => {}
            }
        }
        self.work
    }
}

/// Words that say an attempt did not work: something that persists, or a fix that failed.
/// "Not working" alone is how a new problem is first reported, so it does not count.
const STUCK: &[&str] = &[
    "still not", "still isn't", "still isnt", "still doesn't", "still doesnt", "still facing", "still getting", "still failing",
    "still broken", "still the same", "still seeing", "still showing", "still happening", "still wrong", "still an issue",
    "still having", "same error", "same issue", "same problem", "same bug", "didn't work", "didnt work", "did not work",
    "not fixed", "isn't fixed", "nothing changed", "wrong again", "broken again", "failing again", "again the same",
    "not working again", "doesn't work either", "doesnt work either",
];

/// Claude Code's own notes that arrive as if typed: the summary after a conversation is compacted.
const NOT_TYPED: &[&str] = &["this session is being continued from a previous conversation"];

pub fn sounds_stuck(text: &str) -> bool {
    let text = text.to_lowercase().replace('’', "'");
    !NOT_TYPED.iter().any(|note| text.starts_with(note)) && STUCK.iter().any(|phrase| text.contains(phrase))
}

/// Failures that are not the code's: refused or timed-out tool calls, permission prompts.
const NOT_CODE: &[&str] = &["doesn't want to proceed", "permission", "denied", "timed out", "interrupted", "not allowed", "sandbox"];
const ERROR_WORDS: &[&str] = &["error", "failed", "failure", "exception", "cannot", "could not", "not found", "undefined", "traceback", "panic", "assert"];

/// The line of a failed command's output that names the error, with numbers and paths
/// taken out so the same error matches itself next time. None when it is not the code's.
pub fn code_error(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if lower.contains("<tool_use_error>") || NOT_CODE.iter().any(|w| lower.contains(w)) {
        return None;
    }
    let line = error_line(&lower)?;
    let mut out = String::new();
    for word in line.split_whitespace() {
        let word = if word.contains('/') || word.contains('\\') {
            "<path>".to_string()
        } else {
            let mut cleaned = String::new();
            let mut in_number = false;
            for c in word.chars() {
                if c.is_ascii_digit() {
                    if !in_number {
                        cleaned.push('#');
                    }
                    in_number = true;
                } else {
                    in_number = false;
                    cleaned.push(c);
                }
            }
            cleaned
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&word);
        if out.chars().count() > 120 {
            break;
        }
    }
    Some(out.chars().take(120).collect())
}

/// The line of a command's output that names its error. A Python traceback names it on its
/// last line; anything else, on the first line that says "error", "failed" and the like.
fn error_line(text: &str) -> Option<&str> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.to_lowercase().starts_with("exit code"));
    if text.to_lowercase().contains("traceback (most recent call last)") {
        return lines.next_back();
    }
    lines.find(|l| {
        let lower = l.to_lowercase();
        !lower.starts_with("traceback") && ERROR_WORDS.iter().any(|w| lower.contains(w))
    })
}

fn shorten(text: &str, chars: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= chars {
        return text;
    }
    format!("{}…", text.chars().take(chars).collect::<String>().trim_end())
}

// ------------------------------------------------------------------- verdict

fn dismissed() -> HashSet<String> {
    store::read_json_or_null(&state_file("loops.json"))
        .get("dismissed")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(|k| k.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn key(account: &str, session_id: &str) -> String {
    format!("{account}|{session_id}")
}

/// Mark a session "not a loop", or take that back.
pub fn set_dismissed(account: &str, session_id: &str, on: bool) -> Result<()> {
    let mut list: Vec<String> = dismissed().into_iter().collect();
    let k = key(account, session_id);
    list.retain(|x| *x != k);
    if on {
        list.push(k);
    }
    list.sort();
    store::locked(|| store::write_json(&state_file("loops.json"), &json!({ "dismissed": list }), false))
}

fn fresh(item: &HistoryItem) -> u64 {
    item.own_tokens.input + item.own_tokens.output + item.own_tokens.cache_write
}

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 1.0;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 2].max(1.0)
}

fn clamp(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// For each item, in order: is it judged, its score, whether it is worth a look, and why.
pub fn assess_all(items: &[HistoryItem]) -> Vec<Value> {
    let gone = dismissed();
    let bar = (BAR + BAR_STEP * gone.len() as u32).min(BAR_MAX);
    let measured = |item: &HistoryItem| item.work.replies >= MIN_REPLIES && fresh(item) >= MIN_FRESH_TOKENS && item.work.edits >= MIN_EDITS;
    let change = |item: &HistoryItem| f64::from((item.work.lines_kept + item.work.lines_replaced).max(1));
    let judged: Vec<&HistoryItem> = items.iter().filter(|i| measured(i)).collect();
    let usual_per_line = median(judged.iter().map(|i| fresh(i) as f64 / change(i)).collect());
    let usual_per_prompt = median(judged.iter().map(|i| fresh(i) as f64 / f64::from(i.prompts.max(1))).collect());

    items
        .iter()
        .map(|item| {
            let w = &item.work;
            if !measured(item) {
                let why = if w.edits == 0 {
                    "no edits to the project".to_string()
                } else if w.edits < MIN_EDITS {
                    format!("only {} edits", w.edits)
                } else if w.replies < MIN_REPLIES {
                    format!("only {} replies", w.replies)
                } else {
                    "a small session".to_string()
                };
                return json!({ "measured": false, "why": why });
            }
            let mut reasons = Vec::new();
            let repeat = clamp((f64::from(w.repeats) - 1.0) / 3.0);
            if w.repeats >= 2 {
                reasons.push(format!("The same error came back {} times after fixes: \"{}\"", w.repeats, w.repeat_error.as_deref().unwrap_or("")));
            }
            let stuck = clamp(f64::from(w.still_prompts) / 2.0);
            if w.still_prompts > 0 {
                let said = w.still_example.as_deref().unwrap_or("");
                reasons.push(if w.still_prompts == 1 { format!("You said: \"{said}\"") } else { format!("{} times you said it still wasn't working, like: \"{said}\"", w.still_prompts) });
            }
            let ratio = f64::from(w.lines_written) / f64::from(w.lines_kept.max(1));
            let churn = clamp((ratio - 2.0) / 3.0).max(clamp(f64::from(w.undone) / 3.0));
            if churn >= 0.3 {
                let mut bits = Vec::new();
                if ratio > 2.0 {
                    bits.push(format!("{} lines written, {} kept", w.lines_written, w.lines_kept));
                }
                if w.undone > 0 {
                    bits.push(format!("{} edits put back text taken out earlier", w.undone));
                }
                if let Some(file) = &w.busiest_file {
                    bits.push(format!("{file} edited {} times", w.busiest_edits));
                }
                reasons.push(bits.join(" · "));
            }
            // Spend alone is not a loop (a big job is expensive too): it only adds to the signs above.
            let signs = repeat.max(stuck).max(churn);
            let per_line = fresh(item) as f64 / change(item) / usual_per_line;
            let per_prompt = fresh(item) as f64 / f64::from(item.prompts.max(1)) / usual_per_prompt;
            let (cost, prompt_cost) = if signs >= 0.3 { (clamp((per_line - 2.0) / 6.0), clamp((per_prompt - 2.0) / 4.0)) } else { (0.0, 0.0) };
            if cost >= 0.3 {
                reasons.push(format!("{per_line:.1}× your usual tokens for each line changed"));
            }
            let hits = u32::from(w.tail_errors >= 2) + u32::from(w.tail_edits >= 1 && w.tail_kept <= 5) + u32::from(w.tail_edits >= 3);
            let stall = match hits {
                3 => 1.0,
                2 => 0.5,
                _ => 0.0,
            };
            if stall >= 0.5 {
                reasons.push(format!("In its last {TAIL} replies: {} errors, {} edits, about {} lines kept", w.tail_errors, w.tail_edits, w.tail_kept.max(0)));
            }
            let score = (25.0 * repeat + 25.0 * stuck + 20.0 * churn + 15.0 * cost + 5.0 * prompt_cost + 10.0 * stall).round() as u32;
            let is_dismissed = gone.contains(&key(&item.account, &item.session_id));
            json!({
                "measured": true,
                "score": score,
                "look": score >= bar && !is_dismissed,
                "dismissed": is_dismissed,
                "reasons": reasons,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_code_errors_count_and_match_themselves() {
        let a = code_error("Exit code 1\nsrc/app.ts(12,4): error TS2339: Property 'role' does not exist on type 'User'.").unwrap();
        let b = code_error("Exit code 2\nsrc/app.ts(40,9): error TS2339: Property 'role' does not exist on type 'User'.").unwrap();
        assert_eq!(a, b, "line numbers and paths do not make it a different error");
        assert!(a.contains("property 'role' does not exist"), "{a}");
        assert_eq!(code_error("The user doesn't want to proceed with this tool use."), None);
        assert_eq!(code_error("<tool_use_error>File has not been read yet.</tool_use_error>"), None);
        assert_eq!(code_error("Command timed out after 2m 0s"), None);
        assert_eq!(code_error("Exit code 1"), None, "no error line, nothing to match");
        let key = code_error("Exit code 1\nTraceback (most recent call last):\n  File \"app.py\", line 3\nKeyError: 'name'").unwrap();
        let value = code_error("Exit code 1\nTraceback (most recent call last):\n  File \"app.py\", line 9\nValueError: bad input").unwrap();
        assert_eq!(key, "keyerror: 'name'");
        assert_ne!(key, value, "two different Python errors are not the same error");
    }

    #[test]
    fn stuck_words_are_strict() {
        assert!(sounds_stuck("still facing the issue, investigate it"));
        assert!(sounds_stuck("It’s still not working"));
        assert!(sounds_stuck("that didn't work"));
        assert!(!sounds_stuck("scroll is not working in the settings dialog, fix it"), "a new problem, not a failed fix");
        assert!(!sounds_stuck("This session is being continued from a previous conversation. Still facing the issue."));
        assert!(sounds_stuck("same error as before"));
        assert!(!sounds_stuck("keep this look like this"));
        assert!(!sounds_stuck("try to access it again"));
        assert!(!sounds_stuck("is it still running?"));
        assert!(!sounds_stuck("are there no changes in the crm repo?"), "a question, not a complaint");
        assert!(!sounds_stuck("there should be no change after a service is moved"));
    }

    fn tool_use(id: &str, name: &str, input: Value) -> Value {
        json!({ "message": { "content": [{ "type": "tool_use", "id": id, "name": name, "input": input }] } })
    }

    fn failed(id: &str, text: &str) -> Value {
        json!({ "message": { "content": [{ "type": "tool_result", "tool_use_id": id, "is_error": true, "content": text }] } })
    }

    #[test]
    fn a_loop_leaves_its_marks() {
        let mut g = Gatherer::default();
        g.prompt("make the login work");
        for round in 0..4 {
            g.reply();
            // Flip one line back and forth, then run the tests, which fail the same way.
            let (old, new) = if round % 2 == 0 { ("let a = 1;", "let a = 2;") } else { ("let a = 2;", "let a = 1;") };
            g.assistant(&tool_use(&format!("e{round}"), "Edit", json!({ "file_path": "/w/src/login.ts", "old_string": old, "new_string": new })));
            g.assistant(&tool_use(&format!("b{round}"), "Bash", json!({ "command": "npm test" })));
            g.results(&failed(&format!("b{round}"), &format!("Exit code 1\nFAIL src/login.test.ts:{}\nError: expected 200, got 401", 10 + round)));
        }
        g.prompt("still not working, same error");
        g.assistant(&tool_use("plan", "Write", json!({ "file_path": "/home/me/.claude/plans/x.md", "content": "a\nb" })));
        let w = g.finish();
        assert_eq!(w.edits, 4, "the plan file is not the project");
        assert_eq!(w.repeats, 3, "back three times after an edit: {w:?}");
        assert_eq!(w.repeat_error.as_deref(), Some("Error: expected 200, got 401"), "shown as Claude saw it: {w:?}");
        assert_eq!(w.undone, 3, "rounds 2 to 4 each put back what the one before took out: {w:?}");
        assert_eq!(w.still_prompts, 1);
        assert_eq!(w.busiest_file.as_deref(), Some("login.ts"));
        assert_eq!((w.lines_written, w.lines_kept), (4, 1), "{w:?}");
    }
}
