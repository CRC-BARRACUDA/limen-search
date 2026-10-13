//! Keeping a dictionary, and carrying one somewhere else.

use crate::book::{self, Book};

/// Somewhere to keep a book, cleared up afterwards.
struct Dir(std::path::PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("limen-search-book-{name}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("a place to keep things");
        Dir(p)
    }
    fn as_str(&self) -> String {
        self.0.display().to_string()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const TERMS: &str = "# from the ticket\ninvoice\n\"annual report\"\n/\\.p12$/\n";

/// What was kept is what comes back — every line of it, notes included.
#[test]
fn a_dictionary_survives_being_kept() {
    let dir = Dir::new("round");
    let mut book = Book::default();
    book.dictionaries.insert("Exfil".into(), TERMS.into());
    book.save(&dir.as_str()).expect("written");

    let again = Book::load(&dir.as_str());
    assert_eq!(again.names(), ["Exfil"]);
    assert_eq!(again.dictionaries["Exfil"], TERMS, "the lines, exactly as written");
}

/// A name used twice is a dictionary changed, not two dictionaries.
#[test]
fn saving_under_a_name_that_is_taken_replaces_it() {
    let dir = Dir::new("replace");
    let mut book = Book::default();
    book.dictionaries.insert("Exfil".into(), TERMS.into());
    book.save(&dir.as_str()).unwrap();
    let mut book = Book::load(&dir.as_str());
    book.dictionaries.insert("Exfil".into(), "something else".into());
    book.save(&dir.as_str()).unwrap();

    let third = Book::load(&dir.as_str());
    assert_eq!(third.names(), ["Exfil"]);
    assert_eq!(third.dictionaries["Exfil"], "something else");
}

/// A book that is missing, empty or nonsense is an empty book — a broken file
/// should not stop somebody searching. And a file that belongs to something
/// else is left alone rather than read as ours.
#[test]
fn a_book_that_cannot_be_read_is_simply_empty() {
    let dir = Dir::new("broken");
    assert!(Book::load(&dir.as_str()).names().is_empty(), "nothing kept yet");
    for junk in ["{{{", r#"{"limen":"something.else","dictionaries":{"a":"b"}}"#] {
        std::fs::write(dir.0.join("dictionaries.json"), junk).unwrap();
        assert!(Book::load(&dir.as_str()).names().is_empty(), "{junk}");
    }
}

/// Exported as plain text, because a dictionary already is one: a term to a
/// line, which somebody can paste into a ticket or keep beside the code it is
/// about.
#[test]
fn a_dictionary_is_exported_as_the_text_it_is() {
    let text = book::to_text("Exfil sweep", TERMS);
    assert!(text.starts_with("# Limen search dictionary: Exfil sweep\n"));
    assert!(text.contains("invoice\n"));
    assert!(text.contains("\"annual report\"\n"));
    assert!(text.ends_with('\n'));
}

/// And read back, with the name it was written under.
#[test]
fn an_exported_dictionary_comes_back_with_its_name() {
    let (name, terms) = book::from_text(&book::to_text("Exfil sweep", TERMS));
    assert_eq!(name.as_deref(), Some("Exfil sweep"));
    assert_eq!(book::count(&terms), 3, "the three terms, not the note");
}

/// Any text file is a dictionary. One written by hand, or by another tool, has
/// no name in it and is read for its lines all the same.
#[test]
fn a_plain_list_somebody_typed_is_a_dictionary_too() {
    let (name, terms) = book::from_text("invoice\npasswords.txt\n/\\.kdbx$/\n");
    assert_eq!(name, None, "nothing claimed a name");
    assert_eq!(book::count(&terms), 3);
    assert!(crate::dict::parse(&terms).hits("secrets.kdbx"), "and it works as one");
}

/// What counts as a term, for the line that says how many were loaded.
#[test]
fn notes_and_blank_lines_are_not_terms() {
    assert_eq!(book::count("# a note\n\n  \ninvoice\n"), 1);
    assert_eq!(book::count(""), 0);
    assert_eq!(book::count("# only notes\n"), 0);
}

/// A name becomes a filename that is still recognisably the name.
#[test]
fn a_name_becomes_a_filename_that_is_still_the_name() {
    assert_eq!(book::file_stem("Exfil sweep — Q3"), "Exfil_sweep_Q3");
    assert_eq!(book::file_stem("Перевірка"), "Перевірка");
    assert_eq!(book::file_stem("  "), "dictionary", "something is always offered");
    assert_eq!(book::file_stem("../../etc/passwd"), "etc_passwd", "no path in a name");
}
