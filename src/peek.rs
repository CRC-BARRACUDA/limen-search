//! Looking inside a file.
//!
//! The walk matches names, which costs a `readdir` and a `stat`. Looking inside
//! costs an open, a read and a decode — hundreds of times more — so it is done
//! second, to the files the names, kinds, sizes and dates have already left
//! standing, and never to a file nobody asked a question about: with an empty
//! dictionary there is nothing to look for, and nothing is opened.
//!
//! Everything here reads files from the machine under investigation, which is
//! to say deliberately hostile input. Every limit below is there because the
//! alternative is a module that can be made to hang, fill memory, or take its
//! own tab down with it.

use crate::dict::Dictionary;
use crate::filter::Kind;
use std::io::{BufRead, Read};
use std::path::Path;

/// The largest file this will hold **in memory** at once.
///
/// Some formats cannot be read any other way: a zip member has to be
/// decompressed somewhere, a PDF is parsed as a whole. Plain text is not among
/// them — it is read a line at a time and never held — so this is the ceiling
/// for the formats that must buffer, and it is far lower than it was because
/// every thread was taking one of these at the same time.
pub(crate) const MAX_FILE: u64 = 16 << 20;

/// How much every thread *together* may hold in memory at once.
///
/// The ceiling above is per file; sixteen threads each holding one is sixteen
/// times that, and that is what filled the machine. This is the real limit: a
/// thread that cannot have its share waits for one, and the work goes on at
/// whatever width the memory allows rather than at whatever width the disk
/// does.
const IN_FLIGHT: u64 = 192 << 20;

/// How much of one file is read before the search moves on.
///
/// Not a memory limit — streamed text is never held — but a time one: a
/// hundred-gigabyte log is not something to read to the end of in order to say
/// whether a word is in the first page of it.
const MAX_SCAN: u64 = 512 << 20;

/// A claim on [`IN_FLIGHT`], released when it goes out of scope.
struct Room(u64);

impl Room {
    /// Wait for enough of the budget to be free, then take it.
    ///
    /// `None` when the file is larger than the whole budget — nothing can be
    /// waited for that would make it fit, and the caller counts it as too
    /// large rather than blocking for ever.
    fn take(bytes: u64) -> Option<Self> {
        if bytes > IN_FLIGHT {
            return None;
        }
        let (lock, free) = budget();
        let mut left = lock.lock().unwrap_or_else(|e| e.into_inner());
        while *left < bytes {
            left = free.wait(left).unwrap_or_else(|e| e.into_inner());
        }
        *left -= bytes;
        Some(Room(bytes))
    }
}

impl Drop for Room {
    fn drop(&mut self) {
        let (lock, free) = budget();
        let mut left = lock.lock().unwrap_or_else(|e| e.into_inner());
        *left += self.0;
        // One waiter is enough to hand it to, but any of them may be waiting
        // for more than this returns, so they all get to look.
        free.notify_all();
    }
}

/// The budget itself, and a share of it — for the test that holds several at
/// once, which cannot be written from outside this module.
#[cfg(test)]
pub(crate) fn budget_for_test() -> u64 {
    IN_FLIGHT
}

#[cfg(test)]
pub(crate) fn room_for_test(bytes: u64) -> Option<impl Drop> {
    Room::take(bytes)
}

fn budget() -> &'static (std::sync::Mutex<u64>, std::sync::Condvar) {
    static BUDGET: std::sync::OnceLock<(std::sync::Mutex<u64>, std::sync::Condvar)> =
        std::sync::OnceLock::new();
    BUDGET.get_or_init(|| (std::sync::Mutex::new(IN_FLIGHT), std::sync::Condvar::new()))
}

/// How much of a file is looked at to decide whether it is text at all.
const SNIFF: usize = 8 << 10;

/// How much of the matching line is kept to show. A minified file is one line
/// of two megabytes, and the table has room for a sentence.
const LINE_CAP: usize = 200;

/// What one archive may be made to give up, however small it is on disk. A zip
/// bomb is a few kilobytes that decompresses to terabytes.
const ZIP_MAX_BYTES: u64 = 256 << 20;
const ZIP_MAX_MEMBERS: usize = 10_000;

/// Which of the two kinds of looking the search was asked for.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Look {
    /// Read what files say — plain text, and the text of Office documents.
    pub contents: bool,
    /// Read what archives hold. Member *names* on their own; member contents
    /// as well when `contents` is set.
    pub archives: bool,
}

impl Look {
    pub fn asked(&self) -> bool {
        self.contents || self.archives
    }
}

/// One thing found inside a file.
#[derive(Debug, Clone)]
pub(crate) struct Inside {
    /// The dictionary term that found it, as the dictionary writes it.
    pub term: String,
    /// Empty when it was the file's own text; otherwise the member of the
    /// archive or document it was in.
    pub member: String,
    /// Which line, counted from one. Zero when it was a member's name rather
    /// than anything written on a line.
    pub line: usize,
    /// The line itself, trimmed and cut to something a table can show.
    pub text: String,
}

/// What came of looking inside one file.
#[derive(Debug, Default)]
pub(crate) struct Looked {
    pub found: Vec<Inside>,
    /// It was opened and read to the end, or to a limit.
    pub read: bool,
    /// It was opened and turned out not to be text.
    pub binary: bool,
    /// It was left alone for being larger than [`MAX_FILE`].
    pub too_big: bool,
    /// It could not be read or could not be parsed: no permission, gone since
    /// the listing, encrypted, corrupt, or a parser that gave up.
    pub failed: bool,
}

/// Look inside one file for anything the dictionary is after.
///
/// Never panics, whatever the file does to the parser reading it: a module that
/// panics takes its tab down, and the file deciding when that happens is the
/// file the search was run to find.
pub(crate) fn look(path: &Path, size: u64, kind: Kind, dict: &Dictionary, how: Look) -> Looked {
    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        inspect(path, size, kind, dict, how)
    }));
    done.unwrap_or_else(|_| Looked {
        failed: true,
        ..Default::default()
    })
}

fn inspect(path: &Path, size: u64, kind: Kind, dict: &Dictionary, how: Look) -> Looked {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_string()).unwrap_or_default();
    let _ = kind;

    match shape(&name, &ext) {
        // Archives: opened only when the search asked for archives, because
        // what they hold is a different question from what a file says.
        Shape::Zip => {
            if how.archives {
                in_zip(path, dict, how.contents)
            } else {
                Looked::default()
            }
        }
        Shape::Tar(packed) => {
            if how.archives {
                in_tar(path, packed, dict, how.contents)
            } else {
                Looked::default()
            }
        }
        Shape::SevenZ => {
            if how.archives && how.contents {
                // 7-zip keeps its directory compressed with the rest, so there
                // is no listing to read without unpacking: names and contents
                // come together or not at all.
                in_7z(path, dict)
            } else {
                Looked::default()
            }
        }
        Shape::Gz => {
            if how.archives && how.contents {
                in_gz(path, dict)
            } else {
                Looked::default()
            }
        }
        // Documents: these are what a file *says*, whatever they are made of
        // underneath, so they follow the contents box rather than the archive
        // one.
        _ if !how.contents => Looked::default(),
        Shape::Office(part) => in_office(path, dict, part),
        Shape::Pdf => in_pdf(path, size, dict),
        Shape::Ole => in_ole(path, dict),
        Shape::Plain => stream(path, size, dict),
    }
}

/// Read a text file a line at a time, holding none of it.
///
/// It used to be read whole and then copied again by the decoder — sixty-four
/// megabytes in flight for a thirty-two megabyte file, on every thread at once,
/// which is what filled the machine. A line is all that has to be in memory to
/// answer whether a line matches.
fn stream(path: &Path, size: u64, dict: &Dictionary) -> Looked {
    let Ok(file) = std::fs::File::open(path) else {
        return Looked {
            failed: true,
            ..Default::default()
        };
    };
    let mut reader = std::io::BufReader::with_capacity(64 << 10, file.take(MAX_SCAN));

    // Enough of the front to tell text from not-text, and to read a
    // byte-order mark if there is one.
    let head = match reader.fill_buf() {
        Ok(head) => head.to_vec(),
        Err(_) => {
            return Looked {
                failed: true,
                ..Default::default()
            }
        }
    };
    if head.is_empty() {
        return Looked {
            read: true,
            ..Default::default()
        };
    }
    // UTF-16 is half zero bytes, so it cannot be streamed as lines of bytes
    // and cannot be told from binary without its mark. It is read whole, out
    // of the shared budget, and it is rare.
    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        return buffered(path, size, dict);
    }
    if head[..head.len().min(SNIFF)].contains(&0) {
        return Looked {
            binary: true,
            ..Default::default()
        };
    }

    let mut out = Looked {
        read: true,
        ..Default::default()
    };
    let mut line = Vec::with_capacity(256);
    let mut n = 0usize;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => {
                out.failed = true;
                break;
            }
        }
        n += 1;
        // A minified file is one line of several megabytes; what is kept of a
        // match is two hundred characters of it either way.
        if line.len() > 1 << 20 {
            line.truncate(1 << 20);
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\n', '\r']);
        for term in dict.why(text) {
            remember(&mut out.found, term, "", n, text);
        }
        if out.found.len() >= dict.len() {
            break;
        }
    }
    out
}

/// Read a file whole, out of the shared budget. For the formats that cannot
/// be read any other way.
fn buffered(path: &Path, size: u64, dict: &Dictionary) -> Looked {
    if size > MAX_FILE {
        return Looked {
            too_big: true,
            ..Default::default()
        };
    }
    let Some(_room) = Room::take(size.max(1)) else {
        return Looked {
            too_big: true,
            ..Default::default()
        };
    };
    match std::fs::read(path) {
        Ok(bytes) => match decode(&bytes) {
            Some(text) => {
                let mut found = Vec::new();
                scan(&text, "", dict, &mut found);
                Looked {
                    found,
                    read: true,
                    ..Default::default()
                }
            }
            None => Looked {
                binary: true,
                ..Default::default()
            },
        },
        // Permission denied, gone since the listing, a device that will not
        // read: counted, and on to the next file.
        Err(_) => Looked {
            failed: true,
            ..Default::default()
        },
    }
}

/// What a file is made of, as far as reading it is concerned.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    /// A zip, or one of the many things that is a zip with a different name.
    Zip,
    /// A tar, plain or gzipped.
    Tar(bool),
    /// One gzipped file.
    Gz,
    SevenZ,
    /// A document made of zipped XML: which family tells us where its words
    /// are kept.
    Office(Part),
    Pdf,
    /// Word, Excel or PowerPoint as they were written before 2007: one
    /// compound file with the text in streams inside it.
    Ole,
    Plain,
}

/// Which family of zipped-XML document, and so which members hold the words.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Part {
    Word,
    Sheet,
    Slides,
    /// OpenDocument — one `content.xml` whatever the kind.
    Open,
}

fn shape(name: &str, ext: &str) -> Shape {
    match ext {
        // Everything Java, Android and the rest ship is a zip with its own
        // name on it, and reading one is reading a zip.
        "zip" | "jar" | "war" | "ear" | "apk" | "aar" | "xpi" | "crx" | "epub" | "whl"
        | "nupkg" => Shape::Zip,
        "docx" | "docm" | "dotx" => Shape::Office(Part::Word),
        "xlsx" | "xlsm" | "xltx" => Shape::Office(Part::Sheet),
        "pptx" | "pptm" | "potx" => Shape::Office(Part::Slides),
        "odt" | "ods" | "odp" | "odg" | "odf" => Shape::Office(Part::Open),
        "doc" | "xls" | "ppt" | "msg" => Shape::Ole,
        "pdf" => Shape::Pdf,
        "tar" => Shape::Tar(false),
        "tgz" => Shape::Tar(true),
        // `.gz` is one compressed file — unless the name underneath still ends
        // in `.tar`, which is how nearly every one of them is written.
        "gz" => {
            if name.trim_end_matches(".gz").ends_with(".tar") {
                Shape::Tar(true)
            } else {
                Shape::Gz
            }
        }
        "7z" => Shape::SevenZ,
        _ => Shape::Plain,
    }
}

/// Run the dictionary over some text, a line at a time.
///
/// A line at a time because that is what a pattern is written against: the
/// dictionary's `/^IMG_\\d{4}$/` anchors to a name, and inside a file the thing
/// it should anchor to is a line. Matching against the whole file would make
/// `^` mean "the first byte of the file", which is nobody's intention.
///
/// Stops as soon as every term has been seen: the question is which terms found
/// this file, and the second time a term matches adds nothing to it.
fn scan(text: &str, member: &str, dict: &Dictionary, out: &mut Vec<Inside>) {
    if dict.is_empty() {
        return;
    }
    for (n, line) in text.lines().enumerate() {
        for term in dict.why(line) {
            remember(out, term, member, n + 1, line);
        }
        if out.iter().filter(|i| i.member == member).count() >= dict.len() {
            return;
        }
    }
}

/// Keep a match, unless this term has already been found in this member.
///
/// A term that matched a member's *name* is kept with nothing to show;
/// finding it written inside as well says more, so it takes the place of the
/// bare mention.
fn remember(out: &mut Vec<Inside>, term: &str, member: &str, line: usize, text: &str) {
    let found = Inside {
        term: term.to_string(),
        member: member.to_string(),
        line,
        text: cut(text),
    };
    match out.iter().position(|i| i.term == term && i.member == member) {
        Some(at) if out[at].line == 0 => out[at] = found,
        Some(_) => {}
        None => out.push(found),
    }
}

/// A line as a table can show it.
fn cut(line: &str) -> String {
    let line = line.trim();
    if line.chars().count() <= LINE_CAP {
        return line.to_string();
    }
    let short: String = line.chars().take(LINE_CAP).collect();
    format!("{short}…")
}

/// Bytes as text, or `None` when they are not text at all.
///
/// A NUL byte in the first few kilobytes means binary: it is what every other
/// tool uses, it is right nearly always, and the case it is wrong about —
/// UTF-16, which is half NULs — is caught first by its byte-order mark, because
/// a Windows machine is full of UTF-16 text files.
fn decode(bytes: &[u8]) -> Option<String> {
    if let Some(text) = utf16(bytes) {
        return Some(text);
    }
    let head = &bytes[..bytes.len().min(SNIFF)];
    if head.contains(&0) {
        return None;
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn utf16(bytes: &[u8]) -> Option<String> {
    let (rest, big) = match bytes {
        [0xFF, 0xFE, rest @ ..] => (rest, false),
        [0xFE, 0xFF, rest @ ..] => (rest, true),
        _ => return None,
    };
    let units: Vec<u16> = rest
        .chunks_exact(2)
        .map(|p| {
            if big {
                u16::from_be_bytes([p[0], p[1]])
            } else {
                u16::from_le_bytes([p[0], p[1]])
            }
        })
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Look through an archive: what it holds, and — when the search was asked for
/// contents too — what those members say.
fn in_zip(path: &Path, dict: &Dictionary, contents: bool) -> Looked {
    let Ok(file) = std::fs::File::open(path) else {
        return Looked {
            failed: true,
            ..Default::default()
        };
    };
    let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        // Not a zip after all, or a damaged one. Counted; the walk goes on.
        return Looked {
            failed: true,
            ..Default::default()
        };
    };

    let mut out = Looked {
        read: true,
        ..Default::default()
    };
    let mut budget = ZIP_MAX_BYTES;
    for i in 0..zip.len().min(ZIP_MAX_MEMBERS) {
        let Ok(member) = zip.by_index(i) else {
            // An encrypted or broken member. The rest of the archive is still
            // worth looking at.
            out.failed = true;
            continue;
        };
        if member.is_dir() {
            continue;
        }
        let name = member.name().to_string();
        // The name on its own is worth the look: what is in an archive is
        // usually the question, and reading the directory costs nothing.
        let short = name.rsplit('/').next().unwrap_or(&name).to_string();
        for term in dict.why(&short) {
            out.found.push(Inside {
                term: term.to_string(),
                member: name.clone(),
                line: 0,
                text: String::new(),
            });
        }
        if !contents {
            continue;
        }
        let size = member.size();
        // Nested archives are not opened: one level, so that an archive of
        // archives cannot be used to make this walk arbitrarily deep.
        if size > MAX_FILE || size > budget || is_archive(&name) {
            if size > MAX_FILE {
                out.too_big = true;
            }
            continue;
        }
        budget = budget.saturating_sub(size);
        // A share of what every thread together may hold. Without it, sixteen
        // threads each unpacking a member of their own was sixteen times this
        // much in memory at the same moment.
        let Some(_room) = Room::take(size.max(1)) else {
            out.too_big = true;
            continue;
        };
        let mut bytes = Vec::with_capacity(size.min(MAX_FILE) as usize);
        if member.take(MAX_FILE).read_to_end(&mut bytes).is_err() {
            out.failed = true;
            continue;
        }
        match decode(&bytes) {
            Some(text) => scan(&text, &name, dict, &mut out.found),
            None => out.binary = true,
        }
    }
    out
}

/// Whether this member is itself an archive, or a document made of one.
///
/// Those are not opened: one level, so that an archive of archives cannot be
/// used to make this walk arbitrarily deep, or a few kilobytes of nesting made
/// to unpack to terabytes.
fn is_archive(name: &str) -> bool {
    let name = name.to_lowercase();
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_string()).unwrap_or_default();
    crate::filter::is_archive_ext(&ext) || shape(&name, &ext) != Shape::Plain
}

/// Pull the words out of an Office document.
///
/// These are zips of XML, so the zip reader is already here — but their text
/// cannot be matched as XML. Word splits a sentence wherever the formatting
/// changes, so "annual report" is stored as `annual</w:t>…<w:t>report`: searched
/// as markup, the phrase is simply not there, and the search would answer "not
/// found" about a document that says it on its first page. So the runs are put
/// back together first, and only then read.
fn in_office(path: &Path, dict: &Dictionary, part: Part) -> Looked {
    let Ok(file) = std::fs::File::open(path) else {
        return Looked {
            failed: true,
            ..Default::default()
        };
    };
    let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        return Looked {
            failed: true,
            ..Default::default()
        };
    };
    let mut out = Looked {
        read: true,
        ..Default::default()
    };
    let mut budget = ZIP_MAX_BYTES;
    for i in 0..zip.len().min(ZIP_MAX_MEMBERS) {
        let Ok(member) = zip.by_index(i) else {
            out.failed = true;
            continue;
        };
        let name = member.name().to_string();
        if !holds_text(part, &name) {
            continue;
        }
        let size = member.size();
        if size > MAX_FILE || size > budget {
            out.too_big = size > MAX_FILE;
            continue;
        }
        budget = budget.saturating_sub(size);
        let Some(_room) = Room::take(size.max(1)) else {
            out.too_big = true;
            continue;
        };
        let mut bytes = Vec::new();
        if member.take(MAX_FILE).read_to_end(&mut bytes).is_err() {
            out.failed = true;
            continue;
        }
        let xml = String::from_utf8_lossy(&bytes);
        // Named for the document, not for the part of it: `word/document.xml`
        // is an implementation detail of .docx, and a result saying so would be
        // telling the reader about Microsoft's file format rather than about
        // their document.
        scan(&words(&xml), "", dict, &mut out.found);
    }
    out
}

/// Which members of a document hold the words somebody wrote.
fn holds_text(part: Part, member: &str) -> bool {
    match part {
        Part::Word => matches!(
            member,
            "word/document.xml" | "word/footnotes.xml" | "word/endnotes.xml"
        ),
        // Every string in a workbook is kept once, here, and referred to by
        // number from the cells.
        Part::Sheet => member == "xl/sharedStrings.xml",
        Part::Slides => member.starts_with("ppt/slides/slide") && member.ends_with(".xml"),
        // OpenDocument keeps the document in one part, whatever kind it is.
        Part::Open => member == "content.xml",
    }
}

/// The text of an XML document, with the markup taken out and a line break
/// where the document had a paragraph.
fn words(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len() / 4);
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        push_text(&rest[..open], &mut out);
        let Some(close) = rest[open..].find('>') else {
            break;
        };
        let tag = &rest[open + 1..open + close];
        // A paragraph, a row, a cell's string: the places a reader would see a
        // line end. Without them a page comes out as one line, and a pattern
        // anchored to a line could never match.
        if tag.starts_with("/w:p")
            || tag.starts_with("/a:p")
            || tag.starts_with("/si")
            || tag.starts_with("w:br")
            || tag.starts_with("/text:p")
        {
            out.push('\n');
        }
        rest = &rest[open + close + 1..];
    }
    push_text(rest, &mut out);
    out
}

/// Text between two tags, with XML's five escapes put back.
fn push_text(text: &str, out: &mut String) {
    if text.is_empty() {
        return;
    }
    if !text.contains('&') {
        out.push_str(text);
        return;
    }
    let text = text
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        // Last, or an escaped escape comes out wrong.
        .replace("&amp;", "&");
    out.push_str(&text);
}

/// Look through a tar, which may be gzipped.
///
/// A tar has no directory at the end of it the way a zip does: the only way to
/// know what is in one is to read it from the beginning, so listing the names
/// costs what reading it costs. There is nothing to be gained by offering the
/// names alone.
fn in_tar(path: &Path, packed: bool, dict: &Dictionary, contents: bool) -> Looked {
    let Ok(file) = std::fs::File::open(path) else {
        return Looked {
            failed: true,
            ..Default::default()
        };
    };
    let file = std::io::BufReader::new(file);
    let stream: Box<dyn Read> = if packed {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    // However small the archive is on disk.
    let mut archive = tar::Archive::new(stream.take(ZIP_MAX_BYTES));
    let Ok(entries) = archive.entries() else {
        return Looked {
            failed: true,
            ..Default::default()
        };
    };

    let mut out = Looked {
        read: true,
        ..Default::default()
    };
    for (n, entry) in entries.enumerate() {
        if n >= ZIP_MAX_MEMBERS {
            break;
        }
        let Ok(mut entry) = entry else {
            out.failed = true;
            break;
        };
        let name = entry
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.ends_with('/') {
            continue;
        }
        let short = name.rsplit('/').next().unwrap_or(&name).to_string();
        for term in dict.why(&short) {
            out.found.push(Inside {
                term: term.to_string(),
                member: name.clone(),
                line: 0,
                text: String::new(),
            });
        }
        if !contents || is_archive(&name) {
            continue;
        }
        let size = entry.size();
        if size > MAX_FILE {
            out.too_big = true;
            continue;
        }
        let Some(_room) = Room::take(size.max(1)) else {
            out.too_big = true;
            continue;
        };
        let mut bytes = Vec::new();
        if entry.read_to_end(&mut bytes).is_err() {
            out.failed = true;
            continue;
        }
        match decode(&bytes) {
            Some(text) => scan(&text, &name, dict, &mut out.found),
            None => out.binary = true,
        }
    }
    out
}

/// One gzipped file — a `.log.gz` is still a log.
fn in_gz(path: &Path, dict: &Dictionary) -> Looked {
    let Ok(file) = std::fs::File::open(path) else {
        return Looked {
            failed: true,
            ..Default::default()
        };
    };
    // What it unpacks to is not known until it is unpacked, so it claims the
    // most it may become.
    let Some(_room) = Room::take(MAX_FILE) else {
        return Looked {
            too_big: true,
            ..Default::default()
        };
    };
    let mut bytes = Vec::new();
    let unpacked = flate2::read::GzDecoder::new(std::io::BufReader::new(file))
        .take(MAX_FILE)
        .read_to_end(&mut bytes);
    if unpacked.is_err() {
        return Looked {
            failed: true,
            ..Default::default()
        };
    }
    // Named for what it is inside: `access.log.gz` holds `access.log`.
    let inner = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut out = Looked {
        read: true,
        ..Default::default()
    };
    match decode(&bytes) {
        Some(text) => scan(&text, &inner, dict, &mut out.found),
        None => out.binary = true,
    }
    out
}

/// Look through a 7-zip archive.
///
/// Its directory is compressed along with everything else, so unlike a zip
/// there is no listing to be read cheaply: the names and the contents come out
/// of the same unpacking.
fn in_7z(path: &Path, dict: &Dictionary) -> Looked {
    let mut out = Looked {
        read: true,
        ..Default::default()
    };
    let mut budget = ZIP_MAX_BYTES;
    let mut count = 0usize;
    let opened = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty()).map(
        |mut archive| {
            archive.for_each_entries(|entry, reader| {
                count += 1;
                if count > ZIP_MAX_MEMBERS {
                    return Ok(false);
                }
                let name = entry.name().to_string();
                if entry.is_directory() {
                    return Ok(true);
                }
                let short = name.rsplit('/').next().unwrap_or(&name).to_string();
                for term in dict.why(&short) {
                    out.found.push(Inside {
                        term: term.to_string(),
                        member: name.clone(),
                        line: 0,
                        text: String::new(),
                    });
                }
                let size = entry.size();
                if size > MAX_FILE || size > budget || is_archive(&name) {
                    out.too_big = out.too_big || size > MAX_FILE;
                    // Read past it anyway: a 7z is one compressed stream, and
                    // skipping a member means decompressing it unread.
                    let mut sink = std::io::sink();
                    let _ = std::io::copy(&mut reader.take(MAX_FILE), &mut sink);
                    return Ok(true);
                }
                budget = budget.saturating_sub(size);
                let Some(_room) = Room::take(size.max(1)) else {
                    out.too_big = true;
                    return Ok(true);
                };
                let mut bytes = Vec::new();
                if reader.take(MAX_FILE).read_to_end(&mut bytes).is_err() {
                    return Ok(true);
                }
                match decode(&bytes) {
                    Some(text) => scan(&text, &name, dict, &mut out.found),
                    None => out.binary = true,
                }
                Ok(true)
            })
        },
    );
    // Encrypted headers, a password, a format this version does not know.
    if opened.is_err() || opened.is_ok_and(|inner| inner.is_err()) {
        out.failed = true;
    }
    out
}

/// Pull the words out of a PDF.
///
/// Text in a PDF is drawing instructions — glyphs placed on a page, each in
/// whatever encoding its font declares — so there is no text to read without a
/// parser that understands fonts. What comes back is a best effort, and a PDF
/// that is a photograph of a page contains no text at all: that is reported as
/// a file read with nothing in it rather than as a file that does not mention
/// what it plainly does.
fn in_pdf(path: &Path, size: u64, dict: &Dictionary) -> Looked {
    if size > MAX_FILE {
        return Looked {
            too_big: true,
            ..Default::default()
        };
    }
    // A PDF is parsed whole, and the parser holds more than the file while it
    // works: twice its size is a fair claim on the shared budget.
    let Some(_room) = Room::take(size.saturating_mul(2).max(1)) else {
        return Looked {
            too_big: true,
            ..Default::default()
        };
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Looked {
            failed: true,
            ..Default::default()
        };
    };
    // Noisy on malformed files, and it is the caller's business, not the
    // user's: the counters say how many could not be read.
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let text = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&bytes));
    std::panic::set_hook(quiet);

    match text {
        Ok(Ok(text)) => {
            let mut out = Looked {
                read: true,
                ..Default::default()
            };
            scan(&text, "", dict, &mut out.found);
            out
        }
        // Encrypted, damaged, or a construction the parser will not take.
        _ => Looked {
            failed: true,
            ..Default::default()
        },
    }
}

/// Word, Excel and PowerPoint as they were before 2007: a compound file whose
/// streams hold the text.
///
/// Best effort, and said to be. A correct `.doc` reader follows the piece table
/// in the table stream to put the document back together; what this does is
/// read the runs of text out of the streams that hold them, which finds the
/// words and does not reconstruct the document. For a search — does this file
/// mention X — that is the question being asked. For anything else it is not a
/// document reader, and nothing here pretends it is.
fn in_ole(path: &Path, dict: &Dictionary) -> Looked {
    let Ok(mut file) = cfb::open(path) else {
        return Looked {
            failed: true,
            ..Default::default()
        };
    };
    let streams: Vec<String> = file
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| e.path().to_string_lossy().to_string())
        .collect();
    let mut out = Looked {
        read: true,
        ..Default::default()
    };
    let mut budget = ZIP_MAX_BYTES;
    for name in streams {
        if !holds_words(&name) {
            continue;
        }
        let Ok(stream) = file.open_stream(&name) else {
            out.failed = true;
            continue;
        };
        let Some(_room) = Room::take(MAX_FILE.min(budget).max(1)) else {
            out.too_big = true;
            continue;
        };
        let mut bytes = Vec::new();
        if stream.take(MAX_FILE.min(budget)).read_to_end(&mut bytes).is_err() {
            out.failed = true;
            continue;
        }
        budget = budget.saturating_sub(bytes.len() as u64);
        scan(&runs(&bytes), "", dict, &mut out.found);
    }
    out
}

/// The streams of a compound file that hold what somebody typed.
fn holds_words(name: &str) -> bool {
    let name = name.trim_start_matches('/');
    matches!(
        name,
        "WordDocument" | "Workbook" | "Book" | "PowerPoint Document" | "1Table" | "0Table"
    ) || name.starts_with("__substg1.0_") // the parts of an Outlook .msg
}

/// The runs of readable text in a block of bytes, one to a line.
///
/// Both ways these formats store a string: plain bytes, and UTF-16 — a Word 97
/// document is UTF-16 and an Excel sheet is usually not, often in the same
/// family of files. Runs shorter than four characters are dropped: in binary
/// they are noise, and a term that short would match half the disk anyway.
fn runs(bytes: &[u8]) -> String {
    const MIN: usize = 4;
    let mut out = String::new();
    let mut run = String::new();
    let push = |run: &mut String, out: &mut String| {
        if run.chars().count() >= MIN {
            out.push_str(run);
            out.push('\n');
        }
        run.clear();
    };
    // Eight-bit runs.
    for &b in bytes {
        match b {
            b'\t' | b' '..=b'~' => run.push(b as char),
            _ => push(&mut run, &mut out),
        }
    }
    push(&mut run, &mut out);
    // And the same again, reading pairs: UTF-16 text looks like every other
    // byte being zero, which the pass above breaks into single characters.
    for start in [0usize, 1] {
        if bytes.len() <= start {
            continue;
        }
        for pair in bytes[start..].chunks_exact(2) {
            let unit = u16::from_le_bytes([pair[0], pair[1]]);
            match char::from_u32(unit as u32) {
                Some(c) if c == '\t' || (' '..='~').contains(&c) || (c as u32) > 0xA0 => {
                    run.push(c)
                }
                _ => push(&mut run, &mut out),
            }
        }
        push(&mut run, &mut out);
    }
    out
}
