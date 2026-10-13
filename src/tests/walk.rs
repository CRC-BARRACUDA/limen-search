//! The walk itself: what it finds, where it refuses to go, and that sharing the
//! work out does not change the answer.

use crate::dict::parse;
use crate::filter::{Filters, Kind};
use crate::peek::Look;
use crate::walk::walk;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// A small tree, built in a directory of its own so the tests do not depend on
/// whatever this machine happens to have.
struct Tree(PathBuf);

impl Tree {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("limen-search-test-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["", "projects/project1/src", "projects/project2", "logs"] {
            std::fs::create_dir_all(root.join(dir)).expect("the test tree");
        }
        for (path, bytes) in [
            ("projects/project1/src/invoice.rs", 10),
            ("projects/project1/Invoice-2024.pdf", 2048),
            ("projects/project2/notes.md", 20),
            ("projects/backup.zip", 4096),
            ("logs/invoice.log", 30),
            ("logs/syslog", 40),
        ] {
            std::fs::write(root.join(path), vec![b'x'; bytes]).expect("a test file");
        }
        Tree(root)
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(roots: Vec<PathBuf>, dict: &str, filters: Filters, threads: usize) -> Vec<String> {
    let found = walk(
        roots,
        Arc::new(parse(dict)),
        Arc::new(filters),
        Look::default(),
        threads,
        1_000,
        Arc::new(AtomicBool::new(false)),
    );
    let mut names: Vec<String> = found.hits.iter().map(|h| h.name.clone()).collect();
    names.sort();
    names
}

/// A word finds every name that carries it, however deep.
#[test]
fn a_word_is_found_all_the_way_down() {
    let tree = Tree::new("word");
    let names = run(vec![tree.0.clone()], "invoice", Filters::default(), 4);
    assert_eq!(names, ["Invoice-2024.pdf", "invoice.log", "invoice.rs"]);
}

/// However the work is shared out, the answer is the same. A walk whose result
/// depended on the number of threads would be one nobody could trust twice.
#[test]
fn the_answer_does_not_depend_on_the_number_of_threads() {
    let tree = Tree::new("threads");
    let one = run(vec![tree.0.clone()], "", Filters::default(), 1);
    for threads in [2, 4, 8, 16] {
        let many = run(vec![tree.0.clone()], "", Filters::default(), threads);
        assert_eq!(one, many, "{threads} threads found something else");
    }
    // And it really did find the whole tree, not nothing twice over.
    assert!(one.len() >= 9, "{one:?}");
}

/// An archive is its own kind, so "everything that could be carrying data out"
/// is one question rather than a list of extensions.
#[test]
fn an_archive_is_told_from_a_file() {
    let tree = Tree::new("kinds");
    let archives = run(
        vec![tree.0.clone()],
        "",
        Filters { kinds: vec![Kind::Archive], ..Default::default() },
        4,
    );
    assert_eq!(archives, ["backup.zip"]);

    let dirs = run(
        vec![tree.0.clone()],
        "",
        Filters { kinds: vec![Kind::Dir], ..Default::default() },
        4,
    );
    assert!(dirs.contains(&"logs".to_string()) && dirs.contains(&"project1".to_string()));
    assert!(!dirs.contains(&"backup.zip".to_string()));
}

/// Size narrows what the words found — and never excludes a folder, which has
/// no size worth comparing.
#[test]
fn size_narrows_files_and_leaves_folders_alone() {
    let tree = Tree::new("size");
    let big = run(
        vec![tree.0.clone()],
        "",
        Filters { larger_than: Some(1024), ..Default::default() },
        4,
    );
    assert!(big.contains(&"backup.zip".to_string()));
    assert!(big.contains(&"Invoice-2024.pdf".to_string()));
    assert!(!big.contains(&"notes.md".to_string()), "{big:?}");
    assert!(big.contains(&"logs".to_string()), "a folder has no size to fail");
}

/// Two roots where one is inside the other are not walked twice — the pruning
/// happens before the walk, and this is the walk agreeing with it.
#[test]
fn nothing_is_reported_twice() {
    let tree = Tree::new("once");
    let roots = crate::roots::prune(vec![tree.0.clone(), tree.at("projects/project1")]);
    assert_eq!(roots.keep.len(), 1);
    let names = run(roots.keep, "invoice", Filters::default(), 4);
    assert_eq!(names, ["Invoice-2024.pdf", "invoice.log", "invoice.rs"]);
}

/// A path that is not there is not a crash, and not a silent empty answer
/// either — the walk simply has nowhere to go.
#[test]
fn nowhere_to_look_is_not_a_panic() {
    let names = run(vec![PathBuf::from("/definitely/not/here")], "x", Filters::default(), 4);
    assert!(names.is_empty());
    assert!(run(Vec::new(), "x", Filters::default(), 4).is_empty());
}

/// A file this user cannot stat is stepped over, not guessed at.
///
/// A directory that can be listed but not searched (`r--`) gives its entries'
/// names and nothing else — so the size is unknown and the dates are unknown.
/// Judged against a size of zero, "larger than 1 KB" would quietly drop it;
/// counted and skipped, the search says instead that there was somewhere it
/// could not read.
#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_read_is_counted_and_stepped_over() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let root = std::env::temp_dir().join("limen-search-denied");
    let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("the test tree");
    std::fs::write(root.join("invoice-big.txt"), vec![b'x'; 4096]).expect("a test file");
    // Listable, not searchable: the names come back, the metadata does not.
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o400)).expect("chmod");

    // Root ignores all of this, and so would make the test assert nothing.
    let still_readable = std::fs::metadata(root.join("invoice-big.txt")).is_ok();
    if still_readable {
        let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_dir_all(&root);
        eprintln!("skipped: this user can stat the file anyway (root?)");
        return;
    }

    let look = |filters: Filters| {
        walk(
            vec![root.clone()],
            Arc::new(crate::dict::parse("invoice")),
            Arc::new(filters),
            Look::default(),
            1,
            100,
            Arc::new(AtomicBool::new(false)),
        )
    };

    // Asked about size, it cannot be answered for: skipped, and said to be.
    let narrowed = look(Filters {
        larger_than: Some(1024),
        ..Default::default()
    });
    assert!(narrowed.hits.is_empty(), "{:?}", narrowed.hits);
    assert_eq!(narrowed.denied, 1, "it should be counted: {narrowed:?}");
    // And the walk carried on rather than stopping at it.
    assert_eq!(narrowed.dirs, 1);

    // Asked nothing about size or dates, there is nothing to be wrong about —
    // the name matched, so it is reported.
    let plain = look(Filters::default());
    assert_eq!(plain.hits.len(), 1, "{:?}", plain.hits);
    assert_eq!(plain.denied, 0);

    let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
    let _ = std::fs::remove_dir_all(&root);
}

/// A walk that is told to stop before it starts does nothing, on every thread,
/// and comes back rather than waiting for work that will never be taken.
#[test]
fn a_stopped_walk_comes_straight_back() {
    let tree = Tree::new("stopped");
    let stopped = Arc::new(AtomicBool::new(true));
    let started = std::time::Instant::now();
    let found = walk(
        vec![tree.0.clone()],
        Arc::new(parse("invoice")),
        Arc::new(Filters::default()),
        Look::default(),
        // More threads than there is work for, so most of them have nothing to
        // do but notice.
        8,
        1_000,
        Arc::clone(&stopped),
    );
    assert!(found.hits.is_empty(), "{:?}", found.hits);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "it took {:?} to do nothing",
        started.elapsed()
    );
}

/// The kernel's filesystems are found where they are *mounted*, not where
/// they are usually called.
///
/// A container mounts `/proc` somewhere else, and a list of names would be
/// wrong exactly where being wrong costs the most.
#[test]
fn the_kernels_own_filesystems_are_read_from_the_mount_table() {
    // As the kernel writes it, with a real disk among them.
    let mountinfo = "\
21 27 0:20 / /sys rw,nosuid,nodev,noexec,relatime shared:7 - sysfs sysfs rw,seclabel
22 27 0:5 / /proc rw,nosuid,nodev,noexec,relatime shared:13 - proc proc rw
23 27 0:6 / /dev rw,nosuid shared:2 - devtmpfs devtmpfs rw,seclabel,size=4096k
27 1 0:32 / / rw,relatime shared:1 - btrfs /dev/mapper/luks rw,seclabel,subvolid=257
31 23 0:21 / /dev/shm rw,nosuid,nodev shared:3 - tmpfs tmpfs rw,seclabel
48 27 0:49 / /home/me/My\\040Files rw,relatime shared:28 - ext4 /dev/sdb1 rw
60 22 0:52 / /var/lib/containers/x/proc rw,nosuid shared:99 - proc proc rw
";
    let found = crate::walk::kernel_mounts(mountinfo);
    for want in ["/sys", "/proc", "/dev", "/var/lib/containers/x/proc"] {
        assert!(
            found.contains(&PathBuf::from(want)),
            "{want} is the kernel's: {found:?}"
        );
    }
    // The disks are not: a search that stepped around them would find nothing
    // of what it was asked for.
    for disk in ["/", "/home/me/My Files"] {
        assert!(!found.contains(&PathBuf::from(disk)), "{disk} is a disk: {found:?}");
    }
    // tmpfs is a filesystem in memory, and `/dev/shm` is full of other
    // programs' working files rather than the kernel's own state — it is left
    // to be walked like any other directory.
    assert!(!found.contains(&PathBuf::from("/dev/shm")), "{found:?}");
}

/// A file that is not a file is never read.
///
/// `/dev/zero` reports a size of zero, so it is under every ceiling there is,
/// and reading it does not end: six times in one evening the kernel's
/// out-of-memory killer took the whole application down, at forty-three
/// gigabytes each time.
#[cfg(unix)]
#[test]
fn a_device_is_not_read_as_a_file() {
    let zero = std::path::Path::new("/dev/zero");
    if !zero.exists() {
        return;
    }
    // Pointed at /dev directly — which is allowed, it was named — looking
    // inside everything, for a term that matches every name.
    let found = walk(
        vec![PathBuf::from("/dev")],
        Arc::new(parse("zero")),
        Arc::new(Filters::default()),
        Look { contents: true, archives: true },
        2,
        50,
        Arc::new(AtomicBool::new(false)),
    );
    // It is found by name — it is a thing on the machine, and the name
    // matched.
    assert!(
        found.hits.iter().any(|h| h.name == "zero"),
        "it should still be listed: {:?}",
        found.hits.iter().map(|h| &h.name).collect::<Vec<_>>()
    );
    // And nothing was read out of it.
    let read_it = found
        .hits
        .iter()
        .any(|h| h.name == "zero" && !h.inside.is_empty());
    assert!(!read_it, "something was read out of /dev/zero");
}
