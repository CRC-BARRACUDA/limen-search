//! Every screen this module draws.

use crate::dict::Dictionary;
use crate::filter::Kind;
use crate::roots::Roots;
use crate::walk::{Found, Hit};
use crate::*;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// The form, and the last answer under it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn main(
    lang: &str,
    asked: &HashMap<String, String>,
    found: Option<&Found>,
    rows: &mut HashMap<String, Hit>,
    saved: &[String],
    note: Option<&str>,
    page: usize,
) -> Value {
    let t = |k: &str| catalog().tr(lang, k);
    let was = |k: &str| asked.get(k).cloned().unwrap_or_default();

    let mut w = Vec::new();
    // What just happened, once, above the form it happened to.
    if let Some(note) = note {
        w.push(label(note).strong());
        w.push(separator());
    }
    w.push(label(t("ui.where")).strong());
    w.extend(places(lang, &was("paths")));
    // One box and one button for both ways of naming a folder: typed in, or
    // browsed for when the box is empty. A field to pick into beside a field
    // to type into would be two answers to the same question, and two places
    // to read before knowing what will be searched.
    w.extend(vec![
        row(vec![
            text("path")
                .label(t("ui.path"))
                .placeholder(t("ui.path_ph"))
                .default(was("path")),
            button(t("ui.add"), CAP, "add_path"),
        ]),
        label(t("ui.add_help")).weak(),
        separator(),
        label(t("ui.what")).strong(),
    ]);
    w.extend(terms(lang, &was("dict")));
    // One line to a term, as the dictionary itself is written — and the
    // checkbox says which of the two kinds of term is being written, so that
    // the punctuation is the module's job rather than the reader's.
    w.extend(vec![
        row(vec![
            text("term")
                .label(t("ui.term"))
                .placeholder(t("ui.term_ph"))
                .default(was("term")),
            tick("term_regex", t("ui.regex"), &was("term_regex")),
            button(t("ui.add_term"), CAP, "add_term"),
        ]),
        label(t("ui.dict_help")).weak(),
        // Looking inside is a different order of work from matching names, and
        // the screen says so rather than presenting it as another tick box of
        // the same weight. See `peek`.
        row(vec![
            tick("inside", t("ui.inside"), &was("inside")),
            tick("in_zip", t("ui.in_zip"), &was("in_zip")),
        ]),
        label(t("ui.inside_help")).weak(),
    ]);
    w.extend(keeping(lang, asked, saved));
    w.extend(vec![
        separator(),
        label(t("ui.narrow")).strong(),
        row(vec![
            select(
                "kind",
                vec![t("kind.any"), t("kind.files"), t("kind.dirs"), t("kind.archives")],
            )
            .label(t("ui.kind"))
            .default(if was("kind").is_empty() { t("kind.any") } else { was("kind") }),
            text("larger_than")
                .label(t("ui.larger"))
                .placeholder("10 MB")
                .default(was("larger_than")),
            text("smaller_than")
                .label(t("ui.smaller"))
                .placeholder("1 GB")
                .default(was("smaller_than")),
            // Normally left alone. It is here because a search that is clearly
            // using the machine should be able to be told not to: a walk at
            // full tilt competes with whatever else is reading the same disk.
            select(
                "threads",
                vec![t("threads.auto"), "1".into(), "2".into(), "4".into(), "8".into(), "16".into()],
            )
            .label(t("ui.threads"))
            .default(if was("threads").is_empty() { t("threads.auto") } else { was("threads") }),
        ]),
        row(vec![
            date("created_after").label(t("ui.created_after")).default(was("created_after")),
            date("created_before").label(t("ui.created_before")).default(was("created_before")),
        ]),
        row(vec![
            date("changed_after").label(t("ui.changed_after")).default(was("changed_after")),
            date("changed_before").label(t("ui.changed_before")).default(was("changed_before")),
        ]),
        separator(),
        row(vec![button(t("ui.search"), CAP, "search").primary()]),
    ]);

    // A search with no dictionary and nothing narrowing it will report every
    // name under every path — which is a thing somebody may well want, and a
    // thing nobody wants by accident.
    let dict = crate::dict::parse(&was("dict"));
    let filters = asked_filters(asked);
    if dict.is_empty() && filters.is_empty() {
        w.insert(w.len() - 1, label(t("ui.unbounded")).weak());
    }

    if let Some(found) = found {
        w.push(separator());
        w.extend(results(lang, found, rows, page));
    }
    window(t("ui.title"), w)
}

/// The paths that will be walked, as a list that can be taken from.
///
/// A table rather than a box of lines: these are the places about to be read,
/// and each one is a thing in its own right — to be looked at, and removed
/// without editing a neighbour by accident.
fn places(lang: &str, paths: &str) -> Vec<limen_sdk_rust::ui::Widget> {
    let t = |k: &str| catalog().tr(lang, k);
    let lines: Vec<&str> = paths
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return vec![label(t("paths.none")).weak()];
    }
    vec![
        table(
            vec![t("col.path")],
            lines.iter().map(|l| vec![l.to_string()]).collect(),
        )
        // The row's place in the list, which is what the module removes by.
        .row_ids((0..lines.len()).map(|i| i.to_string()).collect())
        .row_menu(vec![menu_item(t("paths.remove"), CAP, "drop_path")]),
        label(t("paths.hint")).weak(),
    ]
}

/// A checkbox that comes back ticked if it was ticked.
///
/// `default` on the wire is a boolean for a checkbox and a string for
/// everything else, so a box cannot be restored the way a text field is.
fn tick(id: &str, label: String, was: &str) -> limen_sdk_rust::ui::Widget {
    let box_ = checkbox(id, label);
    if was == "true" {
        box_.checked()
    } else {
        box_
    }
}

/// What the search is looking for, as a list that can be taken from.
fn terms(lang: &str, dict: &str) -> Vec<limen_sdk_rust::ui::Widget> {
    let t = |k: &str| catalog().tr(lang, k);
    let lines: Vec<&str> = dict
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return vec![label(t("dict.none")).weak()];
    }
    vec![
        table(
            vec![t("col.term"), t("col.kind")],
            lines
                .iter()
                .map(|l| {
                    vec![
                        crate::dict::reading(l),
                        t(&format!("term.{}", crate::dict::kind_of(l))),
                    ]
                })
                .collect(),
        )
        .row_ids((0..lines.len()).map(|i| i.to_string()).collect())
        .row_menu(vec![menu_item(t("dict.remove"), CAP, "drop_term")]),
        label(t("dict.hint")).weak(),
    ]
}

/// Keeping the dictionary, and carrying one somewhere else.
///
/// Beneath the dictionary rather than at the end of the screen: it is this box
/// that is being saved, and the controls for a thing belong beside it.
fn keeping(
    lang: &str,
    asked: &HashMap<String, String>,
    saved: &[String],
) -> Vec<limen_sdk_rust::ui::Widget> {
    let t = |k: &str| catalog().tr(lang, k);
    let mut w = vec![row(vec![
        text("name")
            .label(t("keep.name"))
            .placeholder(t("keep.name_ph"))
            .default(asked.get("name").cloned().unwrap_or_default()),
        button(t("keep.save"), CAP, "save"),
        button(t("keep.export"), CAP, "export"),
        button(t("keep.import"), CAP, "import"),
    ])];

    if !saved.is_empty() {
        w.push(row(vec![
            select("saved", saved.to_vec()).label(t("keep.saved")),
            button(t("keep.load"), CAP, "load"),
            button(t("keep.forget"), CAP, "forget")
                .danger()
                // Named here rather than left to the host: the host's own word
                // for a question it did not ask is "Continue", and this one
                // throws a dictionary away.
                .confirm_labelled(
                    &t("keep.forget_title"),
                    &t("keep.forget_what"),
                    &t("keep.forget"),
                    &t("keep.cancel"),
                ),
        ]));
    }
    w
}

/// The filters as the form currently has them — read here only to tell the
/// person what the search they are about to run will do.
fn asked_filters(asked: &HashMap<String, String>) -> crate::filter::Filters {
    let was = |k: &str| asked.get(k).map(String::as_str).unwrap_or_default();
    crate::filter::Filters {
        kinds: if was("kind").is_empty() || chose(was("kind"), "kind.any") {
            Vec::new()
        } else {
            vec![Kind::File]
        },
        created_after: crate::filter::date(was("created_after"), false),
        created_before: crate::filter::date(was("created_before"), true),
        changed_after: crate::filter::date(was("changed_after"), false),
        changed_before: crate::filter::date(was("changed_before"), true),
        larger_than: crate::filter::size(was("larger_than")),
        smaller_than: crate::filter::size(was("smaller_than")),
    }
}

/// How many results are shown at once.
///
/// A search of a disk answers with thousands, and a table of thousands is one
/// nobody reads to the end of: it scrolls past the point of usefulness, and the
/// toolkit stops animating rows long before it. A hundred is a screenful a few
/// times over — enough to scan, few enough to arrive at once.
pub(crate) const PER_PAGE: usize = 100;

/// How many pages `n` results come to — at least one, so an empty search still
/// has a page to be on.
pub(crate) fn pages(n: usize) -> usize {
    n.div_ceil(PER_PAGE).max(1)
}

/// What the walk came back with, a page at a time.
fn results(
    lang: &str,
    found: &Found,
    rows: &mut HashMap<String, Hit>,
    page: usize,
) -> Vec<limen_sdk_rust::ui::Widget> {
    let t = |k: &str| catalog().tr(lang, k);
    // Where it went, said in numbers: a search that found nothing has to show
    // it actually went somewhere.
    let mut w = vec![label(
        t("found.counted")
            .replace("{hits}", &found.hits.len().to_string())
            .replace("{dirs}", &thousands(found.dirs))
            .replace("{seen}", &thousands(found.seen)),
    )
    .strong()];
    if found.capped {
        w.push(label(t("found.capped").replace("{n}", &CAP_HITS.to_string())).strong());
    }
    if found.denied > 0 {
        w.push(label(t("found.denied").replace("{n}", &found.denied.to_string())).weak());
    }
    // What looking inside came to. Shown whenever anything was read, because a
    // search that looked inside nothing and one that looked inside everything
    // and found nothing are the same empty table.
    if found.kernel > 0 {
        w.push(label(t("found.kernel").replace("{n}", &found.kernel.to_string())).weak());
    }
    if found.looked > 0 || found.binary > 0 || found.too_big > 0 || found.unread > 0 {
        w.push(
            label(
                t("found.read")
                    .replace("{read}", &thousands(found.looked))
                    .replace("{binary}", &thousands(found.binary))
                    .replace("{big}", &thousands(found.too_big))
                    .replace("{unread}", &thousands(found.unread)),
            )
            .weak(),
        );
    }
    if found.hits.is_empty() {
        w.push(label(t("found.none")).weak());
        return w;
    }

    // Every hit is kept by id, not just the page's: a row action carries the id
    // it was drawn with, and a tab opened on page seven must still answer after
    // the list has been paged back to the first.
    rows.clear();
    for (i, hit) in found.hits.iter().enumerate() {
        rows.insert(i.to_string(), hit.clone());
    }

    let last = pages(found.hits.len()) - 1;
    let page = page.min(last);
    let from = page * PER_PAGE;
    let to = (from + PER_PAGE).min(found.hits.len());

    let mut ids = Vec::new();
    let mut table_rows = Vec::new();
    for (i, hit) in found.hits[from..to].iter().enumerate() {
        // Its place in the whole list, so the id means the same thing on every
        // page.
        let id = (from + i).to_string();
        ids.push(id);
        table_rows.push(vec![
            hit.name.clone(),
            // What found it. A dictionary of twenty terms over a disk answers
            // with a list, and "why is this one here" is the first question
            // anybody asks of it.
            terms_that_found(hit).join(", "),
            // And where: in its name, in what it says, or in something it
            // holds. The same term means a different thing in each.
            whereabouts(lang, hit),
            t(&format!("kind.{}", hit.kind.as_str())),
            if hit.kind == Kind::Dir { String::new() } else { bytes(hit.size) },
            stamp(hit.changed),
            hit.path.to_string_lossy().to_string(),
        ]);
    }
    let menu = vec![
        menu_item(t("menu.about"), CAP, "about").open_in_tab(),
        menu_item(t("menu.reveal"), CAP, "open").args(json!({ "target": "reveal" })),
        menu_item(t("menu.open"), CAP, "open").args(json!({ "target": "path" })),
    ];
    w.push(label(t("found.hint")).weak());
    w.push(
        table(
            [
                "col.name", "col.rule", "col.where", "col.kind", "col.size",
                "col.changed", "col.path",
            ]
            .iter()
            .map(|k| t(k))
            .collect(),
            table_rows,
        )
        .row_ids(ids)
        .row_menu(menu)
        .on_activate(CAP, "about"),
    );
    if last > 0 {
        w.extend(paging(lang, page, last, from + 1, to, found.hits.len()));
    }
    w
}

/// Which page this is, and the way to the others.
///
/// Only drawn when there is more than one: a control that can do nothing is a
/// control a reader has to work out is doing nothing.
fn paging(
    lang: &str,
    page: usize,
    last: usize,
    from: usize,
    to: usize,
    all: usize,
) -> Vec<limen_sdk_rust::ui::Widget> {
    let t = |k: &str| catalog().tr(lang, k);
    let go = |key: &str, to: usize, on: bool| {
        button(t(key), CAP, "page")
            .args(json!({ "page": to }))
            .enabled(on)
    };
    vec![
        row(vec![
            go("page.first", 0, page > 0),
            go("page.prev", page.saturating_sub(1), page > 0),
            label(
                t("page.of")
                    .replace("{page}", &(page + 1).to_string())
                    .replace("{pages}", &(last + 1).to_string()),
            )
            .strong(),
            go("page.next", page + 1, page < last),
            go("page.last", last, page < last),
        ]),
        label(
            t("page.showing")
                .replace("{from}", &from.to_string())
                .replace("{to}", &to.to_string())
                .replace("{all}", &all.to_string()),
        )
        .weak(),
    ]
}

/// Every term that found this result, from its name and from inside it, each
/// said once.
pub(crate) fn terms_that_found(hit: &Hit) -> Vec<String> {
    let mut out = hit.why.clone();
    for found in &hit.inside {
        if !out.contains(&found.term) {
            out.push(found.term.clone());
        }
    }
    out
}

/// Where the match was: in the name, in the text, or in a member of the
/// archive or document.
fn whereabouts(lang: &str, hit: &Hit) -> String {
    let t = |k: &str| catalog().tr(lang, k);
    let mut parts = Vec::new();
    if !hit.why.is_empty() {
        parts.push(t("where.name"));
    }
    let mut members: Vec<&str> = Vec::new();
    let mut in_text = false;
    for found in &hit.inside {
        if found.member.is_empty() {
            in_text = true;
        } else if !members.contains(&found.member.as_str()) {
            members.push(&found.member);
        }
    }
    if in_text {
        parts.push(t("where.text"));
    }
    // Two named, then a count: a zip with forty matching members would
    // otherwise be one unreadable cell.
    for member in members.iter().take(2) {
        parts.push(member.to_string());
    }
    if members.len() > 2 {
        parts.push(t("where.more").replace("{n}", &(members.len() - 2).to_string()));
    }
    parts.join(" · ")
}

/// The screen while the walk is running. It asks the module whether it has
/// finished, every frame, so the UI thread only ever draws.
///
/// And it shows what has been found *so far*, under the spinner. A search of a
/// disk takes minutes, and for all of that time this screen had nothing to say
/// but "looking" — while the answer was already arriving. The table is the
/// same table the finished search shows, paged the same way: what is on screen
/// when it ends is what was on screen a moment before.
pub(crate) fn waiting(
    lang: &str,
    stopping: bool,
    found: Option<&Found>,
    rows: &mut HashMap<String, Hit>,
    page: usize,
) -> Value {
    let t = |k: &str| catalog().tr(lang, k);
    // Once Stop has been pressed the screen says so until the walk has
    // actually stopped — which takes as long as it takes the threads to
    // notice, and is not instant. And the button goes: it has been pressed,
    // and a button that does nothing is a button somebody presses again.
    let mut widgets = if stopping {
        vec![
            step(t("scan.stopping"), "loading"),
            label(t("scan.stopping_hint")).weak(),
        ]
    } else {
        vec![
            step(t("scan.looking"), "loading"),
            label(t("scan.hint")).weak(),
            separator(),
            row(vec![button(t("scan.stop"), CAP, "stop")]),
        ]
    };
    if let Some(found) = found.filter(|f| !f.hits.is_empty()) {
        widgets.push(separator());
        widgets.extend(results(lang, found, rows, page));
    }
    let mut v = window(t("ui.title"), widgets);
    if let Value::Object(m) = &mut v {
        m.insert("auto".into(), poll_again());
    }
    v
}

/// Ask again, a few times a second.
///
/// Not as fast as the module can answer: the walk is doing the work and this
/// screen is reporting on it, and a report that asks sixteen hundred times a
/// second spends the machine on itself — the window stops drawing, and the
/// Stop button beside the spinner stops being noticed.
fn poll_again() -> Value {
    json!({ "capability": CAP, "method": "poll", "args": {}, "every_ms": 150 })
}

/// Asked to search nowhere: every path was missing, or there were none.
pub(crate) fn nowhere(lang: &str, places: &Roots, dict: &Dictionary) -> Value {
    let t = |k: &str| catalog().tr(lang, k);
    let mut w = vec![label(t("nowhere.title")).strong(), label(t("nowhere.body")).weak()];
    for path in &places.missing {
        w.push(label(format!("— {}", path.display())).mono());
    }
    w.extend(complaints(lang, places, dict));
    w.push(separator());
    w.push(row(vec![button(t("ui.back"), CAP, "ui")]));
    window(t("ui.title"), w)
}

/// What was wrong with the question, if anything: paths already covered by
/// another, and dictionary lines that are not patterns.
pub(crate) fn complaints(
    lang: &str,
    places: &Roots,
    dict: &Dictionary,
) -> Vec<limen_sdk_rust::ui::Widget> {
    let t = |k: &str| catalog().tr(lang, k);
    let mut w = Vec::new();
    for (dropped, by) in &places.covered {
        w.push(
            label(
                t("covered.one")
                    .replace("{path}", &dropped.display().to_string())
                    .replace("{by}", &by.display().to_string()),
            )
            .weak(),
        );
    }
    for (line, why) in &dict.rejected {
        w.push(
            label(
                t("dict.rejected")
                    .replace("{line}", line)
                    .replace("{why}", why),
            )
            .weak(),
        );
    }
    w
}

/// One result, in a tab of its own.
pub(crate) fn about(lang: &str, hit: Option<&Hit>) -> Value {
    let t = |k: &str| catalog().tr(lang, k);
    let Some(hit) = hit else {
        return window(t("detail.title"), vec![label(t("detail.missing")).weak()]);
    };
    let field = |name: String, value: String| {
        row(vec![
            label(name).strong(),
            label(if value.is_empty() { "—".to_string() } else { value }),
        ])
    };
    let mut v = window(
        hit.name.clone(),
        vec![
            label(hit.name.clone()).strong(),
            separator(),
            field(t("col.kind"), t(&format!("kind.{}", hit.kind.as_str()))),
            field(
                t("col.size"),
                if hit.kind == Kind::Dir { String::new() } else { bytes(hit.size) },
            ),
            field(t("col.created"), stamp(hit.created)),
            field(t("col.changed"), stamp(hit.changed)),
            field(t("col.rule"), terms_that_found(hit).join(", ")),
            field(t("col.path"), hit.path.to_string_lossy().to_string()),
        ],
    );
    // What it actually says, where it says it. The line is the answer to the
    // question a content search is asked: not only that the word is in there,
    // but what it is in there for.
    if !hit.inside.is_empty() {
        let mut more = vec![separator(), label(t("detail.inside")).strong()];
        for found in &hit.inside {
            let mut where_ = found.term.clone();
            if !found.member.is_empty() {
                where_ = format!("{where_} — {}", found.member);
            }
            if found.line > 0 {
                where_ = format!("{where_} : {}", found.line);
            }
            more.push(label(where_).weak());
            if !found.text.is_empty() {
                more.push(label(found.text.clone()).mono());
            }
        }
        if let Value::Object(m) = &mut v {
            if let Some(Value::Array(ws)) = m.get_mut("widgets") {
                ws.extend(more.into_iter().map(|w| w.into_value()));
            }
        }
    }
    v
}

/// A size as somebody reads one.
pub(crate) fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// A count with its thousands spaced, because six digits run together.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

/// A timestamp as `YYYY-MM-DD HH:MM`, or nothing when the filesystem does not
/// keep one. Written here rather than taken from a date library: the module
/// needs one format, in one place.
pub(crate) fn stamp(when: Option<SystemTime>) -> String {
    let Some(secs) = when
        .and_then(|w| w.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
    else {
        return String::new();
    };
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rest / 3600,
        (rest % 3600) / 60
    )
}

/// The civil date of a day number, by Howard Hinnant's algorithm — the inverse
/// of the one the date filters use.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
