//! Which paths are actually walked, when somebody names overlapping ones.

use crate::roots::prune;
use std::path::PathBuf;

fn paths(given: &[&str]) -> Vec<PathBuf> {
    given.iter().map(PathBuf::from).collect()
}

/// The case from the brief: a folder named beside its own parent is walked
/// once, as part of the parent.
#[test]
fn a_folder_inside_another_is_not_walked_twice() {
    let r = prune(paths(&["D:/dev/projects", "D:/dev/projects/project1"]));
    assert_eq!(r.keep, paths(&["D:/dev/projects"]));
    assert_eq!(r.covered.len(), 1, "it should say where the other went");
    assert_eq!(r.covered[0].0, PathBuf::from("D:/dev/projects/project1"));
    assert_eq!(r.covered[0].1, PathBuf::from("D:/dev/projects"));
}

/// Whichever order they were typed in.
#[test]
fn the_parent_wins_whichever_was_named_first() {
    let child_first = prune(paths(&["D:/dev/projects/project1", "D:/dev/projects"]));
    assert_eq!(child_first.keep, paths(&["D:/dev/projects"]));
    assert_eq!(child_first.covered.len(), 1);
}

/// Deep nesting collapses to the outermost, in one pass.
#[test]
fn a_whole_chain_collapses_to_its_outermost() {
    let r = prune(paths(&["/a/b/c/d", "/a", "/a/b", "/a/b/c"]));
    assert_eq!(r.keep, paths(&["/a"]));
    assert_eq!(r.covered.len(), 3);
    assert!(r.covered.iter().all(|(_, by)| by == &PathBuf::from("/a")));
}

/// The same path twice is one path — and is said to be, rather than quietly
/// dropped.
#[test]
fn a_repeat_is_caught_as_a_repeat() {
    let r = prune(paths(&["/srv/data", "/srv/data"]));
    assert_eq!(r.keep, paths(&["/srv/data"]));
    assert_eq!(r.covered.len(), 1);
}

/// Compared by path component, not by text. `/a/bc` is not inside `/a/b`, and a
/// search that dropped it would silently miss everything in it.
#[test]
fn a_name_that_merely_starts_the_same_is_its_own_path() {
    let r = prune(paths(&["/a/b", "/a/bc"]));
    assert_eq!(r.keep, paths(&["/a/b", "/a/bc"]));
    assert!(r.covered.is_empty(), "{:?}", r.covered);
}

/// Siblings are all kept, in the order they were asked for.
#[test]
fn unrelated_paths_are_all_kept_in_the_order_given() {
    let given = paths(&["/var/log", "/home/me", "/srv"]);
    let r = prune(given.clone());
    assert_eq!(r.keep, given);
    assert!(r.covered.is_empty());
}

/// Nothing in, nothing out — and no panic on the way.
#[test]
fn nothing_is_not_an_error() {
    let r = prune(Vec::new());
    assert!(r.keep.is_empty() && r.covered.is_empty());
}
