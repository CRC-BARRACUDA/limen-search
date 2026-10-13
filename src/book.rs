//! Dictionaries that are kept.
//!
//! The dictionary is the part of a search worth keeping. Paths change with the
//! machine and dates change with the case, but the list of things worth looking
//! for — the filenames a piece of malware drops, what a leak looks like, the
//! extensions that should never be on a share — is written once and used for
//! years, by everybody.
//!
//! So it can be **saved** under a name and picked from a list, or **exported**
//! as a plain text file: one term per line, which is a dictionary already. A
//! text file is what somebody pastes into a ticket, keeps in a repository, or
//! writes without this module at all.

use crate::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a kept dictionary is called and what is in it.
#[derive(Debug, Default)]
pub(crate) struct Book {
    /// By name, in name order — a list somebody reads, so it is sorted the way
    /// a list is read rather than the order things happened to be saved.
    pub dictionaries: BTreeMap<String, String>,
}

/// What the file says it is. Anything else is not one of ours, and is left
/// alone rather than overwritten.
const MARK: &str = "limen.search.dictionaries";
const VERSION: u64 = 1;

impl Book {
    /// What is on disk, or an empty book. A book that cannot be read is an
    /// empty one: a broken file should not stop somebody searching.
    pub fn load(dir: &str) -> Self {
        let mut book = Book::default();
        let Ok(text) = std::fs::read_to_string(path_in(dir)) else {
            return book;
        };
        let Ok(doc) = text.parse::<Value>() else {
            return book;
        };
        if doc.get("limen").and_then(Value::as_str) != Some(MARK) {
            return book;
        }
        if let Some(map) = doc.get("dictionaries").and_then(Value::as_object) {
            for (name, terms) in map {
                if let Some(terms) = terms.as_str() {
                    book.dictionaries.insert(name.clone(), terms.to_string());
                }
            }
        }
        book
    }

    pub fn save(&self, dir: &str) -> Result<(), String> {
        let map: serde_json::Map<String, Value> = self
            .dictionaries
            .iter()
            .map(|(name, terms)| (name.clone(), Value::String(terms.clone())))
            .collect();
        let doc = json!({
            "limen": MARK,
            "version": VERSION,
            "dictionaries": Value::Object(map),
        });
        write_json(&path_in(dir), &doc)
    }

    pub fn names(&self) -> Vec<String> {
        self.dictionaries.keys().cloned().collect()
    }
}

fn path_in(dir: &str) -> PathBuf {
    Path::new(dir).join("dictionaries.json")
}

fn write_json(path: &Path, doc: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, text + "\n").map_err(|e| e.to_string())
}

/// A dictionary as a file to send somebody: the terms, and a first line saying
/// what it is.
///
/// Plain text, because a dictionary already is: one term per line. It can be
/// pasted into a ticket, kept beside the code it is about, written in a text
/// editor by somebody who has never run this module, and read back by it
/// without ceremony.
pub(crate) fn to_text(name: &str, terms: &str) -> String {
    let head = if name.trim().is_empty() {
        "# Limen search dictionary".to_string()
    } else {
        format!("# Limen search dictionary: {}", name.trim())
    };
    format!("{head}\n{}\n", terms.trim_end())
}

/// And back. Every text file is a dictionary — notes and blank lines are
/// already part of the format — so the only thing read out of it is the name
/// this module wrote, if it is the one that wrote it.
pub(crate) fn from_text(text: &str) -> (Option<String>, String) {
    let name = text
        .lines()
        .next()
        .and_then(|l| l.trim().strip_prefix("# Limen search dictionary:"))
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    (name, text.to_string())
}

/// A name made safe for a filename, without being flattened: a dictionary
/// called "Exfil — Q3" keeps its shape.
pub(crate) fn file_stem(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
            gap = false;
        } else if !gap && !out.is_empty() {
            out.push('_');
            gap = true;
        }
    }
    let trimmed = out.trim_end_matches('_').to_string();
    if trimmed.is_empty() {
        "dictionary".to_string()
    } else {
        trimmed
    }
}

/// How many terms a dictionary holds, for the line that says what is in it.
/// Notes and blank lines are not terms.
pub(crate) fn count(terms: &str) -> usize {
    terms
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count()
}
