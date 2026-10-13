//! Looking inside files: what is read, what is stepped over, and what the
//! results say about it.
//!
//! Every case here is built as a real file on disk and read by the real code —
//! a zip written by the zip writer, a document assembled part by part. A test
//! that asserted against a hand-made string would be testing the string.

use crate::dict::parse;
use crate::filter::Filters;
use crate::peek::{Look, MAX_FILE};
use crate::walk::{walk, Found, Hit};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// A directory of fixtures, cleaned up after itself.
struct Case(PathBuf);

impl Case {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("limen-search-peek-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("the fixture directory");
        Case(root)
    }

    fn file(&self, name: &str, bytes: impl AsRef<[u8]>) -> &Self {
        std::fs::write(self.0.join(name), bytes).expect("a fixture file");
        self
    }

    /// A zip holding each (name, contents) in turn.
    fn zip(&self, name: &str, members: &[(&str, &[u8])]) -> &Self {
        let file = std::fs::File::create(self.0.join(name)).expect("a fixture zip");
        let mut w = zip::ZipWriter::new(file);
        let how = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (member, bytes) in members {
            w.start_file(*member, how).expect("a member");
            w.write_all(bytes).expect("the member's bytes");
        }
        w.finish().expect("the zip");
        self
    }

    /// A tar, plain or gzipped, holding each (name, contents) in turn.
    fn tar(&self, name: &str, packed: bool, members: &[(&str, &[u8])]) -> &Self {
        let mut body = Vec::new();
        {
            let mut w = tar::Builder::new(&mut body);
            for (member, bytes) in members {
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                w.append_data(&mut header, member, *bytes).expect("a member");
            }
            w.finish().expect("the tar");
        }
        if packed {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            gz.write_all(&body).expect("packing");
            body = gz.finish().expect("the gzip");
        }
        self.file(name, body)
    }

    fn look(&self, dict: &str, how: Look) -> Found {
        walk(
            vec![self.0.clone()],
            Arc::new(parse(dict)),
            Arc::new(Filters::default()),
            how,
            1,
            1_000,
            Arc::new(AtomicBool::new(false)),
        )
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const CONTENTS: Look = Look {
    contents: true,
    archives: false,
};
const BOTH: Look = Look {
    contents: true,
    archives: true,
};
const NAMES_IN_ZIP: Look = Look {
    contents: false,
    archives: true,
};

fn named<'a>(found: &'a Found, name: &str) -> Option<&'a Hit> {
    found.hits.iter().find(|h| h.name == name)
}

/// Stage A: a file whose name says nothing and whose contents say everything.
#[test]
fn a_word_inside_a_text_file_is_found() {
    let case = Case::new("text");
    case.file("notes.md", "nothing here\nan invoice for March\nnor here\n")
        .file("quiet.md", "nothing at all");

    // Names alone: neither file is called anything like it.
    let names = case.look("invoice", Look::default());
    assert!(names.hits.is_empty(), "{:?}", names.hits);

    let inside = case.look("invoice", CONTENTS);
    let hit = named(&inside, "notes.md").expect("the file that says it");
    assert_eq!(inside.hits.len(), 1, "and only that one");
    assert_eq!(hit.inside.len(), 1);
    assert_eq!(hit.inside[0].term, "invoice");
    assert_eq!(hit.inside[0].line, 2, "the line it is on");
    assert_eq!(hit.inside[0].text, "an invoice for March");
    assert!(hit.inside[0].member.is_empty(), "the file's own text");
    assert_eq!(inside.looked, 2, "both files were read");
}

/// A pattern is written against a line, which is what `^` and `$` mean to
/// somebody reading a file.
#[test]
fn a_pattern_anchors_to_a_line_not_to_the_whole_file() {
    let case = Case::new("lines");
    case.file("log.txt", "preamble\nERROR 500\ntrailing\n");
    let found = case.look("/^ERROR \\d+$/", CONTENTS);
    assert_eq!(found.hits.len(), 1, "{:?}", found.hits);
    assert_eq!(found.hits[0].inside[0].text, "ERROR 500");
}

/// Binary files are stepped over, including one that happens to have the word
/// in it in plain ASCII — the point is that nobody wants their search answered
/// out of an executable.
#[test]
fn a_binary_file_is_not_read_as_text() {
    let case = Case::new("binary");
    let mut blob = b"invoice".to_vec();
    blob.push(0);
    blob.extend_from_slice(b"more");
    case.file("thing.bin", blob);

    let found = case.look("invoice", CONTENTS);
    assert!(found.hits.is_empty(), "{:?}", found.hits);
    assert_eq!(found.binary, 1);
    assert_eq!(found.looked, 0);
}

/// UTF-16 is half NUL bytes, and a Windows machine is full of it. Its mark is
/// read before the binary test, or every one of those files would be skipped.
#[test]
fn a_utf16_file_is_still_text() {
    let case = Case::new("utf16");
    let mut bytes = vec![0xFF, 0xFE];
    for unit in "an invoice".encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    case.file("windows.txt", bytes);

    let found = case.look("invoice", CONTENTS);
    assert_eq!(found.hits.len(), 1, "{found:?}");
    assert_eq!(found.binary, 0);
}

/// A text file larger than anything that could be held in memory is still
/// read — a line at a time, holding none of it.
///
/// It used to be skipped for being large, because reading it meant holding
/// it: the whole file, and then a second copy of it from the decoder, on
/// every thread at once. Streaming it costs a line.
#[test]
fn a_large_text_file_is_read_a_line_at_a_time() {
    let case = Case::new("huge");
    // Comfortably past the ceiling for anything that must be buffered, with
    // the word at the far end of it.
    let path = case.0.join("huge.log");
    {
        use std::io::Write as _;
        let mut out = std::io::BufWriter::new(std::fs::File::create(&path).expect("the big file"));
        let filler = vec![b'x'; 4095];
        for _ in 0..(MAX_FILE / 4096 + 64) {
            out.write_all(&filler).expect("filler");
            out.write_all(b"\n").expect("a line end");
        }
        out.write_all(b"an invoice at the very end\n").expect("the needle");
    }
    assert!(
        std::fs::metadata(&path).expect("it exists").len() > MAX_FILE,
        "the fixture has to be past the ceiling or it tests nothing"
    );

    let found = case.look("invoice", CONTENTS);
    assert_eq!(found.hits.len(), 1, "{:?}", found.hits);
    assert_eq!(found.too_big, 0, "it was skipped: {found:?}");
    assert_eq!(found.hits[0].inside[0].text, "an invoice at the very end");
    assert!(found.hits[0].inside[0].line > 4_000, "the line it was on");
}

/// A file that *must* be held in memory to be read at all is still skipped
/// when it is too large to hold.
#[test]
fn a_format_that_must_be_buffered_is_skipped_when_it_is_too_large() {
    let case = Case::new("hugepdf");
    // A PDF is parsed whole; nothing can read one a line at a time. Written
    // sparsely, so the test costs no disk.
    let big = std::fs::File::create(case.0.join("huge.pdf")).expect("the big file");
    big.set_len(MAX_FILE + 1).expect("its length");
    drop(big);
    case.file("small.txt", "an invoice");

    let found = case.look("invoice", CONTENTS);
    assert_eq!(found.hits.len(), 1, "only the small one: {:?}", found.hits);
    assert_eq!(found.too_big, 1);
}

/// Stage B: what an archive holds, by name, without decompressing anything.
#[test]
fn a_name_inside_a_zip_is_found() {
    let case = Case::new("zipnames");
    case.zip(
        "backup.zip",
        &[("notes/passwords.xlsx", b"PK-ish"), ("other.bin", b"x")],
    );

    let found = case.look("passwords", NAMES_IN_ZIP);
    let hit = named(&found, "backup.zip").expect("the archive");
    assert_eq!(hit.inside.len(), 1);
    assert_eq!(hit.inside[0].member, "notes/passwords.xlsx");
    assert_eq!(hit.inside[0].line, 0, "a name is not on a line");
    assert!(hit.why.is_empty(), "the archive's own name says nothing");
}

/// Stage C: what an archive's members say.
#[test]
fn a_word_inside_a_zipped_file_is_found() {
    let case = Case::new("zipbody");
    case.zip(
        "backup.zip",
        &[("notes.txt", b"an invoice for March"), ("blob.bin", &[0u8, 1, 2])],
    );

    // Names only: nothing in this archive is called "invoice".
    let names = case.look("invoice", NAMES_IN_ZIP);
    assert!(names.hits.is_empty(), "{:?}", names.hits);

    let found = case.look("invoice", BOTH);
    let hit = named(&found, "backup.zip").expect("the archive");
    assert_eq!(hit.inside[0].member, "notes.txt");
    assert_eq!(hit.inside[0].line, 1);
    assert_eq!(hit.inside[0].text, "an invoice for March");
}

/// An archive inside an archive is not opened: one level, so that an archive
/// of archives cannot be used to make the walk arbitrarily deep.
#[test]
fn an_archive_inside_an_archive_is_not_opened() {
    let case = Case::new("nested");
    let mut inner = Vec::new();
    {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut inner));
        let how = zip::write::FileOptions::default();
        w.start_file("deep.txt", how).unwrap();
        w.write_all(b"an invoice").unwrap();
        w.finish().unwrap();
    }
    case.zip("outer.zip", &[("inner.zip", &inner)]);

    let found = case.look("invoice", BOTH);
    assert!(found.hits.is_empty(), "it went a level deep: {:?}", found.hits);
    // Not decompressed at all, rather than decompressed and then thrown away
    // for not being text: a bomb is cheap to store and expensive to unpack, so
    // the rule has to be "do not open it", not "do not believe it".
    assert_eq!(found.binary, 0, "the inner archive was unpacked: {found:?}");
}

/// Stage D: a Word document, whose words are split across runs wherever the
/// formatting changes — the case that makes searching the XML useless.
#[test]
fn a_phrase_split_across_runs_in_a_docx_is_found() {
    let case = Case::new("docx");
    // Exactly what Word writes when half the phrase is in bold.
    let document = r#"<?xml version="1.0"?><w:document><w:body><w:p><w:r><w:t>the annual</w:t></w:r><w:r><w:t xml:space="preserve"> report</w:t></w:r></w:p><w:p><w:r><w:t>second page</w:t></w:r></w:p></w:body></w:document>"#;
    case.zip(
        "memo.docx",
        &[
            ("[Content_Types].xml", b"<Types/>"),
            ("word/document.xml", document.as_bytes()),
        ],
    );

    let found = case.look("\"annual report\"", CONTENTS);
    let hit = named(&found, "memo.docx").expect("the document");
    assert_eq!(hit.inside.len(), 1, "{:?}", hit.inside);
    assert_eq!(hit.inside[0].text, "the annual report");
    assert!(
        hit.inside[0].member.is_empty(),
        "named for the document, not for Microsoft's part of it"
    );
    // Paragraphs are lines, or a pattern anchored to one could never match.
    let anchored = case.look("/^second page$/", CONTENTS);
    assert_eq!(anchored.hits.len(), 1, "{:?}", anchored.hits);
}

/// A spreadsheet keeps every string once, in a part of its own.
#[test]
fn a_word_in_a_spreadsheet_is_found() {
    let case = Case::new("xlsx");
    let shared = r#"<sst><si><t>Totals</t></si><si><t>invoice 2024</t></si></sst>"#;
    case.zip(
        "book.xlsx",
        &[("xl/sharedStrings.xml", shared.as_bytes())],
    );
    let found = case.look("invoice", CONTENTS);
    assert_eq!(found.hits.len(), 1, "{:?}", found.hits);
    assert_eq!(found.hits[0].inside[0].text, "invoice 2024");
}

/// The markup itself is not the document: a term that appears only in a tag
/// name or an attribute has not been found.
#[test]
fn the_markup_of_a_document_is_not_searched() {
    let case = Case::new("markup");
    let document = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>nothing to see</w:t></w:r></w:p></w:body></w:document>"#;
    case.zip("memo.docx", &[("word/document.xml", document.as_bytes())]);

    let found = case.look("schemas", CONTENTS);
    assert!(found.hits.is_empty(), "it matched the markup: {:?}", found.hits);
}

/// A damaged archive is counted and stepped over, like any other file that
/// will not be read.
#[test]
fn a_file_that_is_not_the_archive_it_claims_to_be_is_stepped_over() {
    let case = Case::new("broken");
    case.file("broken.zip", b"PK\x03\x04 and then nonsense")
        .file("real.txt", "an invoice");

    let found = case.look("invoice", BOTH);
    assert_eq!(found.hits.len(), 1, "the good file still came back");
    assert_eq!(found.unread, 1, "and the bad one was counted");
}

/// With nothing to look for, nothing is opened — which is what keeps a content
/// search over a whole disk from being the default.
#[test]
fn an_empty_dictionary_opens_nothing() {
    let case = Case::new("empty");
    case.file("a.txt", "whatever").file("b.txt", "whatever");
    let found = case.look("", BOTH);
    assert_eq!(found.hits.len(), 2, "both still match by name");
    assert_eq!(found.looked, 0, "and neither was opened");
}

/// Every term that found a file is reported, from its name and its contents
/// alike, each said once.
#[test]
fn a_file_found_both_ways_says_both() {
    let case = Case::new("both");
    case.file("invoice-march.txt", "paid by cheque");
    let found = case.look("invoice\ncheque", CONTENTS);
    let hit = &found.hits[0];
    assert_eq!(hit.why, ["invoice"]);
    assert_eq!(hit.inside.len(), 1);
    assert_eq!(hit.inside[0].term, "cheque");
    assert_eq!(
        crate::view::terms_that_found(hit),
        ["invoice".to_string(), "cheque".to_string()]
    );
}

/// A `.jar`, a `.apk`, an `.epub`: a zip with somebody else's name on it, and
/// reading one is reading a zip.
#[test]
fn a_zip_under_another_name_is_still_a_zip() {
    let case = Case::new("jar");
    case.zip("app.jar", &[("META-INF/invoice.txt", b"an invoice")]);
    let found = case.look("invoice", BOTH);
    let hit = named(&found, "app.jar").expect("the jar");
    assert_eq!(hit.inside.len(), 1, "{:?}", hit.inside);
    // The member's name matched first and its text matched after; what is
    // reported is the one that says more.
    assert_eq!(hit.inside[0].member, "META-INF/invoice.txt");
    assert_eq!(hit.inside[0].line, 1);
    assert_eq!(hit.inside[0].text, "an invoice");
}

/// A tar, and the same tar gzipped — which is how nearly every one of them is
/// actually written.
#[test]
fn a_word_inside_a_tar_is_found() {
    let case = Case::new("tar");
    case.tar("bundle.tar", false, &[("notes.txt", b"an invoice for March")])
        .tar("bundle.tar.gz", true, &[("deep/notes.txt", b"an invoice")]);

    let found = case.look("invoice", BOTH);
    let plain = named(&found, "bundle.tar").expect("the tar");
    assert_eq!(plain.inside[0].member, "notes.txt");
    assert_eq!(plain.inside[0].text, "an invoice for March");
    let packed = named(&found, "bundle.tar.gz").expect("the gzipped tar");
    assert_eq!(packed.inside[0].member, "deep/notes.txt");
}

/// One gzipped file is still the file it was: `access.log.gz` holds a log.
#[test]
fn a_word_inside_a_gzipped_file_is_found() {
    use flate2::write::GzEncoder;
    let case = Case::new("gz");
    let mut packed = GzEncoder::new(Vec::new(), flate2::Compression::fast());
    packed.write_all(b"GET /invoice HTTP/1.1\n").unwrap();
    case.file("access.log.gz", packed.finish().unwrap());

    let found = case.look("invoice", BOTH);
    let hit = named(&found, "access.log.gz").expect("the log");
    assert_eq!(hit.inside[0].member, "access.log", "named for what is inside");
    assert_eq!(hit.inside[0].line, 1);
}

/// A 7-zip archive, whose directory is compressed with everything else.
#[test]
fn a_word_inside_a_7z_is_found() {
    let case = Case::new("sevenz");
    let made = sevenz_rust::SevenZWriter::create(case.0.join("box.7z")).map(|mut w| {
        let mut entry = sevenz_rust::SevenZArchiveEntry::new();
        entry.name = "notes.txt".into();
        entry.has_stream = true;
        w.push_archive_entry(entry, Some(&b"an invoice for March"[..]))
            .expect("a member");
        w.finish().expect("the archive");
    });
    made.expect("a 7z to read back");

    let found = case.look("invoice", BOTH);
    let hit = named(&found, "box.7z").expect("the archive");
    assert_eq!(hit.inside[0].member, "notes.txt");
    assert_eq!(hit.inside[0].text, "an invoice for March");
}

/// An OpenDocument keeps its whole document in one part.
#[test]
fn a_word_in_an_opendocument_is_found() {
    let case = Case::new("odt");
    let content = r#"<office:document-content><office:body><text:p>the annual</text:p><text:p>report of 2024</text:p></office:body></office:document-content>"#;
    case.zip("notes.odt", &[("content.xml", content.as_bytes())]);
    let found = case.look("/^report of \\d{4}$/", CONTENTS);
    assert_eq!(found.hits.len(), 1, "{:?}", found.hits);
    assert_eq!(found.hits[0].inside[0].text, "report of 2024");
}

/// A PDF: text in a PDF is glyphs placed on a page, so this goes through a
/// real parser rather than through anything this module could write.
#[test]
fn a_word_inside_a_pdf_is_found() {
    let case = Case::new("pdf");
    case.file("memo.pdf", pdf_saying("an invoice for March"));

    let found = case.look("invoice", CONTENTS);
    let hit = named(&found, "memo.pdf").expect("the pdf");
    assert!(
        hit.inside[0].text.contains("invoice"),
        "{:?}",
        hit.inside
    );
}

/// A PDF that is a photograph of a page has no text in it, and the search says
/// it read the file rather than that the words are not in it.
#[test]
fn a_pdf_with_no_text_is_read_and_found_empty() {
    let case = Case::new("pdfempty");
    case.file("scan.pdf", pdf_saying(""));
    let found = case.look("invoice", CONTENTS);
    assert!(found.hits.is_empty());
    assert_eq!(found.looked, 1, "it was read: {found:?}");
    assert_eq!(found.unread, 0);
}

/// Word before 2007: a compound file with the text in a stream, as UTF-16.
#[test]
fn a_word_inside_a_legacy_document_is_found() {
    let case = Case::new("ole");
    let path = case.0.join("memo.doc");
    let mut doc = cfb::create(&path).expect("a compound file");
    {
        use std::io::Write as _;
        let mut stream = doc.create_stream("WordDocument").expect("the stream");
        let mut bytes = vec![0u8; 512];
        for unit in "an invoice for March".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        stream.write_all(&bytes).expect("the text");
    }
    doc.flush().expect("written");
    drop(doc);

    let found = case.look("invoice", CONTENTS);
    let hit = named(&found, "memo.doc").expect("the document");
    assert!(
        hit.inside[0].text.contains("an invoice for March"),
        "{:?}",
        hit.inside
    );
}

/// The smallest PDF that actually says something, written by hand so the test
/// does not depend on a PDF writer as well as a PDF reader.
///
/// Its text is written as a **hex string**, which is ordinary in a real PDF
/// and is the whole point of the fixture: the words appear nowhere in the
/// file's bytes, so nothing but a parser that understands PDF can find them.
/// Against a plain `(invoice) Tj` the test would pass with no parser at all.
fn pdf_saying(words: &str) -> Vec<u8> {
    let hex: String = words.bytes().map(|b| format!("{b:02x}")).collect();
    let stream = if words.is_empty() {
        Vec::new()
    } else {
        format!("BT /F1 12 Tf 72 720 Td <{hex}> Tj ET").into_bytes()
    };
    assert!(
        !find_bytes(&stream, words.as_bytes()),
        "the fixture has to hide the words, or it tests nothing"
    );

    // Built as bytes throughout: a deflated stream does not survive being put
    // in a String.
    let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut at: Vec<usize> = Vec::new();
    let add = |out: &mut Vec<u8>, at: &mut Vec<usize>, body: &[u8]| {
        at.push(out.len());
        out.extend_from_slice(body);
    };
    add(&mut out, &mut at, b"1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n");
    add(
        &mut out,
        &mut at,
        b"2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n",
    );
    add(
        &mut out,
        &mut at,
        b"3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Resources<</Font<</F1 5 0 R>>>>/Contents 4 0 R>>endobj\n",
    );
    at.push(out.len());
    out.extend_from_slice(format!("4 0 obj<</Length {}>>stream\n", stream.len()).as_bytes());
    out.extend_from_slice(&stream);
    out.extend_from_slice(b"\nendstream endobj\n");
    add(
        &mut out,
        &mut at,
        b"5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj\n",
    );

    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", at.len() + 1).as_bytes());
    for offset in &at {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n",
            at.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// A real content search of a real tree, run by hand:
/// `cargo test -- --ignored live_inside --nocapture`.
///
/// Points at this checkout, looks for something only a file's text can answer,
/// and prints what it cost.
#[test]
#[ignore = "reads a real tree; run with --ignored"]
fn live_inside() {
    let at = std::env::var("LIMEN_SEARCH_AT")
        .unwrap_or_else(|_| std::env::current_dir().unwrap().display().to_string());
    let term = std::env::var("LIMEN_SEARCH_FOR").unwrap_or_else(|_| "catch_unwind".into());
    let started = std::time::Instant::now();
    let found = walk(
        vec![PathBuf::from(&at)],
        Arc::new(parse(&term)),
        Arc::new(Filters::default()),
        BOTH,
        crate::walk::threads_available(),
        5_000,
        Arc::new(AtomicBool::new(false)),
    );
    println!(
        "{} hits · {} read · {} not text · {} too large · {} unreadable · {} entries in {:?}",
        found.hits.len(),
        found.looked,
        found.binary,
        found.too_big,
        found.unread,
        found.seen,
        started.elapsed()
    );
    for hit in found.hits.iter().take(5) {
        println!(
            "  {} — {:?}",
            hit.path.display(),
            hit.inside.first().map(|i| (&i.member, i.line, &i.text))
        );
    }
}

/// Every thread together holds no more than the budget allows.
///
/// The ceiling on one file was never the problem: sixteen threads each
/// holding one at the same moment is sixteen times it, and that is what
/// filled the machine. A thread that cannot have its share waits for one.
#[test]
fn what_every_thread_holds_together_is_bounded() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    // Each asks for a tenth of the budget; far more of them than fit at once.
    let share = crate::peek::budget_for_test() / 10;
    let held = Arc::new(AtomicU64::new(0));
    let most = Arc::new(AtomicU64::new(0));

    std::thread::scope(|scope| {
        for _ in 0..40 {
            let (held, most) = (Arc::clone(&held), Arc::clone(&most));
            scope.spawn(move || {
                let room = crate::peek::room_for_test(share).expect("a share");
                let now = held.fetch_add(share, Ordering::AcqRel) + share;
                most.fetch_max(now, Ordering::AcqRel);
                // Long enough that the threads overlap, short enough that the
                // test is not a pause.
                std::thread::sleep(std::time::Duration::from_millis(5));
                held.fetch_sub(share, Ordering::AcqRel);
                drop(room);
            });
        }
    });

    let peak = most.load(Ordering::Acquire);
    assert!(peak > share, "the threads never overlapped, so nothing was tested");
    assert!(
        peak <= crate::peek::budget_for_test(),
        "{peak} bytes were held at once, out of a budget of {}",
        crate::peek::budget_for_test()
    );
    // And the budget is whole again afterwards: a share that was taken and
    // not given back is a search that gets slower every time it is run.
    assert!(
        crate::peek::room_for_test(crate::peek::budget_for_test()).is_some(),
        "the budget leaked"
    );
}

/// More than the whole budget can never be waited for: it is refused rather
/// than blocking for ever.
#[test]
fn more_than_the_budget_is_refused_not_waited_for() {
    assert!(crate::peek::room_for_test(crate::peek::budget_for_test() + 1).is_none());
}
