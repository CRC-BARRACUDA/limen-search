//! `search` — a native Limen module that finds things on **this** machine.
//!
//! Provides `search.local`. Given a list of places and a dictionary of what to
//! look for, it walks those places on several threads and reports every file,
//! folder and archive whose name matches, narrowed by kind, size and date.
//!
//! Three things it does that a one-line `find` does not:
//!
//! - **A dictionary.** Words, phrases and regular expressions together, one per
//!   line, the way a list of things to look for is actually written down. See
//!   [`dict`].
//! - **Paths that overlap.** `D:/dev/projects` and `D:/dev/projects/project1`
//!   are one walk, not two, and it says so rather than quietly dropping the
//!   second. See [`roots`].
//! - **Threads.** A search is nearly all waiting on the filesystem, so the
//!   directories still to read are shared out among workers. See [`walk`].
//!
//! Nothing is sent anywhere, nothing is run, and nothing outside the paths the
//! user named is read.
//!
//! Built as a native (`cdylib`) module using `limen-sdk-rust`.

pub(crate) use limen_sdk_rust::ui::{
    button, checkbox, date, label, menu_item, row, select, separator, step, table, text, window,
};
pub(crate) use limen_sdk_rust::{export_module, json, rpc, Catalog, Handler, Host, RpcError, Value};

/// The capability this module provides. Named once: it is also the target every
/// button on every screen calls back into.
pub(crate) const CAP: &str = "search.local";

/// How many hits one search will hold.
///
/// A dictionary of `/./` across a whole disk would otherwise fill memory with a
/// list nobody could read. The number is said on screen when it is reached, so
/// a truncated answer never passes for a complete one.
pub(crate) const CAP_HITS: usize = 5_000;

/// Every word this module shows, in each language it has.
///
/// English lives in a file beside the Ukrainian rather than in the code: a
/// string written into the source is a string nobody can translate.
pub(crate) fn catalog() -> &'static Catalog {
    static C: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        Catalog::new(&[
            ("en", include_str!("../locales/en.toml")),
            ("uk", include_str!("../locales/uk.toml")),
        ])
    })
}

/// Whether a dropdown's answer is this choice, in whichever language it was
/// shown in.
///
/// An option is its own value — a person reading Ukrainian sends Ukrainian
/// back — so the answer is compared against every language rather than against
/// the English it used to be.
pub(crate) fn chose(answer: &str, key: &str) -> bool {
    ["en", "uk"].iter().any(|lang| catalog().tr(lang, key) == answer)
}

mod book;
mod dict;
mod filter;
mod handler;
mod peek;
mod roots;
pub(crate) mod view;
mod walk;

#[cfg(test)]
mod tests;

// This reads best as one namespace: each part takes `use crate::*` and finds
// everything, rather than every file carrying a list of its neighbours that has
// to be maintained by hand.
pub(crate) use handler::*;

export_module!(Search);
