//! What the module is while it runs, and every method it answers to.

use crate::book::{self, Book};
use crate::dict::parse;
use crate::filter::{date, size, Filters, Kind};
use crate::peek::Look;
use crate::walk::{threads_available, walk_into, Found, Hit};

/// Every field of the form, named once. The screen fills them, the walk is
/// built from them, and a kept dictionary is one of them — so a field added in
/// one place and forgotten in another is not possible.
/// `paths` is on the list though no field carries it any more: the list of
/// places is the module's, drawn as a table and changed by Add and Remove, and
/// it is saved and restored with everything else.
pub(crate) const FORM_KEYS: [&str; 16] = [
    "paths", "path", "dict", "term", "term_regex", "inside", "in_zip",
    "name", "kind",
    "created_after", "created_before", "changed_after", "changed_before",
    "larger_than", "smaller_than", "threads",
];
use crate::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct Search {
    /// The search in flight, if there is one. The walk happens on its own
    /// threads: the screen asks whether it has finished and draws a spinner
    /// until it has, rather than holding the UI for the length of a disk.
    running: Option<Running>,
    /// What the last search found, so reopening the tab shows it again instead
    /// of an empty screen.
    last: Option<Found>,
    /// What was asked for, restored into the fields on reopen.
    asked: HashMap<String, String>,
    /// The last results by row id, so a row action can say which hit it was.
    rows: HashMap<String, Hit>,
    /// Where this module keeps its things, learned from the first call that
    /// carried a host. The screen needs it to list what has been saved, and a
    /// screen is drawn from state rather than handed a host of its own.
    dir: Option<String>,
    /// Which page of the results is being looked at, counted from zero. Kept
    /// here rather than in the view because a press that is not about the
    /// results — saving a dictionary, say — must not quietly turn the page
    /// back to the first.
    page: usize,
    /// One line about what just happened — a search kept, a file written, a
    /// file that turned out not to be one of ours. Shown once, at the top of
    /// the screen that follows.
    note: Option<String>,
}

struct Running {
    /// Told once, when the walk has finished and the table below is settled.
    done: Receiver<()>,
    stop: Arc<AtomicBool>,
    /// What has been found so far — filled by the walk, read by the screen
    /// between frames. See [`crate::walk::walk_into`].
    live: Arc<Mutex<Found>>,
    /// Whether Stop has been pressed.
    ///
    /// Kept, because stopping is not instant: the walk notices between
    /// directories and between the files it is reading inside, and until it
    /// does the screen must go on saying it is stopping. Without this the
    /// next poll — a tenth of a second later — said "Looking…" again, which
    /// reads as the Stop having been forgotten.
    asked_to_stop: bool,
}

impl Handler for Search {
    fn capabilities(&self) -> Vec<String> {
        vec![CAP.into()]
    }

    fn invoke(
        &mut self,
        _capability: &str,
        method: &str,
        params: Value,
        host: &Host,
    ) -> Result<Value, RpcError> {
        let lang = host.locale();
        let lang = lang.as_str();
        if self.dir.is_none() {
            self.dir = host.module_dir();
        }
        match method {
            "ui" => Ok(self.screen(lang)),
            "search" => Ok(self.start(&params, lang)),
            // Asked for by the scanning screen itself, every frame, until the
            // walk is done. See `view::scanning`.
            "poll" => Ok(self.poll(lang)),
            "stop" => Ok(self.cancel(lang)),
            "about" => Ok(self.about(&params, lang)),
            // Keeping a search, and carrying one somewhere else.
            "add_path" => Ok(self.add_path(&params, host, lang)),
            "drop_path" => Ok(self.drop_path(&params, lang)),
            "add_term" => Ok(self.add_term(&params, lang)),
            "drop_term" => Ok(self.drop_term(&params, lang)),
            "page" => Ok(self.turn_to(&params, lang)),
            "save" => Ok(self.save(&params, host, lang)),
            "load" => Ok(self.load(&params, host, lang)),
            "forget" => Ok(self.forget(&params, host, lang)),
            "export" => Ok(self.export(&params, host, lang)),
            "import" => Ok(self.import(&params, host, lang)),
            "open" => Ok(self.open(&params, host)),
            other => Err(RpcError::new(
                rpc::METHOD_NOT_FOUND,
                format!("search has no method {other}"),
            )),
        }
    }
}

impl Search {
    /// The screen a method would answer with, without the host a real call
    /// carries. For tests: everything here is the module's own state and the
    /// catalogue, neither of which needs a host.
    #[cfg(test)]
    pub(crate) fn invoke_for_test(&mut self, method: &str) -> Value {
        match method {
            "ui" => self.screen("en"),
            other => panic!("no test door for {other}"),
        }
    }

    /// Where saved searches go — the one thing a call would have learned from
    /// its host.
    /// One folder into the list, without the chooser a real press opens.
    #[cfg(test)]
    pub(crate) fn append_for_test(&mut self, path: &str, lang: &str) -> Value {
        self.append(path, lang)
    }

    /// A path typed into the box, without the press that carries the form.
    #[cfg(test)]
    pub(crate) fn add_typed_for_test(&mut self, typed: &str, lang: &str) -> Value {
        self.asked.insert("path".into(), typed.to_string());
        let typed = typed.trim().to_string();
        self.add_typed(&typed, lang)
    }

    /// One row of the list taken out, by its place in it.
    #[cfg(test)]
    pub(crate) fn drop_path_for_test(&mut self, id: &str, lang: &str) -> Value {
        self.drop_path(&json!({ "id": id }), lang)
    }

    /// A term typed into the box, without the press that carries the form.
    #[cfg(test)]
    pub(crate) fn add_term_for_test(&mut self, typed: &str, pattern: bool, lang: &str) -> Value {
        self.add_term(&json!({ "term": typed, "term_regex": pattern }), lang)
    }

    #[cfg(test)]
    pub(crate) fn drop_term_for_test(&mut self, id: &str, lang: &str) -> Value {
        self.drop_term(&json!({ "id": id }), lang)
    }

    /// What the form holds for one field.
    #[cfg(test)]
    pub(crate) fn form_for_test(&self, key: &str) -> Option<String> {
        self.asked.get(key).cloned()
    }

    /// Turn to a page of the results, without the press that carries the form.
    #[cfg(test)]
    pub(crate) fn page_for_test(&mut self, page: usize, lang: &str) -> Value {
        self.turn_to(&json!({ "page": page }), lang)
    }

    /// A search in flight whose table already holds these hits — the state a
    /// real walk is in a second after it starts, without the second.
    #[cfg(test)]
    pub(crate) fn looking_at_for_test(&mut self, found: Found) {
        let (tx, done) = channel::<()>();
        // Kept, so the search never finishes: this is the middle of one.
        std::mem::forget(tx);
        self.running = Some(Running {
            done,
            stop: Arc::new(AtomicBool::new(false)),
            live: Arc::new(Mutex::new(found)),
            asked_to_stop: false,
        });
    }

    /// A search whose walk has died — the thread gone without an answer.
    #[cfg(test)]
    pub(crate) fn orphan_for_test(&mut self) {
        let (tx, done) = channel::<()>();
        drop(tx);
        self.running = Some(Running {
            done,
            stop: Arc::new(AtomicBool::new(false)),
            live: Arc::new(Mutex::new(Found::default())),
            asked_to_stop: false,
        });
    }

    #[cfg(test)]
    pub(crate) fn stop_for_test(&mut self, lang: &str) -> Value {
        self.cancel(lang)
    }

    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        self.running.is_some()
    }

    #[cfg(test)]
    pub(crate) fn keep_things_in(&mut self, dir: &str) {
        self.dir = Some(dir.to_string());
    }

    #[cfg(test)]
    pub(crate) fn save_for_test(&mut self, params: &Value, dir: &str, lang: &str) -> Value {
        self.keep(params, dir, lang)
    }

    #[cfg(test)]
    pub(crate) fn load_for_test(&mut self, name: &str, dir: &str, lang: &str) -> Value {
        self.recall(&json!({ "saved": name }), dir, lang)
    }

    #[cfg(test)]
    pub(crate) fn forget_for_test(&mut self, name: &str, dir: &str, lang: &str) -> Value {
        self.drop_saved(&json!({ "saved": name }), dir, lang)
    }

    /// The main screen: what was asked last time, and whatever it found.
    pub(crate) fn screen(&mut self, lang: &str) -> Value {
        if let Some(run) = &self.running {
            let found = run.live.lock().unwrap_or_else(|e| e.into_inner());
            let page = self.page.min(view::pages(found.hits.len()) - 1);
            let stopping = run.asked_to_stop;
            return view::waiting(lang, stopping, Some(&found), &mut self.rows, page);
        }
        let saved = self
            .dir
            .as_deref()
            .map(|dir| Book::load(dir).names())
            .unwrap_or_default();
        let note = self.note.take();
        view::main(
            lang,
            &self.asked,
            self.last.as_ref(),
            &mut self.rows,
            &saved,
            note.as_deref(),
            self.page,
        )
    }

    /// Read the form, start the walk, and hand back the screen that waits.
    pub(crate) fn start(&mut self, params: &Value, lang: &str) -> Value {
        // Remembered before anything else: a search that fails should come back
        // to the form the person filled in, not an empty one. Read from the
        // form as it stands afterwards rather than from the call: `paths` has
        // no field of its own and so is not in the params.
        self.asked = self.form_now(params);
        let get = |k: &str| self.asked.get(k).cloned().unwrap_or_default();

        let lines: Vec<String> = get("paths").lines().map(str::to_string).collect();
        let places = roots::resolve(&lines);
        let dict = parse(&get("dict"));
        if places.keep.is_empty() {
            return view::nowhere(lang, &places, &dict);
        }

        let kind_said = get("kind");
        let kinds = if chose(&kind_said, "kind.dirs") {
            vec![Kind::Dir]
        } else if chose(&kind_said, "kind.files") {
            vec![Kind::File, Kind::Archive]
        } else if chose(&kind_said, "kind.archives") {
            vec![Kind::Archive]
        } else {
            Vec::new()
        };
        let filters = Filters {
            kinds,
            created_after: date(&get("created_after"), false),
            created_before: date(&get("created_before"), true),
            changed_after: date(&get("changed_after"), false),
            changed_before: date(&get("changed_before"), true),
            larger_than: size(&get("larger_than")),
            smaller_than: size(&get("smaller_than")),
        };
        // "Auto" in either language, or anything that is not a number, means
        // as many as the machine will give.
        let look = Look {
            contents: get("inside") == "true",
            archives: get("in_zip") == "true",
        };
        let threads = get("threads").trim().parse().unwrap_or(0);
        let threads = if threads == 0 { threads_available() } else { threads };

        let stop = Arc::new(AtomicBool::new(false));
        let (tx, done) = channel();
        let live = Arc::new(Mutex::new(Found::default()));
        let (keep, dict, filters, flag, into) = (
            places.keep.clone(),
            Arc::new(dict),
            Arc::new(filters),
            Arc::clone(&stop),
            Arc::clone(&live),
        );
        // The one thread this spawns is the one that waits for the pool; the
        // pool itself is inside `walk_into`, which is scoped and so joins
        // before it returns. What it finds goes into `live` as it goes, which
        // is what the screen draws while it waits.
        std::thread::spawn(move || {
            walk_into(keep, dict, filters, look, threads, CAP_HITS, flag, into);
            let _ = tx.send(());
        });
        self.running = Some(Running {
            done,
            stop,
            live,
            asked_to_stop: false,
        });
        self.last = None;
        // A new search starts at the top of its own results.
        self.page = 0;
        // Nothing found yet, by definition: the walk has only just been set
        // going. The screen it returns asks again straight away.
        view::waiting(lang, false, None, &mut self.rows, 0)
    }

    /// Show another page of the results.
    pub(crate) fn turn_to(&mut self, params: &Value, lang: &str) -> Value {
        self.asked = self.form_now(params);
        // However many pages there are *now*: a search still running has as
        // many as it has found so far, and the page somebody turned to while
        // it ran should stay turned to.
        let last = match (&self.last, &self.running) {
            (Some(f), _) => view::pages(f.hits.len()) - 1,
            (None, Some(run)) => {
                let live = run.live.lock().unwrap_or_else(|e| e.into_inner());
                view::pages(live.hits.len()) - 1
            }
            _ => 0,
        };
        self.page = params
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(last as u64) as usize;
        self.screen(lang)
    }

    /// Has it finished? The scanning screen asks until it has.
    pub(crate) fn poll(&mut self, lang: &str) -> Value {
        let Some(run) = &self.running else {
            return self.screen(lang);
        };
        match run.done.try_recv() {
            Ok(()) => {
                let found = std::mem::take(&mut *run.live.lock().unwrap_or_else(|e| e.into_inner()));
                self.running = None;
                self.last = Some(found);
                self.screen(lang)
            }
            // Still looking — and showing what it has found while it does.
            // Read under the lock and drawn from there: a search that has
            // filled the table with five thousand hits should not have them
            // copied out of it sixty times a second.
            Err(TryRecvError::Empty) => {
                let found = run.live.lock().unwrap_or_else(|e| e.into_inner());
                let page = self.page.min(view::pages(found.hits.len()) - 1);
                let stopping = run.asked_to_stop;
                view::waiting(lang, stopping, Some(&found), &mut self.rows, page)
            }
            // The thread holding the other end is gone without having sent
            // anything: the walk died rather than finished. Said, and the form
            // comes back — the two must be told apart, because waiting on a
            // sender that no longer exists is waiting for ever, and the screen
            // would spin with a Stop button that could not stop anything.
            Err(TryRecvError::Disconnected) => {
                self.running = None;
                self.say(lang, "scan.died", &[])
            }
        }
    }

    /// Stop looking, and show whatever it is holding.
    ///
    /// The flag is all this does: the walk notices it — between directories,
    /// between the files it is reading inside, and within a tenth of a second
    /// if it is asleep waiting for work — and the thread that was waiting for
    /// the walk sends what it has, which the next poll picks up. Stopping is
    /// not instant, so the screen says it is stopping rather than pretending
    /// the question has already been answered.
    pub(crate) fn cancel(&mut self, lang: &str) -> Value {
        match &mut self.running {
            Some(run) => {
                run.stop.store(true, Ordering::Relaxed);
                // Remembered, so every screen from here to the end of the walk
                // says it is stopping. It was being forgotten on the very next
                // poll, which put "Looking…" back up a tenth of a second after
                // the button was pressed.
                run.asked_to_stop = true;
                let found = run.live.lock().unwrap_or_else(|e| e.into_inner());
                let page = self.page.min(view::pages(found.hits.len()) - 1);
                view::waiting(lang, true, Some(&found), &mut self.rows, page)
            }
            // Nothing is running: the walk ended between the press and the
            // call. Whatever it found is what there is to show.
            None => self.screen(lang),
        }
    }

    /// Whatever the form holds right now, from the params a button carried.
    ///
    /// Taken from the call rather than from what was last searched: somebody
    /// keeping a dictionary has just finished typing it, and has not run it
    /// yet.
    fn form_now(&self, params: &Value) -> HashMap<String, String> {
        let mut form = self.asked.clone();
        for key in FORM_KEYS {
            match params.get(key) {
                Some(Value::String(v)) => {
                    form.insert(key.to_string(), v.clone());
                }
                // A checkbox comes back as a boolean; the form keeps everything
                // as text, so that one store answers for the whole screen.
                Some(Value::Bool(b)) => {
                    form.insert(key.to_string(), b.to_string());
                }
                _ => {}
            }
        }
        form
    }

    /// Put the folder that was just browsed for into the list of places.
    ///
    /// One list, one place to read: a path that has been picked and a path that
    /// was typed are the same thing, so the picker feeds the list rather than
    /// standing beside it as a second answer to the same question.
    pub(crate) fn add_path(&mut self, params: &Value, host: &Host, lang: &str) -> Value {
        self.asked = self.form_now(params);
        let typed = self
            .asked
            .get("path")
            .map(|p| p.trim().to_string())
            .unwrap_or_default();
        if typed.is_empty() {
            // Nothing typed, so the press is a request to go and find one.
            let Some(picked) = host.pick_dir() else {
                // Cancelled, which is an answer rather than a failure. Nothing
                // is said about it: the list is unchanged and visibly so.
                return self.screen(lang);
            };
            return self.append(&picked, lang);
        }
        self.add_typed(&typed, lang)
    }

    /// A path somebody wrote out: added if it is a folder that is there, and
    /// refused with a reason if it is not.
    ///
    /// Checked here rather than left for the search: a typo in a path would
    /// otherwise sit in the list looking right until the search came back
    /// saying it had nowhere to look.
    fn add_typed(&mut self, typed: &str, lang: &str) -> Value {
        match std::fs::canonicalize(typed) {
            // Canonical, so that two spellings of one folder cannot both be in
            // the list — the same rule the walk itself prunes by.
            Ok(p) if p.is_dir() => {
                let p = p.to_string_lossy().to_string();
                self.append(&p, lang)
            }
            Ok(_) => self.say(lang, "paths.not_a_folder", &[("path", typed)]),
            Err(_) => self.say(lang, "paths.missing", &[("path", typed)]),
        }
    }

    /// Take one path back out of the list.
    pub(crate) fn drop_path(&mut self, params: &Value, lang: &str) -> Value {
        self.asked = self.form_now(params);
        let mut lines = self.path_lines();
        let gone = params
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| id.parse::<usize>().ok())
            .filter(|i| *i < lines.len())
            .map(|i| lines.remove(i));
        // A row that is no longer there — the list changed under the menu that
        // was open on it. Nothing to say, and nothing to remove.
        let Some(gone) = gone else {
            return self.screen(lang);
        };
        self.asked.insert("paths".into(), lines.join("\n"));
        self.say(lang, "paths.removed", &[("path", &gone)])
    }

    /// The list of places, one per line, with the blanks gone.
    fn path_lines(&self) -> Vec<String> {
        self.asked
            .get("paths")
            .map(|p| {
                p.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Put one folder into the list of places.
    fn append(&mut self, added: &str, lang: &str) -> Value {
        let added = added.trim().to_string();
        if added.is_empty() {
            return self.screen(lang);
        }
        let mut lines = self.path_lines();
        if lines.iter().any(|l| l == &added) {
            // Already there. Said, rather than silently doing nothing — a
            // button that appears not to have worked is a button pressed again.
            return self.say(lang, "paths.already", &[("path", &added)]);
        }
        lines.push(added.clone());
        self.asked.insert("paths".into(), lines.join("\n"));
        // The box is empty again: what was in it is now in the list, and a line
        // left behind would be added twice by the next press.
        self.asked.insert("path".into(), String::new());
        self.say(lang, "paths.added", &[("path", &added)])
    }

    /// Put what was typed into the dictionary, as a term of the kind the
    /// checkbox says.
    pub(crate) fn add_term(&mut self, params: &Value, lang: &str) -> Value {
        self.asked = self.form_now(params);
        let typed = self
            .asked
            .get("term")
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
        if typed.is_empty() {
            return self.say(lang, "dict.needs_term", &[]);
        }
        let pattern = self.asked.get("term_regex").map(String::as_str) == Some("true");
        let line = match crate::dict::write(&typed, pattern) {
            Ok(line) => line,
            // Only a pattern can fail to be written, and when it does the
            // regex engine has already said why.
            Err(why) => return self.say(lang, "dict.bad_pattern", &[("why", &why)]),
        };
        let mut lines = self.dict_lines();
        if lines.iter().any(|l| l == &line) {
            return self.say(lang, "dict.already", &[("term", &typed)]);
        }
        lines.push(line);
        self.asked.insert("dict".into(), lines.join("\n"));
        // The box is empty again: what was in it is now in the list. The
        // checkbox is left as it was — somebody adding one pattern is usually
        // adding several.
        self.asked.insert("term".into(), String::new());
        self.say(lang, "dict.added", &[("term", &typed)])
    }

    /// Take one term back out.
    pub(crate) fn drop_term(&mut self, params: &Value, lang: &str) -> Value {
        self.asked = self.form_now(params);
        let mut lines = self.dict_lines();
        let gone = params
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| id.parse::<usize>().ok())
            .filter(|i| *i < lines.len())
            .map(|i| lines.remove(i));
        let Some(gone) = gone else {
            return self.screen(lang);
        };
        self.asked.insert("dict".into(), lines.join("\n"));
        self.say(
            lang,
            "dict.removed",
            &[("term", &crate::dict::reading(&gone))],
        )
    }

    /// The dictionary, one term to a line, with the blanks gone.
    fn dict_lines(&self) -> Vec<String> {
        self.asked
            .get("dict")
            .map(|d| {
                d.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Keep this dictionary under the name the form gave it.
    pub(crate) fn save(&mut self, params: &Value, host: &Host, lang: &str) -> Value {
        match host.module_dir() {
            Some(dir) => self.keep(params, &dir, lang),
            None => self.say(lang, "dict.nowhere", &[]),
        }
    }

    fn keep(&mut self, params: &Value, dir: &str, lang: &str) -> Value {
        self.asked = self.form_now(params);
        let name = self.named(params);
        if name.is_empty() {
            return self.say(lang, "dict.needs_name", &[]);
        }
        let terms = self.asked.get("dict").cloned().unwrap_or_default();
        if book::count(&terms) == 0 {
            return self.say(lang, "dict.empty", &[]);
        }
        let mut book = Book::load(dir);
        // A name already taken is replaced: somebody saving under a name they
        // have used means to change what is under it.
        book.dictionaries.insert(name.clone(), terms);
        match book.save(dir) {
            Ok(()) => self.say(lang, "dict.kept", &[("name", &name)]),
            Err(e) => self.say(lang, "dict.failed", &[("why", &e)]),
        }
    }

    /// Put a kept dictionary back into the box.
    pub(crate) fn load(&mut self, params: &Value, host: &Host, lang: &str) -> Value {
        match host.module_dir() {
            Some(dir) => self.recall(params, &dir, lang),
            None => self.say(lang, "dict.nowhere", &[]),
        }
    }

    fn recall(&mut self, params: &Value, dir: &str, lang: &str) -> Value {
        self.asked = self.form_now(params);
        let name = params
            .get("saved")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let book = Book::load(dir);
        match book.dictionaries.get(&name) {
            Some(terms) => {
                self.asked.insert("dict".into(), terms.clone());
                self.asked.insert("name".into(), name.clone());
                self.say(
                    lang,
                    "dict.loaded",
                    &[("name", &name), ("n", &book::count(terms).to_string())],
                )
            }
            None => self.say(lang, "dict.gone", &[("name", &name)]),
        }
    }

    /// Forget one.
    pub(crate) fn forget(&mut self, params: &Value, host: &Host, lang: &str) -> Value {
        match host.module_dir() {
            Some(dir) => self.drop_saved(params, &dir, lang),
            None => self.say(lang, "dict.nowhere", &[]),
        }
    }

    fn drop_saved(&mut self, params: &Value, dir: &str, lang: &str) -> Value {
        self.asked = self.form_now(params);
        let name = params
            .get("saved")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut book = Book::load(dir);
        if book.dictionaries.remove(&name).is_none() {
            return self.say(lang, "dict.gone", &[("name", &name)]);
        }
        match book.save(dir) {
            Ok(()) => self.say(lang, "dict.forgot", &[("name", &name)]),
            Err(e) => self.say(lang, "dict.failed", &[("why", &e)]),
        }
    }

    /// Write the dictionary to a text file, wherever the user says.
    pub(crate) fn export(&mut self, params: &Value, host: &Host, lang: &str) -> Value {
        self.asked = self.form_now(params);
        let terms = self.asked.get("dict").cloned().unwrap_or_default();
        if book::count(&terms) == 0 {
            return self.say(lang, "dict.empty", &[]);
        }
        let name = self.named(params);
        let suggested = format!("{}.txt", book::file_stem(&name));
        let Some(path) = host.save_file(&suggested) else {
            return self.say(lang, "dict.export_cancelled", &[]);
        };
        match std::fs::write(&path, book::to_text(&name, &terms)) {
            Ok(()) => self.say(lang, "dict.exported", &[("path", &path)]),
            Err(e) => self.say(lang, "dict.failed", &[("why", &e.to_string())]),
        }
    }

    /// Read one somebody else wrote — this module's export, or any text file
    /// with one term to a line.
    pub(crate) fn import(&mut self, params: &Value, host: &Host, lang: &str) -> Value {
        self.asked = self.form_now(params);
        let Some(path) = host.pick_file() else {
            return self.say(lang, "dict.import_cancelled", &[]);
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return self.say(lang, "dict.unreadable", &[("path", &path)]);
        };
        let (name, terms) = book::from_text(&text);
        if book::count(&terms) == 0 {
            return self.say(lang, "dict.import_empty", &[("path", &path)]);
        }
        let n = book::count(&terms);
        self.asked.insert("dict".into(), terms);
        if let Some(name) = &name {
            self.asked.insert("name".into(), name.clone());
        }
        self.say(
            lang,
            "dict.imported",
            &[
                ("name", name.as_deref().unwrap_or(&path)),
                ("n", &n.to_string()),
            ],
        )
    }

    /// The name on the form, trimmed.
    fn named(&self, params: &Value) -> String {
        params
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| self.asked.get("name").map(String::as_str))
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// The screen again, with one line saying what just happened.
    ///
    /// Shown once: it is news about the press that produced it, and a notice
    /// that outlived its press would be read as being about the next one.
    fn say(&mut self, lang: &str, key: &str, fill: &[(&str, &str)]) -> Value {
        let mut text = catalog().tr(lang, key);
        for (slot, value) in fill {
            text = text.replace(&format!("{{{slot}}}"), value);
        }
        self.note = Some(text);
        self.screen(lang)
    }

    /// One result, in a tab of its own.
    pub(crate) fn about(&self, params: &Value, lang: &str) -> Value {
        let id = params.get("id").and_then(Value::as_str).unwrap_or("");
        view::about(lang, self.rows.get(id))
    }

    /// Show a result where it lives.
    pub(crate) fn open(&self, params: &Value, host: &Host) -> Value {
        let id = params.get("id").and_then(Value::as_str).unwrap_or("");
        let target = params.get("target").and_then(Value::as_str).unwrap_or("reveal");
        if let Some(hit) = self.rows.get(id) {
            host.open(target, &hit.path.to_string_lossy());
        }
        Value::Null
    }
}

use crate::roots;
