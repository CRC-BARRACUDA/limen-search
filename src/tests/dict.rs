//! Reading a dictionary, and what it then matches.

use crate::dict::parse;

#[test]
fn a_word_is_found_anywhere_in_a_name_whatever_its_case() {
    let d = parse("invoice");
    assert!(d.hits("Invoice-2024.pdf"));
    assert!(d.hits("2024_INVOICE_final.xlsx"));
    assert!(!d.hits("receipt.pdf"));
}

/// A phrase keeps its spaces, which a list of words would lose.
#[test]
fn a_phrase_keeps_its_spaces() {
    let d = parse("\"annual report\"");
    assert!(d.hits("2023 Annual Report.docx"));
    assert!(!d.hits("report annual.docx"), "the words, but not the phrase");
}

#[test]
fn a_pattern_is_a_regular_expression() {
    let d = parse(r"/^IMG_\d{4}\.jpe?g$/");
    assert!(d.hits("IMG_0042.jpg"));
    assert!(d.hits("img_0042.jpeg"), "patterns ignore case too");
    assert!(!d.hits("IMG_42.jpg"), "four digits were asked for");
    assert!(!d.hits("holiday IMG_0042.jpg"), "anchored, so not anywhere");
}

/// Any one term is enough: a dictionary is a list of things worth finding.
#[test]
fn any_term_is_enough() {
    let d = parse("invoice\n\"annual report\"\n/\\.p12$/");
    for name in ["invoice.txt", "Annual Report.pdf", "keystore.p12"] {
        assert!(d.hits(name), "{name}");
    }
    assert!(!d.hits("notes.md"));
}

/// Notes and blank lines are for whoever reads the dictionary, not for the
/// search.
#[test]
fn notes_and_blank_lines_are_not_terms() {
    let d = parse("# things from the ticket\n\n  invoice  \n\n# and nothing else\n");
    assert_eq!(d.terms.len(), 1);
    assert!(d.hits("invoice.pdf"));
}

/// An empty dictionary is not an empty search: it matches every name, so dates
/// and kinds alone can be the question.
#[test]
fn an_empty_dictionary_matches_everything() {
    let d = parse("   \n# nothing but a note\n");
    assert!(d.is_empty());
    assert!(d.hits("anything at all"));
}

/// A pattern that will not compile is reported, not dropped. A dictionary that
/// quietly ignores a line searches for less than it was asked to.
#[test]
fn a_broken_pattern_is_said_out_loud() {
    let d = parse("invoice\n/[unclosed/\n");
    assert_eq!(d.terms.len(), 1, "the good term still stands");
    assert_eq!(d.rejected.len(), 1);
    assert_eq!(d.rejected[0].0, "/[unclosed/");
    assert!(!d.rejected[0].1.is_empty(), "with a reason");
    assert!(d.rejected[0].1.lines().count() == 1, "one line, for a screen");
}

/// Two slashes are not a pattern, and a lone quote is not a phrase — they are
/// what somebody typed, searched for as written.
#[test]
fn the_shortest_things_are_taken_literally() {
    let d = parse("//\n\"\n");
    assert_eq!(d.terms.len(), 2);
    assert!(d.hits("a//b"));
    assert!(d.hits("say \"what\""));
}
