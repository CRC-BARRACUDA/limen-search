//! The dictionary: what a search is looking for.
//!
//! One term per line, because that is how a list of things to look for is
//! written down — pasted from a ticket, kept in a file, added to over a case.
//! The three kinds are told apart by how they are written rather than by a
//! dropdown somewhere else:
//!
//! ```text
//! invoice              a word     — anywhere in the name, any case
//! "annual report"      a phrase   — spaces and all, any case
//! /^IMG_\d{4}\.jpe?g$/ a pattern  — a regular expression
//! # a line like this is a note to whoever reads the dictionary
//! ```
//!
//! A dictionary with nothing in it matches every name, so a search can be made
//! of dates and kinds alone.

use regex::RegexBuilder;

/// One thing to look for, and the line it was written on.
///
/// The line is kept because a result has to be able to say what found it: a
/// search of twenty terms over a disk answers with a list, and "why is this
/// here" is the first question anybody asks of it.
#[derive(Debug)]
pub(crate) struct Term {
    pub written: String,
    how: How,
}

#[derive(Debug)]
enum How {
    /// Anywhere in the name, ignoring case.
    Word(String),
    /// The same, but written with spaces in it.
    Phrase(String),
    /// A regular expression, matched against the whole name.
    Pattern(regex::Regex),
}

impl Term {
    fn hits(&self, lower: &str, name: &str) -> bool {
        match &self.how {
            How::Word(w) | How::Phrase(w) => lower.contains(w),
            How::Pattern(re) => re.is_match(name),
        }
    }
}

/// The terms, and whatever could not be read as one.
#[derive(Debug, Default)]
pub(crate) struct Dictionary {
    pub terms: Vec<Term>,
    /// A line that was meant to be a pattern and is not one, with what the
    /// regex engine said about it. Kept rather than dropped: a dictionary that
    /// silently ignores a line searches for less than it was asked to, and says
    /// nothing about it.
    pub rejected: Vec<(String, String)>,
}

impl Dictionary {
    /// Whether this name is one of the things being looked for.
    ///
    /// Any term is enough — a dictionary is a list of things worth finding, not
    /// a description of one thing.
    pub fn hits(&self, name: &str) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        let lower = name.to_lowercase();
        self.terms.iter().any(|t| t.hits(&lower, name))
    }

    /// Which terms found this name, as they are written in the dictionary.
    ///
    /// Every one of them, not the first: a name that answers to three terms is
    /// worth knowing about, and a reader who sees only one of them cannot tell
    /// a lucky hit from a thing their whole dictionary agrees on. Asked only of
    /// names that have already matched, so the second pass over the terms costs
    /// what the results cost rather than what the disk does.
    pub fn why(&self, name: &str) -> Vec<&str> {
        let lower = name.to_lowercase();
        self.terms
            .iter()
            .filter(|t| t.hits(&lower, name))
            .map(|t| t.written.as_str())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// How many terms there are — what "every term has matched" counts up to.
    pub fn len(&self) -> usize {
        self.terms.len()
    }
}

/// Read a dictionary, a line at a time.
pub(crate) fn parse(text: &str) -> Dictionary {
    let mut dict = Dictionary::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // A pattern is written the way every tool writes one, between slashes;
        // anything shorter than `//` is two slashes and no pattern.
        if line.len() > 2 && line.starts_with('/') && line.ends_with('/') {
            let body = &line[1..line.len() - 1];
            match RegexBuilder::new(body).case_insensitive(true).build() {
                Ok(re) => dict.terms.push(Term {
                    written: line.to_string(),
                    how: How::Pattern(re),
                }),
                Err(e) => dict
                    .rejected
                    .push((line.to_string(), first_line(&e.to_string()))),
            }
            continue;
        }
        if line.len() > 1 && line.starts_with('"') && line.ends_with('"') {
            let body = line[1..line.len() - 1].trim().to_lowercase();
            if !body.is_empty() {
                dict.terms.push(Term {
                    written: line.to_string(),
                    how: How::Phrase(body),
                });
            }
            continue;
        }
        dict.terms.push(Term {
            written: line.to_string(),
            how: How::Word(line.to_lowercase()),
        });
    }
    dict
}

/// The regex crate explains a bad pattern over several lines with a diagram.
/// The screen has room for the sentence.
fn first_line(err: &str) -> String {
    err.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('^') && !l.contains("regex parse error"))
        .unwrap_or("not a valid pattern")
        .to_string()
}

/// How a line is written, as one word: a `word`, a `"phrase"`, a `/pattern/`,
/// or a `# note`.
///
/// Used by the screen to say what each line of the dictionary is, so the
/// writing is not the only thing that tells them apart.
pub(crate) fn kind_of(line: &str) -> &'static str {
    let line = line.trim();
    if line.starts_with('#') {
        "note"
    } else if line.len() > 2 && line.starts_with('/') && line.ends_with('/') {
        "pattern"
    } else if line.len() > 1 && line.starts_with('"') && line.ends_with('"') {
        "phrase"
    } else {
        "word"
    }
}

/// The term itself, without the punctuation that says what kind it is.
pub(crate) fn reading(line: &str) -> String {
    let line = line.trim();
    match kind_of(line) {
        "note" => line.trim_start_matches('#').trim().to_string(),
        "pattern" | "phrase" => line[1..line.len() - 1].to_string(),
        _ => line.to_string(),
    }
}

/// Write what somebody typed as a line of the dictionary, or say why it cannot
/// be one.
///
/// A pattern is checked here rather than at search time: a regular expression
/// with a bracket missing should be refused while it is being typed, not three
/// minutes into a walk of the disk.
pub(crate) fn write(text: &str, pattern: bool) -> Result<String, String> {
    let text = text.trim();
    if pattern {
        // Slashes are how a pattern is written, so somebody who typed them
        // meant the pattern between them rather than two more characters to
        // match.
        let body = if text.len() > 2 && text.starts_with('/') && text.ends_with('/') {
            &text[1..text.len() - 1]
        } else {
            text
        };
        return match RegexBuilder::new(body).case_insensitive(true).build() {
            Ok(_) => Ok(format!("/{body}/")),
            Err(e) => Err(first_line(&e.to_string())),
        };
    }
    // Quoted when the bare text would be read as something else: a phrase has
    // spaces in it, and a word starting with #, " or / would come back as a
    // note or a pattern.
    let quoted = text.contains(char::is_whitespace)
        || text.starts_with('#')
        || text.starts_with('"')
        || text.starts_with('/');
    Ok(if quoted {
        format!("\"{}\"", text.trim_matches('"'))
    } else {
        text.to_string()
    })
}
