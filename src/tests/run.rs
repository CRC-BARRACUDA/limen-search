//! A whole search, as the screen drives one: fill the form, start, poll until
//! it is done, read the table.

use crate::*;
use std::path::PathBuf;

/// A tree with a couple of things worth finding in it.
struct Tree(PathBuf);

impl Tree {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("limen-search-run-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["projects/project1", "projects/project2", "logs"] {
            std::fs::create_dir_all(root.join(dir)).expect("the test tree");
        }
        for (path, bytes) in [
            ("projects/project1/Invoice-2024.pdf", 4096),
            ("projects/project2/invoice.txt", 12),
            ("projects/project2/notes.md", 12),
            ("logs/old.zip", 2048),
        ] {
            std::fs::write(root.join(path), vec![b'x'; bytes]).expect("a test file");
        }
        Tree(root)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Drive the module the way its own screen does, and hand back the view it
/// settles on.
fn search_for(tree: &Tree, extra_path: Option<&str>, params: Value) -> Value {
    let mut module = Search::default();
    let mut form = params;
    let paths = match extra_path {
        Some(extra) => format!("{}\n{}", tree.0.display(), tree.0.join(extra).display()),
        None => tree.0.display().to_string(),
    };
    if let Value::Object(o) = &mut form {
        o.insert("paths".into(), Value::String(paths));
    }
    let waiting = module.start(&form, "en");
    assert_eq!(
        waiting.get("auto").and_then(|a| a.get("method")).and_then(Value::as_str),
        Some("poll"),
        "the screen that waits has to ask again, or it waits for ever"
    );
    // Poll as the screen does, with a stop so a broken walk fails the test
    // rather than hanging it.
    for _ in 0..2_000 {
        let view = module.poll("en");
        if view.get("auto").is_none() {
            return view;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the search never finished");
}

/// Every name in the results table — which is the one with a Name column, the
/// screen having a second table listing the paths to search.
fn names(view: &Value) -> Vec<String> {
    let mut out: Vec<String> = tables(view)
        .into_iter()
        .filter(|t| t["columns"][0] == "Name")
        .flat_map(|t| t["rows"].as_array().cloned().unwrap_or_default())
        .map(|r| r[0].as_str().unwrap_or_default().to_string())
        .collect();
    out.sort();
    out
}

/// Every table on a screen, at whatever depth it was drawn.
fn tables(view: &Value) -> Vec<Value> {
    fn walk(v: &Value, out: &mut Vec<Value>) {
        if v["kind"] == "table" {
            out.push(v.clone());
        }
        for child in v["widgets"].as_array().into_iter().flatten() {
            walk(child, out);
        }
        for child in v["children"].as_array().into_iter().flatten() {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(view, &mut out);
    out
}

/// Each result's name, and what the table says found it.
fn found_by(view: &Value) -> std::collections::HashMap<String, String> {
    tables(view)
        .into_iter()
        .filter(|t| t["columns"][0] == "Name")
        .flat_map(|t| t["rows"].as_array().cloned().unwrap_or_default())
        .map(|r| {
            (
                r[0].as_str().unwrap_or_default().to_string(),
                r[1].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The ids the results table gave its rows.
fn row_ids(view: &Value) -> Vec<String> {
    tables(view)
        .into_iter()
        .filter(|t| t["columns"][0] == "Name")
        .flat_map(|t| t["row_ids"].as_array().cloned().unwrap_or_default())
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The dictionary as the screen lists it: each term and what kind it is.
fn listed_terms(view: &Value) -> Vec<(String, String)> {
    tables(view)
        .into_iter()
        .filter(|t| t["columns"][0] == "Term")
        .flat_map(|t| t["rows"].as_array().cloned().unwrap_or_default())
        .map(|r| {
            (
                r[0].as_str().unwrap_or_default().to_string(),
                r[1].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The paths the screen says will be searched, as its table lists them.
fn listed_paths(view: &Value) -> Vec<String> {
    tables(view)
        .into_iter()
        .filter(|t| t["columns"][0] == "Path" && t["columns"].as_array().map(Vec::len) == Some(1))
        .flat_map(|t| t["rows"].as_array().cloned().unwrap_or_default())
        .map(|r| r[0].as_str().unwrap_or_default().to_string())
        .collect()
}

fn said(view: &Value) -> String {
    view.to_string()
}

/// The form, the walk and the table, end to end.
#[test]
fn a_search_runs_and_comes_back_with_a_table() {
    let tree = Tree::new("basic");
    let view = search_for(&tree, None, json!({ "dict": "invoice" }));
    assert_eq!(names(&view), ["Invoice-2024.pdf", "invoice.txt"]);
    assert!(said(&view).contains("2 found"), "it should say what it did");
}

/// The brief's case, through the whole module: naming a folder and its own
/// parent searches once, and says where the second went.
#[test]
fn a_path_inside_another_is_searched_once_and_said_so() {
    let tree = Tree::new("overlap");
    let view = search_for(&tree, Some("projects/project2"), json!({ "dict": "invoice" }));
    // Not twice, though project2 was named as well.
    assert_eq!(names(&view), ["Invoice-2024.pdf", "invoice.txt"]);
}

/// Dates, kinds and sizes narrow what the words found.
#[test]
fn the_filters_narrow_what_the_dictionary_found() {
    let tree = Tree::new("narrow");
    let archives = search_for(&tree, None, json!({ "dict": "", "kind": "Archives" }));
    assert_eq!(names(&archives), ["old.zip"]);

    let big = search_for(&tree, None, json!({ "dict": "", "kind": "Files", "larger_than": "1 KB" }));
    assert_eq!(big, big.clone());
    assert_eq!(names(&big), ["Invoice-2024.pdf", "old.zip"]);

    // Nothing was made before the epoch, so this finds nothing — and says so
    // rather than showing an empty table.
    let none = search_for(&tree, None, json!({ "dict": "", "changed_before": "1971-01-01" }));
    assert!(names(&none).is_empty());
    assert!(said(&none).contains("Nothing matched"), "{}", said(&none));
}

/// A search of nowhere is a screen saying so, not an empty table and not a
/// crash.
#[test]
fn asking_for_a_path_that_is_not_there_says_so() {
    let mut module = Search::default();
    let view = module.start(
        &json!({ "paths": "/definitely/not/here", "dict": "x" }),
        "en",
    );
    assert!(view.get("auto").is_none(), "there is nothing to wait for");
    assert!(said(&view).contains("Nowhere to look"), "{}", said(&view));
    assert!(said(&view).contains("/definitely/not/here"), "it names the path");
}

/// What was typed is still there afterwards: a search that fails should come
/// back to the form somebody filled in, not an empty one.
#[test]
fn the_form_remembers_what_was_asked() {
    let mut module = Search::default();
    module.start(&json!({ "paths": "/definitely/not/here", "dict": "invoice" }), "en");
    let again = module.invoke_for_test("ui");
    assert!(said(&again).contains("invoice"), "the dictionary came back");
    assert!(said(&again).contains("/definitely/not/here"), "and the path");
}

/// thing `invoke` would have learned from one.
#[test]
fn a_dictionary_can_be_kept_and_brought_back() {
    let dir = std::env::temp_dir().join("limen-search-keep");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("somewhere to keep it");
    let place = dir.display().to_string();

    let mut module = Search::default();
    module.keep_things_in(&place);

    let terms = "# from the ticket\ninvoice\n\"annual report\"\n";
    let kept = module.save_for_test(
        &json!({ "name": "Exfil sweep", "dict": terms, "paths": "/srv/data" }),
        &place,
        "en",
    );
    let said = kept.to_string();
    assert!(said.contains("Saved the dictionary as Exfil sweep"), "{said}");
    assert!(said.contains("Saved dictionaries"), "it is offered by name: {said}");

    // Type something else over it, then ask for the kept one back.
    module.start(&json!({ "paths": "/elsewhere", "dict": "nothing" }), "en");
    let back = module.load_for_test("Exfil sweep", &place, "en");
    let said = back.to_string();
    assert!(said.contains("Loaded Exfil sweep"), "{said}");
    assert!(said.contains("2 terms"), "and says how many: {said}");
    assert!(said.contains("annual report"), "the dictionary came back: {said}");
    // The places are the machine's, not the dictionary's — loading one does
    // not reach over and change where somebody was about to look.
    assert!(said.contains("/elsewhere"), "the paths were left alone: {said}");

    let gone = module.forget_for_test("Exfil sweep", &place, "en");
    assert!(gone.to_string().contains("Forgot Exfil sweep"), "{gone}");
    assert!(
        !gone.to_string().contains("Saved dictionaries"),
        "the list should be gone with it: {gone}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A chosen folder joins the list of places, which is the one list that is
/// actually searched.
///
/// The chooser itself belongs to the host, so what is driven here is what
/// happens to the list once a folder has come back from it.
#[test]
fn a_chosen_folder_joins_the_list() {
    let mut module = Search::default();
    module.start(&json!({ "paths": "/srv/data", "dict": "x" }), "en");

    let first = module.append_for_test("/var/log", "en");
    assert!(first.to_string().contains("Added /var/log"), "{first}");
    assert_eq!(listed_paths(&first), ["/srv/data", "/var/log"]);

    // The same folder twice is one line, and is said to be rather than looking
    // like a button that did nothing.
    let again = module.append_for_test("/var/log", "en");
    assert!(again.to_string().contains("already in the list"), "{again}");
    assert_eq!(listed_paths(&again), ["/srv/data", "/var/log"]);
}

/// A path typed into the box is checked before it joins the list: a folder
/// that is there is added, and anything else is refused with a reason.
#[test]
fn a_typed_path_is_checked_before_it_is_added() {
    let tree = Tree::new("typed");
    let here = tree.0.display().to_string();
    let mut module = Search::default();

    let added = module.add_typed_for_test(&here, "en");
    assert!(added.to_string().contains("Added"), "{added}");
    let canonical = crate::roots::tidy(std::fs::canonicalize(&here).unwrap());
    assert_eq!(listed_paths(&added), [canonical.display().to_string()]);
    // Written the way somebody would write it. Canonical on Windows means the
    // extended-length spelling, and a list headed `\\?\C:\...` is a list of
    // paths nobody recognises as their own.
    for shown in listed_paths(&added) {
        assert!(!shown.starts_with("\\\\?\\"), "shown verbatim: {shown}");
    }
    // The box it was typed into is empty again, or the next press adds it twice.
    assert!(
        !added.to_string().contains(&format!("\"id\":\"path\",\"default\":\"{here}")),
        "the box kept what is now in the list: {added}"
    );

    let missing = module.add_typed_for_test("/definitely/not/here", "en");
    assert!(missing.to_string().contains("is not there"), "{missing}");

    let a_file = tree.0.join("logs/old.zip").display().to_string();
    let refused = module.add_typed_for_test(&a_file, "en");
    assert!(refused.to_string().contains("not a folder"), "{refused}");

    // Neither of the two refusals left anything behind.
    assert_eq!(listed_paths(&refused).len(), 1, "{refused}");
}

/// And one can be taken back out, by the row it is on.
#[test]
fn a_path_can_be_removed_from_the_list() {
    // Paths that are not there, so filling the form does not start a walk
    // across this machine — what is driven here is the list, not the search.
    let mut module = Search::default();
    module.start(&json!({ "paths": "/nope/a\n/nope/b\n/nope/c", "dict": "x" }), "en");

    let gone = module.drop_path_for_test("1", "en");
    assert!(gone.to_string().contains("Removed /nope/b"), "{gone}");
    assert_eq!(listed_paths(&gone), ["/nope/a", "/nope/c"]);

    // A row that is no longer there changes nothing — the list can have been
    // edited while a menu stood open on it.
    let nothing = module.drop_path_for_test("9", "en");
    assert_eq!(listed_paths(&nothing), ["/nope/a", "/nope/c"]);

    for id in ["1", "0"] {
        module.drop_path_for_test(id, "en");
    }
    let empty = module.invoke_for_test("ui");
    assert!(listed_paths(&empty).is_empty(), "{empty}");
    assert!(empty.to_string().contains("No paths yet"), "{empty}");
}

/// The dictionary is built the same way the paths are: a term is typed,
/// checked, and joins a list that can be taken from.
#[test]
fn a_term_is_checked_before_it_joins_the_dictionary() {
    let mut module = Search::default();

    let added = module.add_term_for_test("invoice", false, "en");
    assert!(added.to_string().contains("Added invoice"), "{added}");
    assert_eq!(listed_terms(&added), [("invoice".into(), "Word".into())]);

    // A phrase is quoted for it: how a term is written is the module's job.
    let phrase = module.add_term_for_test("annual report", false, "en");
    assert_eq!(
        listed_terms(&phrase).last().cloned(),
        Some(("annual report".into(), "Phrase".into())),
        "{phrase}"
    );

    // A pattern is compiled before it is accepted — a bracket missing should
    // be refused while it is typed, not three minutes into a walk.
    let bad = module.add_term_for_test("^IMG_[0-9", true, "en");
    assert!(
        bad.to_string().contains("not a valid regular expression"),
        "{bad}"
    );
    assert_eq!(listed_terms(&bad).len(), 2, "nothing was left behind: {bad}");

    let good = module.add_term_for_test("^IMG_\\d{4}\\.jpe?g$", true, "en");
    assert_eq!(
        listed_terms(&good).last().cloned(),
        Some(("^IMG_\\d{4}\\.jpe?g$".into(), "Pattern".into())),
        "{good}"
    );

    // The same term twice is one term, and is said to be.
    let again = module.add_term_for_test("invoice", false, "en");
    assert!(again.to_string().contains("already in the dictionary"), "{again}");
    assert_eq!(listed_terms(&again).len(), 3);

    // And one can be taken back out.
    let gone = module.drop_term_for_test("0", "en");
    assert!(gone.to_string().contains("Removed invoice"), "{gone}");
    assert_eq!(listed_terms(&gone).len(), 2);
}

/// What is in the dictionary is what the search actually uses — the table is
/// the same text the walk is built from, not a second copy of it.
#[test]
fn the_dictionary_on_screen_is_the_one_that_is_searched() {
    let tree = Tree::new("terms");
    let mut module = Search::default();
    module.add_term_for_test("invoice", false, "en");
    let dict = module
        .form_for_test("dict")
        .expect("the dictionary the form holds");
    let view = search_for(&tree, None, json!({ "dict": dict }));
    assert_eq!(names(&view), ["Invoice-2024.pdf", "invoice.txt"]);
}

/// A tree with more hits in it than one page holds.
struct BigTree(PathBuf);

impl BigTree {
    /// Named, because the tests run at the same time and two of them sharing a
    /// directory is two searches of each other's files.
    fn new(name: &str, n: usize) -> Self {
        let root = std::env::temp_dir().join(format!("limen-search-big-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("the test tree");
        for i in 0..n {
            std::fs::write(root.join(format!("invoice-{i:04}.txt")), b"x").expect("a test file");
        }
        BigTree(root)
    }
}

impl Drop for BigTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run a search and leave the module holding its results, so the pages can be
/// turned.
fn finished(module: &mut Search, form: Value) {
    module.start(&form, "en");
    for _ in 0..4_000 {
        let view = module.poll("en");
        if view.get("auto").is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the search never finished");
}

/// Results come a hundred at a time, and the pages can be turned.
#[test]
fn a_long_list_of_results_is_paged() {
    let tree = BigTree::new("pages", 250);
    let mut module = Search::default();
    finished(
        &mut module,
        json!({ "paths": tree.0.display().to_string(), "dict": "invoice" }),
    );

    let first = module.invoke_for_test("ui");
    assert!(first.to_string().contains("250 found"), "{first}");
    assert_eq!(names(&first).len(), 100, "a page is a hundred rows");
    assert!(first.to_string().contains("Page 1 of 3"), "{first}");
    assert!(first.to_string().contains("Showing 1–100 of 250"), "{first}");
    assert!(names(&first).contains(&"invoice-0000.txt".to_string()));

    let third = module.page_for_test(2, "en");
    assert_eq!(names(&third).len(), 50, "the last page is the remainder");
    assert!(third.to_string().contains("Page 3 of 3"), "{third}");
    assert!(names(&third).contains(&"invoice-0249.txt".to_string()));
    assert!(
        !names(&third).contains(&"invoice-0000.txt".to_string()),
        "the first page is not still on screen"
    );

    // A row on a later page still answers: its id is its place in the whole
    // list, not in the page it was drawn on.
    let id = row_ids(&third).last().cloned().expect("a row on page three");
    let detail = module.about(&json!({ "id": id }), "en");
    assert!(
        detail.to_string().contains("invoice-"),
        "the details of a row from page three: {detail}"
    );

    // Asking for a page past the end lands on the last one rather than on an
    // empty table.
    let past = module.page_for_test(99, "en");
    assert!(past.to_string().contains("Page 3 of 3"), "{past}");

    // And a search that fits on one page is not given controls for pages it
    // does not have.
    let small = Tree::new("onepage");
    let mut module = Search::default();
    finished(
        &mut module,
        json!({ "paths": small.0.display().to_string(), "dict": "invoice" }),
    );
    let only = module.invoke_for_test("ui");
    assert!(!only.to_string().contains("Page 1 of"), "{only}");
}

/// Every result says what found it, which is the first question asked of a
/// list produced by twenty terms at once.
#[test]
fn each_result_says_which_term_found_it() {
    let tree = Tree::new("why");
    let view = search_for(
        &tree,
        None,
        json!({ "dict": "invoice\n/\\.zip$/\n\"notes\"" }),
    );
    let found = found_by(&view);
    assert_eq!(
        found.get("Invoice-2024.pdf").map(String::as_str),
        Some("invoice"),
        "{found:?}"
    );
    assert_eq!(
        found.get("old.zip").map(String::as_str),
        Some("/\\.zip$/"),
        "a pattern is quoted as the dictionary writes it: {found:?}"
    );
    assert_eq!(
        found.get("notes.md").map(String::as_str),
        Some("\"notes\""),
        "{found:?}"
    );

    // A name two terms agree on says both of them.
    let both = search_for(&tree, None, json!({ "dict": "invoice\n/2024/" }));
    assert_eq!(
        found_by(&both).get("Invoice-2024.pdf").map(String::as_str),
        Some("invoice, /2024/"),
        "{both}"
    );

    // With an empty dictionary nothing in particular was being looked for, and
    // the column says nothing rather than "everything".
    let all = search_for(&tree, None, json!({ "dict": "", "kind": "Archives" }));
    assert_eq!(found_by(&all).get("old.zip").map(String::as_str), Some(""));
}

/// The results are on screen while the search is still running.
///
/// A search of a disk takes minutes, and for all of that time the screen had
/// nothing to say but "looking" — while the answer was already arriving, hit
/// by hit, into a table nobody could see.
///
/// Driven from the state a walk is in a second after it starts, rather than by
/// starting one and hoping to catch it: a tree small enough for a test is
/// searched in three polls on a warm cache, and a test that waits for the
/// right moment is a test that fails on a fast machine.
#[test]
fn what_is_found_is_shown_while_it_is_still_looking() {
    let mut module = Search::default();
    module.looking_at_for_test(found_so_far(250));

    let view = module.poll("en");
    // Still looking, still asking, still stoppable — and asking at a pace
    // rather than as fast as it can be answered: the walk is the work, and a
    // screen that reports on it must leave the machine to do it.
    let auto = view.get("auto").unwrap_or_else(|| panic!("it stopped asking: {view}"));
    assert_eq!(auto["method"], "poll");
    assert!(
        auto["every_ms"].as_u64().is_some_and(|ms| (50..=1000).contains(&ms)),
        "a few times a second: {auto}"
    );
    let said = view.to_string();
    assert!(said.contains("Looking"), "{said}");
    assert!(said.contains("Stop"), "{said}");

    // And the table is there, a page of it, with the counts beside it.
    assert_eq!(names(&view).len(), 100, "a page of what it has found");
    assert!(said.contains("250 found"), "it says how many so far: {said}");
    assert!(said.contains("Page 1 of 3"), "{said}");
    assert!(names(&view).contains(&"found-000.txt".to_string()));
}

/// A search that has found nothing yet shows no table at all — an empty one
/// under a spinner says "nothing matched" about a question not yet answered.
#[test]
fn an_empty_search_in_flight_shows_no_table() {
    let mut module = Search::default();
    module.looking_at_for_test(found_so_far(0));
    let view = module.poll("en");
    assert!(view.get("auto").is_some());
    assert!(names(&view).is_empty(), "{view}");
    assert!(
        !view.to_string().contains("Nothing matched"),
        "it answered a question it has not finished asking: {view}"
    );
}

/// A table of `n` hits, as a walk half way through one would hold.
fn found_so_far(n: usize) -> crate::walk::Found {
    crate::walk::Found {
        hits: (0..n)
            .map(|i| crate::walk::Hit {
                path: std::path::PathBuf::from(format!("/tmp/found-{i:03}.txt")),
                name: format!("found-{i:03}.txt"),
                kind: crate::filter::Kind::File,
                size: 1,
                created: None,
                changed: None,
                why: vec!["found".into()],
                inside: Vec::new(),
            })
            .collect(),
        dirs: 3,
        seen: n * 2,
        ..Default::default()
    }
}

/// And the pages can be turned while it is still looking.
///
/// The same door as above, for the same reason: catching a real walk between
/// two pages is a race, and a test that waits for the right moment fails on a
/// machine quick enough not to have one.
#[test]
fn the_pages_turn_while_it_is_still_looking() {
    let mut module = Search::default();
    module.looking_at_for_test(found_so_far(250));
    // The first page, so the module is holding one.
    module.poll("en");

    let two = module.page_for_test(1, "en");
    assert!(two.get("auto").is_some(), "it stopped looking: {two}");
    assert!(two.to_string().contains("Page 2 of 3"), "{two}");
    assert!(two.to_string().contains("Looking"), "{two}");
    assert!(names(&two).contains(&"found-100.txt".to_string()), "{two}");

    // Asking for a page past what has been found so far lands on the last one
    // it has, rather than on an empty table that fills in later.
    let past = module.page_for_test(99, "en");
    assert!(past.to_string().contains("Page 3 of 3"), "{past}");
    assert_eq!(names(&past).len(), 50);

    // And the page somebody turned to is still the page when the next poll
    // comes round — a search filling up must not drag the reader back to the
    // top of it.
    let again = module.poll("en");
    assert!(again.to_string().contains("Page 3 of 3"), "{again}");
}

/// Once Stop has been pressed the screen says so until the walk has stopped.
///
/// It said it for one frame and then went back to "Looking…" — the next poll
/// is a tenth of a second later and knew nothing about the button — so a stop
/// that was working looked like one that had been forgotten, on a walk that
/// then ran for seconds more.
#[test]
fn a_search_told_to_stop_goes_on_saying_so() {
    let mut module = Search::default();
    module.looking_at_for_test(found_so_far(120));

    let pressed = module.stop_for_test("en");
    assert!(pressed.to_string().contains("Stopping"), "{pressed}");
    assert!(
        !pressed.to_string().contains("\"text\":\"Stop\""),
        "the button is still offered: {pressed}"
    );
    // What it found is still on screen: the walk is winding down, not undone.
    assert_eq!(names(&pressed).len(), 100);

    // Every poll from here to the end says the same, however many come.
    for _ in 0..5 {
        let view = module.poll("en");
        assert!(view.get("auto").is_some(), "it stopped asking: {view}");
        let said = view.to_string();
        assert!(said.contains("Stopping"), "{said}");
        assert!(!said.contains("Looking"), "it went back to looking: {said}");
    }
}

/// Stop is answered at once, and the search really does end.
///
/// The screen cannot show results the moment the button is pressed — the walk
/// has to notice — so what it must do instead is say it is stopping, and then
/// actually stop.
#[test]
fn stop_is_answered_and_the_walk_ends() {
    let tree = BigTree::new("stop", 400);
    let mut module = Search::default();
    module.start(
        &json!({ "paths": tree.0.display().to_string(), "dict": "invoice" }),
        "en",
    );

    let stopping = module.stop_for_test("en");
    assert!(stopping.to_string().contains("Stopping"), "{stopping}");
    assert!(
        stopping.get("auto").is_some(),
        "it still has to ask how it went: {stopping}"
    );

    // And it ends — promptly, including the workers that were asleep waiting
    // for a directory nobody is going to push.
    let waited = std::time::Instant::now();
    loop {
        let view = module.poll("en");
        if view.get("auto").is_none() {
            break;
        }
        assert!(
            waited.elapsed() < std::time::Duration::from_secs(5),
            "the walk did not stop"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!module.is_running());
}

/// A walk that dies says so, instead of leaving the screen spinning on a
/// sender that no longer exists.
#[test]
fn a_walk_that_dies_is_not_waited_on_for_ever() {
    let mut module = Search::default();
    module.orphan_for_test();
    let view = module.poll("en");
    assert!(
        view.get("auto").is_none(),
        "it must stop asking: {view}"
    );
    assert!(
        view.to_string().contains("stopped before it finished"),
        "{view}"
    );
    assert!(!module.is_running(), "and it is not still running");
    // The form is back, not a dead end.
    assert!(view.to_string().contains("Where to look"), "{view}");
}

/// A real search of a real tree, run by hand: `cargo test -- --ignored live`.
///
/// Points at this checkout and looks for the things every Limen module has, to
/// show the numbers a screen would show.
#[test]
#[ignore = "walks a real tree; run with --ignored"]
fn live() {
    let here = std::env::var("LIMEN_SEARCH_AT")
        .unwrap_or_else(|_| std::env::current_dir().unwrap().display().to_string());
    let mut module = Search::default();
    let started = std::time::Instant::now();
    let threads = std::env::var("LIMEN_SEARCH_THREADS").unwrap_or_default();
    module.start(
        &json!({ "paths": here, "threads": threads,
                 "dict": "/^limen\\.toml$/\n\"package.sh\"\nLICENSE" }),
        "en",
    );
    let view = loop {
        let v = module.poll("en");
        if v.get("auto").is_none() {
            break v;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let found = names(&view);
    println!("{} hits in {:?}", found.len(), started.elapsed());
    for w in view["widgets"].as_array().unwrap() {
        if w["kind"] == "label" && w["text"].as_str().is_some_and(|t| t.contains("found")) {
            println!("{}", w["text"].as_str().unwrap());
        }
    }
    println!("{:?}", found.iter().take(8).collect::<Vec<_>>());
}

/// How long Stop takes on a real tree, with the screen polling as it does.
/// `cargo test --release -- --ignored live_stop --nocapture`
#[test]
#[ignore = "walks a real tree; run with --ignored"]
fn live_stop() {
    let at = std::env::var("LIMEN_SEARCH_AT")
        .unwrap_or_else(|_| std::env::current_dir().unwrap().display().to_string());
    let mut module = Search::default();
    module.start(&json!({ "paths": at, "dict": "/./" }), "en");

    // Poll as the screen does — building the whole view each time, which is
    // what the UI thread actually spends its time on.
    let started = std::time::Instant::now();
    let mut polls = 0;
    while started.elapsed() < std::time::Duration::from_millis(400) {
        module.poll("en");
        polls += 1;
    }
    println!("  {polls} polls in {:?} before Stop", started.elapsed());

    let pressed = std::time::Instant::now();
    module.stop_for_test("en");
    let mut after = 0;
    loop {
        let view = module.poll("en");
        after += 1;
        if view.get("auto").is_none() {
            break;
        }
        if pressed.elapsed() > std::time::Duration::from_secs(30) {
            panic!("it did not stop in 30s ({after} polls)");
        }
    }
    println!("  stopped in {:?} after {after} polls", pressed.elapsed());
}
