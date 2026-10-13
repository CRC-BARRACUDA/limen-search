//! The catalogue, and that both languages actually say everything.

use crate::*;
use std::collections::HashMap;

/// Every `a.b` key a locale file defines, read from the file rather than
/// through the catalogue: `tr` falls back to English for a key Ukrainian is
/// missing, so asking it would hide exactly what this is looking for.
fn keys(src: &str) -> Vec<String> {
    let mut table = String::new();
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            table = name.to_string();
        } else if let Some((key, _)) = line.split_once(" = ") {
            out.push(format!("{table}.{key}"));
        }
    }
    out
}

#[test]
fn both_languages_say_the_same_things() {
    let en = keys(include_str!("../../locales/en.toml"));
    let uk = keys(include_str!("../../locales/uk.toml"));
    assert!(en.len() > 40, "the catalogue is suspiciously small");
    for key in &en {
        assert!(uk.contains(key), "uk.toml is missing {key}");
    }
    for key in uk.iter().filter(|k| !k.starts_with("module.")) {
        assert!(en.contains(key), "en.toml is missing {key}");
    }
}

/// A screen asked for in Ukrainian comes back in Ukrainian — not a mix, and not
/// English with a translated title.
#[test]
fn the_screens_are_translated_not_merely_titled() {
    let mut rows = HashMap::new();
    let form = view::main("uk", &HashMap::new(), None, &mut rows, &[], None, 0).to_string();
    for word in ["Де шукати", "Словник", "Шукати", "Архіви", "Створено після", "Назва словника", "Імпортувати…", "Додати шлях"] {
        assert!(form.contains(word), "{word} is missing from the form");
    }
    assert!(!form.contains("Dictionary"), "English survived: {form}");

    let waiting = view::waiting("uk", false, None, &mut rows, 0).to_string();
    assert!(waiting.contains("Пошук…") && waiting.contains("Зупинити"), "{waiting}");
    // And the screen after Stop says so in Ukrainian too, without offering a
    // button that has already been pressed.
    let stopping = view::waiting("uk", true, None, &mut rows, 0).to_string();
    assert!(stopping.contains("Зупиняємо…"), "{stopping}");
    assert!(!stopping.contains("Зупинити"), "the button is still there: {stopping}");
}

/// A dropdown's answer comes back in the language it was shown in, so the
/// module has to recognise its own words — in either language, because a search
/// saved or scripted elsewhere may still say "Archives".
#[test]
fn a_choice_is_understood_in_the_language_it_was_made_in() {
    assert!(chose("Архіви", "kind.archives"));
    assert!(chose("Archives", "kind.archives"));
    assert!(!chose("Теки", "kind.archives"));
}
