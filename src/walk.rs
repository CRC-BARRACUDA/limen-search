//! Walking the paths, on as many threads as the machine will give.
//!
//! A search is nearly all waiting: on a directory listing, on the metadata of
//! each entry. One thread spends that time doing nothing, so the work is shared
//! out — but a filesystem is not a list that can be cut into equal parts, it is
//! a tree whose shape nobody knows until it has been walked. So the threads
//! share one stack of directories still to read: whoever is free takes the next
//! one, and puts what it finds back for everybody.
//!
//! The walk ends when the stack is empty *and* nobody is still reading, which
//! are two different things — a stack empties long before the last worker
//! finishes finding more.

use crate::dict::Dictionary;
use crate::filter::{kind_of, Filters, Kind};
use crate::peek::{self, Inside, Look};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

/// One thing found.
#[derive(Debug, Clone)]
pub(crate) struct Hit {
    pub path: PathBuf,
    pub name: String,
    pub kind: Kind,
    pub size: u64,
    pub created: Option<SystemTime>,
    pub changed: Option<SystemTime>,
    /// The terms that found it, as the dictionary writes them. Empty when the
    /// dictionary is — a search of dates and kinds alone was not looking for
    /// anything in particular, and saying "everything" in every row would be
    /// noise.
    pub why: Vec<String>,
    /// What was found inside it, when the search was asked to look: which term,
    /// in which member, on which line, and what the line says.
    pub inside: Vec<Inside>,
}

/// What a finished search amounts to.
#[derive(Debug, Default, Clone)]
pub(crate) struct Found {
    pub hits: Vec<Hit>,
    /// Directories read, and entries looked at: the two numbers that say
    /// whether a search that found nothing actually went anywhere.
    pub dirs: usize,
    pub seen: usize,
    /// Places the operating system would not let this user read. Counted and
    /// reported — a search that quietly skips half a disk is worse than one
    /// that says it could not read it.
    pub denied: usize,
    /// Whether the search stopped because it had found all it was asked to
    /// hold, rather than because it ran out of places to look.
    pub capped: bool,
    /// Kernel filesystems not walked into — `/proc`, `/sys`, `/dev` and the
    /// rest. Counted and said, because a search of `/` that quietly stepped
    /// around a quarter of what is mounted is a search nobody can trust.
    pub kernel: usize,
    /// Files opened and read, and the three reasons one was not: it was not
    /// text, it was too large to be worth reading, or it would not be read.
    /// Shown, because a search that looked inside nothing and a search that
    /// looked inside everything and found nothing are the same empty table.
    pub looked: usize,
    pub binary: usize,
    pub too_big: usize,
    pub unread: usize,
}

/// What every worker shares.
struct Work {
    /// Directories still to read. A stack, not a queue: depth first keeps the
    /// working set to one branch, where breadth first would hold a whole level
    /// of a big tree in memory for nothing.
    stack: Mutex<Vec<PathBuf>>,
    /// How many workers are reading a directory right now. The walk is over
    /// only when this is zero and the stack is empty.
    busy: AtomicUsize,
    wake: Condvar,
    stop: Arc<AtomicBool>,
}

/// Where the kernel's own filesystems are mounted.
///
/// Read from `/proc/self/mountinfo` rather than assumed to be at `/proc` and
/// `/sys`: a container mounts them where it likes, and a list of names would
/// be wrong exactly where it matters.
///
/// Nothing under them is a file on a disk. `/proc` is tens of thousands of
/// directories describing processes, `/sys` describes the hardware, and `/dev`
/// is where the files that never end live — `/dev/zero` reports a size of
/// zero, passes every ceiling there is, and reading it ends with the machine
/// out of memory.
pub(crate) fn kernel_trees() -> Vec<PathBuf> {
    std::fs::read_to_string("/proc/self/mountinfo")
        .map(|text| kernel_mounts(&text))
        .unwrap_or_default()
}

/// The mount points of `mountinfo` that belong to the kernel.
pub(crate) fn kernel_mounts(mountinfo: &str) -> Vec<PathBuf> {
    // The kinds of filesystem the kernel makes up out of its own state. A
    // real disk is never one of these.
    const MADE_UP: [&str; 19] = [
        "proc", "sysfs", "devtmpfs", "devpts", "cgroup", "cgroup2", "debugfs", "tracefs",
        "securityfs", "bpf", "configfs", "fusectl", "pstore", "mqueue", "hugetlbfs",
        "binfmt_misc", "selinuxfs", "nsfs", "rpc_pipefs",
    ];
    let mut out = Vec::new();
    for line in mountinfo.lines() {
        // ... - <fstype> <source> <options>, with the mount point at field 5.
        let Some((head, tail)) = line.split_once(" - ") else {
            continue;
        };
        let Some(kind) = tail.split_whitespace().next() else {
            continue;
        };
        if !MADE_UP.contains(&kind) {
            continue;
        }
        let Some(at) = head.split_whitespace().nth(4) else {
            continue;
        };
        // Mount points are written with the awkward characters escaped.
        let at = at.replace("\\040", " ").replace("\\011", "\t");
        let at = PathBuf::from(at);
        if !out.contains(&at) {
            out.push(at);
        }
    }
    out
}

/// How many threads to use for a walk, when nobody said.
pub(crate) fn threads_available() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16)
}

/// Walk `roots`, keeping everything the dictionary and the filters agree on.
///
/// `cap` is how many hits to hold: a search for `/./` across a disk would
/// otherwise fill memory with a list nobody can read anyway.
#[allow(clippy::too_many_arguments)]
/// Walk and wait for the answer.
///
/// For tests, which want the whole result and have nothing to draw meanwhile.
/// The module itself uses [`walk_into`], because a screen that waits for the
/// whole answer is a screen with nothing to show while it is being found.
#[cfg(test)]
pub(crate) fn walk(
    roots: Vec<PathBuf>,
    dict: Arc<Dictionary>,
    filters: Arc<Filters>,
    look: Look,
    threads: usize,
    cap: usize,
    stop: Arc<AtomicBool>,
) -> Found {
    let out = Arc::new(Mutex::new(Found::default()));
    walk_into(roots, dict, filters, look, threads, cap, stop, Arc::clone(&out));
    let found = out.lock().unwrap_or_else(|e| e.into_inner());
    found.clone()
}

/// The same walk, filling a table the caller can read while it runs.
///
/// A search of a disk takes seconds or minutes, and for all of that time the
/// screen used to have nothing to show but a spinner — while the answer was
/// already arriving, hit by hit, into a table nobody could see. The caller
/// holds the table; the walk fills it; the screen reads it between frames.
#[allow(clippy::too_many_arguments)]
pub(crate) fn walk_into(
    roots: Vec<PathBuf>,
    dict: Arc<Dictionary>,
    filters: Arc<Filters>,
    look: Look,
    threads: usize,
    cap: usize,
    stop: Arc<AtomicBool>,
    out: Arc<Mutex<Found>>,
) {
    if roots.is_empty() {
        return;
    }
    let work = Arc::new(Work {
        stack: Mutex::new(roots),
        busy: AtomicUsize::new(0),
        wake: Condvar::new(),
        stop,
    });
    // The kernel's own filesystems, read once: what is *mounted* rather than
    // what is named `/proc`, because a container mounts them elsewhere.
    let kernel = kernel_trees();
    let threads = threads.clamp(1, 16);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let (work, out, dict, filters) = (
                Arc::clone(&work),
                Arc::clone(&out),
                Arc::clone(&dict),
                Arc::clone(&filters),
            );
            let kernel = &kernel;
            scope.spawn(move || worker(&work, &out, &dict, &filters, look, cap, kernel));
        }
    });

    // In path order, which is the order somebody reads a list of files in: a
    // folder's hits together, and the same list every time the same search is
    // run. The threads finish in whatever order the disk gives them, and a
    // result set that comes back shuffled cannot be paged through — page two
    // would be a different hundred each time it was asked for.
    //
    // Sorted only at the end: while the walk runs the hits are in the order
    // they were found, so what is on the screen stays where it is and new
    // ones arrive beneath it. Sorting as they came would reshuffle the page
    // somebody is reading, several times a second.
    let mut found = out.lock().unwrap_or_else(|e| e.into_inner());
    found.hits.sort_by(|a, b| a.path.cmp(&b.path));
}

#[allow(clippy::too_many_arguments)]
fn worker(
    work: &Work,
    out: &Mutex<Found>,
    dict: &Dictionary,
    filters: &Filters,
    look: Look,
    cap: usize,
    kernel: &[PathBuf],
) {
    loop {
        // Take the next directory, or wait for one. Waiting rather than
        // spinning: a worker with nothing to do should cost nothing.
        let dir = {
            let mut stack = work.stack.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if work.stop.load(Ordering::Relaxed) {
                    // Everybody else has to be told, or the ones asleep sleep
                    // on: the last worker to leave normally wakes them when
                    // the count reaches zero, and a worker leaving because it
                    // was stopped never gets there.
                    work.wake.notify_all();
                    return;
                }
                if let Some(d) = stack.pop() {
                    work.busy.fetch_add(1, Ordering::AcqRel);
                    break d;
                }
                // Nothing left, and nobody still looking: the walk is done, and
                // every other sleeper has to be told, or they sleep for ever.
                if work.busy.load(Ordering::Acquire) == 0 {
                    work.wake.notify_all();
                    return;
                }
                // Timed, so that Stop is noticed by a worker that is asleep.
                // Stop is set from the UI thread, which has no way to reach
                // this condvar: without a timeout a sleeper waits for a push
                // that is never coming, the scope never joins, and the screen
                // goes on saying "looking" long after it was told to stop.
                stack = work
                    .wake
                    .wait_timeout(stack, std::time::Duration::from_millis(100))
                    .map(|(guard, _)| guard)
                    .unwrap_or_else(|e| e.into_inner().0);
            }
        };

        let (found, subdirs, denied, tally) =
            read(&dir, dict, filters, look, &work.stop, kernel);

        let mut full = false;
        {
            let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
            o.dirs += 1;
            o.denied += denied;
            o.kernel += tally.kernel;
            o.looked += tally.looked;
            o.binary += tally.binary;
            o.too_big += tally.too_big;
            o.unread += tally.unread;
            o.seen += found.0;
            for hit in found.1 {
                if o.hits.len() >= cap {
                    o.capped = true;
                    full = true;
                    break;
                }
                o.hits.push(hit);
            }
        }
        if full {
            work.stop.store(true, Ordering::Relaxed);
        }

        {
            let mut stack = work.stack.lock().unwrap_or_else(|e| e.into_inner());
            stack.extend(subdirs);
            work.busy.fetch_sub(1, Ordering::AcqRel);
        }
        // Outside the lock: a waiter wakes to take the lock, and waking it
        // while still holding it only has it block again.
        work.wake.notify_all();
    }
}

/// Read one directory: what matched, where to go next, and whether it could be
/// read at all.
/// What looking inside the files of one directory came to.
#[derive(Default)]
struct Tally {
    looked: usize,
    binary: usize,
    too_big: usize,
    unread: usize,
    kernel: usize,
}

#[allow(clippy::type_complexity)]
fn read(
    dir: &PathBuf,
    dict: &Dictionary,
    filters: &Filters,
    look: Look,
    stop: &AtomicBool,
    kernel: &[PathBuf],
) -> ((usize, Vec<Hit>), Vec<PathBuf>, usize, Tally) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // Unreadable: counted, and not a reason to stop. On any real machine a
        // search crosses something it is not allowed into.
        Err(_) => return ((0, Vec::new()), Vec::new(), 1, Tally::default()),
    };

    let mut seen = 0usize;
    let mut hits = Vec::new();
    let mut subdirs = Vec::new();
    let mut denied = 0usize;
    let mut tally = Tally::default();

    for entry in entries {
        let Ok(entry) = entry else {
            denied += 1;
            continue;
        };
        seen += 1;
        // The entry's own type, which does not follow a symlink: a link into a
        // parent would otherwise send the walk round for ever.
        let Ok(file_type) = entry.file_type() else {
            denied += 1;
            continue;
        };
        let is_dir = file_type.is_dir();
        if is_dir && !file_type.is_symlink() {
            // Not into the kernel's own filesystems. `/proc` alone is tens of
            // thousands of directories that are not on any disk, `/dev` is
            // where the files that never end live, and nothing in either is
            // what somebody means by "search this machine". Walked only when
            // one of them is named as a place to search outright — see
            // `kernel_trees`.
            let path = entry.path();
            if kernel.contains(&path) {
                tally.kernel += 1;
            } else {
                subdirs.push(path);
            }
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // The name decides it on its own; looking inside is for the files whose
        // names said nothing — and only when the search asked for it, and only
        // when there is something to look for.
        let by_name = dict.hits(&name);
        if !by_name && !(look.asked() && !dict.is_empty()) {
            continue;
        }
        let kind = kind_of(is_dir, &name);
        let meta = entry.metadata().ok();
        // Nothing to judge it by. A file this user may not stat is skipped and
        // counted, never guessed at: judged against a size of zero and no
        // dates, "larger than 10 MB" would quietly drop it and "changed before
        // Tuesday" would quietly keep it — both answers made up. When nothing
        // was asked about size or dates there is nothing to be wrong about, and
        // the name matched, so it is reported as it stands.
        if meta.is_none() && filters.needs_metadata() {
            denied += 1;
            continue;
        }
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let created = meta.as_ref().and_then(|m| m.created().ok());
        let changed = meta.as_ref().and_then(|m| m.modified().ok());
        if !filters.keeps(kind, size, created, changed) {
            continue;
        }
        let why: Vec<String> = if by_name {
            dict.why(&name).into_iter().map(str::to_string).collect()
        } else {
            Vec::new()
        };
        // Narrowed first, then read: this is the one place the two passes meet,
        // and the whole reason a content search over a disk finishes at all.
        let mut inside = Vec::new();
        // Only a real file is read. A character device, a pipe, a socket: a
        // read of one of those blocks, or never ends — `/dev/zero` reports a
        // size of zero and is under every ceiling there is, and reading it
        // took the whole application down with the kernel's out-of-memory
        // killer, six times in one evening.
        if look.asked() && file_type.is_file() && !dict.is_empty() {
            // Checked here as well as between directories: a tree of large
            // files would otherwise keep reading for seconds after Stop.
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let found = peek::look(&entry.path(), size, kind, dict, look);
            tally.looked += found.read as usize;
            tally.binary += found.binary as usize;
            tally.too_big += found.too_big as usize;
            tally.unread += found.failed as usize;
            inside = found.found;
        }
        if !by_name && inside.is_empty() {
            continue;
        }
        hits.push(Hit {
            path: entry.path(),
            name,
            kind,
            size,
            created,
            changed,
            why,
            inside,
        });
    }
    ((seen, hits), subdirs, denied, tally)
}
