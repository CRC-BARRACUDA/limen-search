//! What narrows a search beyond its words: what kind of thing, how big, and
//! when it was made or last changed.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// What a found thing is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Dir,
    /// A file whose extension says it holds other files. Its own kind because
    /// it is what somebody looking for "everything that could be carrying data
    /// out" is asking for.
    Archive,
    File,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Dir => "dir",
            Kind::Archive => "archive",
            Kind::File => "file",
        }
    }
}

/// The extensions counted as an archive.
///
/// By name, because the alternative is opening every file to find out — which
/// on a whole disk is the search taking all afternoon.
const ARCHIVES: [&str; 18] = [
    "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz", "xz", "zst", "iso", "jar", "war", "ear",
    "cab", "dmg", "pkg", "apk",
];

/// Whether this extension is one of the archive kinds.
pub(crate) fn is_archive_ext(ext: &str) -> bool {
    ARCHIVES.contains(&ext)
}

pub(crate) fn kind_of(is_dir: bool, name: &str) -> Kind {
    if is_dir {
        return Kind::Dir;
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase());
    match ext {
        Some(e) if ARCHIVES.contains(&e.as_str()) => Kind::Archive,
        _ => Kind::File,
    }
}

/// Everything a search narrows by, once the name has matched.
#[derive(Debug, Default, Clone)]
pub(crate) struct Filters {
    /// Empty means every kind.
    pub kinds: Vec<Kind>,
    pub created_after: Option<SystemTime>,
    pub created_before: Option<SystemTime>,
    pub changed_after: Option<SystemTime>,
    pub changed_before: Option<SystemTime>,
    pub smaller_than: Option<u64>,
    pub larger_than: Option<u64>,
}

impl Filters {
    /// Whether this is worth reporting. Every filter has to pass — each one is
    /// a thing the person said they wanted.
    pub fn keeps(&self, kind: Kind, size: u64, created: Option<SystemTime>, changed: Option<SystemTime>) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&kind) {
            return false;
        }
        if kind != Kind::Dir {
            if self.larger_than.is_some_and(|m| size < m) {
                return false;
            }
            if self.smaller_than.is_some_and(|m| size > m) {
                return false;
            }
        }
        // A filesystem that does not record a creation time cannot answer a
        // question about one. Excluded rather than assumed: saying "made after
        // Tuesday" about a time nobody knows is an answer made up.
        if !within(created, self.created_after, self.created_before) {
            return false;
        }
        within(changed, self.changed_after, self.changed_before)
    }

    /// Whether answering this needs the entry's metadata — its size, or either
    /// of its times. The kind does not: that is read off the name and whether
    /// the entry is a directory.
    pub fn needs_metadata(&self) -> bool {
        self.created_after.is_some()
            || self.created_before.is_some()
            || self.changed_after.is_some()
            || self.changed_before.is_some()
            || self.smaller_than.is_some()
            || self.larger_than.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
            && self.created_after.is_none()
            && self.created_before.is_none()
            && self.changed_after.is_none()
            && self.changed_before.is_none()
            && self.smaller_than.is_none()
            && self.larger_than.is_none()
    }
}

fn within(when: Option<SystemTime>, after: Option<SystemTime>, before: Option<SystemTime>) -> bool {
    if after.is_none() && before.is_none() {
        return true;
    }
    let Some(when) = when else {
        return false;
    };
    after.is_none_or(|a| when >= a) && before.is_none_or(|b| when <= b)
}

/// A date as the date field sends it — `2024-01-31`, or with a time after it.
///
/// `end` moves a bare date to the last moment of that day, so "before the 31st"
/// includes the 31st. A reader means the day, not midnight at the start of it.
pub(crate) fn date(value: &str, end: bool) -> Option<SystemTime> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (day, time) = match value.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (value, None),
    };
    let mut parts = day.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut secs = days_from_civil(y, m, d) * 86_400;
    match time {
        Some(t) => {
            let mut hms = t.split(':');
            let h: i64 = hms.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let mi: i64 = hms.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let s: i64 = hms.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            secs += h * 3600 + mi * 60 + s;
        }
        None if end => secs += 86_400 - 1,
        None => {}
    }
    if secs < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(secs as u64))
}

/// Days from 1970-01-01 to this date, by Howard Hinnant's civil calendar
/// algorithm — the one every date library is built on, and short enough to
/// carry rather than depend on.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A size as somebody writes one: `500`, `10k`, `2.5 MB`, `1gb`.
pub(crate) fn size(value: &str) -> Option<u64> {
    let v = value.trim().to_lowercase();
    if v.is_empty() {
        return None;
    }
    let digits: String = v
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let number: f64 = digits.parse().ok()?;
    let unit: String = v[digits.len()..].trim().replace('b', "");
    let scale: f64 = match unit.as_str() {
        "" => 1.0,
        "k" => 1024.0,
        "m" => 1024.0 * 1024.0,
        "g" => 1024.0 * 1024.0 * 1024.0,
        "t" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((number * scale) as u64)
}
