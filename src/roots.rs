//! The places a search starts from.
//!
//! A person names the paths they care about, and names them the way they think
//! of them — so the same tree arrives twice, or a folder arrives beside its own
//! parent. Walking both would read every file under the child twice, report it
//! twice, and take longer doing it.

use std::path::{Path, PathBuf};

/// What came of the paths that were asked for.
#[derive(Debug, Default)]
pub(crate) struct Roots {
    /// The ones to walk, in the order they were given.
    pub keep: Vec<PathBuf>,
    /// Dropped, with the path that already covers each: `D:/dev/projects/p1`
    /// covered by `D:/dev/projects`. Reported rather than silently removed —
    /// somebody who listed a path means to have it searched, and should be told
    /// where it went.
    pub covered: Vec<(PathBuf, PathBuf)>,
    /// Named but not there, or not readable as a directory.
    pub missing: Vec<PathBuf>,
}

/// Resolve what the user typed, then prune it.
pub(crate) fn resolve(given: &[String]) -> Roots {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for line in given {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Canonical, so that `.`, `..`, a trailing slash and a symlink all land
        // on the same path — otherwise two spellings of one directory look like
        // two directories and neither covers the other.
        match std::fs::canonicalize(line) {
            Ok(p) if p.is_dir() => found.push(tidy(p)),
            _ => missing.push(PathBuf::from(line)),
        }
    }
    let mut roots = prune(found);
    roots.missing = missing;
    roots
}

/// Drop every path that another already contains, and every repeat.
///
/// Compared by path component, never as text: `/a/bc` is not inside `/a/b`,
/// though one spelling starts with the other.
pub(crate) fn prune(given: Vec<PathBuf>) -> Roots {
    // Shortest first, so a parent is always seen before its children; ties by
    // name, so the answer does not depend on the order they were typed in.
    let mut sorted: Vec<PathBuf> = given.clone();
    sorted.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });

    let mut keep: Vec<PathBuf> = Vec::new();
    let mut covered: Vec<(PathBuf, PathBuf)> = Vec::new();
    for path in sorted {
        match keep.iter().find(|k| covers(k, &path)) {
            Some(by) => covered.push((path, by.clone())),
            None => keep.push(path),
        }
    }

    // Back into the order they were asked for: the screen lists them as the
    // person wrote them.
    keep.sort_by_key(|p| given.iter().position(|g| g == p).unwrap_or(usize::MAX));
    Roots {
        keep,
        covered,
        missing: Vec::new(),
    }
}

/// Whether everything under `child` is already under `parent` — including when
/// they are the same path, which is how a repeat is caught.
fn covers(parent: &Path, child: &Path) -> bool {
    child.starts_with(parent)
}

/// A canonical path as somebody would write it down.
///
/// [`std::fs::canonicalize`] answers in Windows' extended-length form, the one
/// that starts `\\?\` — the right thing to hand to the filesystem and the
/// wrong thing to put in front of a person: it is what every result, every
/// saved report and every path in the list would otherwise be headed with.
///
/// Taking the prefix off costs nothing. Rust's own file calls put it back for
/// any path long enough to need it, so a six-hundred-character path still
/// opens; and the verbatim form is the one spelling that refuses a forward
/// slash, which is how a good many people type a drive.
pub(crate) fn tidy(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    let text = path.to_string_lossy();
    let Some(rest) = text.strip_prefix(VERBATIM) else {
        return path;
    };
    // The verbatim spelling of a share names UNC first; written out, it is the
    // two leading slashes everybody else writes.
    match rest.strip_prefix("UNC\\") {
        Some(share) => PathBuf::from(format!("\\\\{share}")),
        None => PathBuf::from(rest),
    }
}

/// What Windows puts in front of an extended-length path.
const VERBATIM: &str = "\\\\?\\";
