# limen-search

A [Limen](https://github.com/CRC-BARRACUDA/Limen) module that finds things on
**this** machine: files, folders and archives, by name, phrase, pattern or date,
across several paths at once — and, when you ask it to, by what they say
inside. It provides the `search.local` capability.

Nothing is sent anywhere, nothing is run, and nothing outside the paths you name
is read. **With "Search inside file contents" ticked it reads those files**, so
it is worth knowing which ones: see [Looking inside](#looking-inside).

## The dictionary

What a search is looking for, one line at a time — because that is how a list of
things to look for is actually written down: pasted from a ticket, kept in a
file, added to over a case.

```text
invoice                 a word     — anywhere in the name, any case
"annual report"         a phrase   — spaces and all, any case
/^IMG_\d{4}\.jpe?g$/    a pattern  — a regular expression
# a line like this is a note to whoever reads the dictionary
```

Any one term is enough: a dictionary is a list of things worth finding, not a
description of one thing. A line that was meant to be a pattern and will not
compile is **reported**, not dropped — a dictionary that quietly ignores a line
searches for less than it was asked to.

Leave it empty to match every name and let the rest of the form do the asking.

## Paths that overlap

Give it as many places as you like, one per line. **Add path** opens a folder
chooser and the folder lands in that same box as a line like any other — one
list, however a line got into it, and one place to read before knowing what
will be searched. Name a folder and its own
parent and it is searched **once**, as part of the parent:

```text
D:/dev/projects
D:/dev/projects/project1     ← already inside the first, and the screen says so
```

Compared by path component, never as text: `/a/bc` is not inside `/a/b`, and a
search that dropped it would silently miss everything in it. Repeats collapse
the same way, and a path that is not a readable folder is named rather than
ignored.

## Narrowing

| | |
|---|---|
| **Kind** | anything, files, folders, or archives |
| **Size** | larger than / smaller than — `500`, `10k`, `2.5 MB`, `1gb` |
| **Created** | after / before |
| **Changed** | after / before |

A filesystem that does not record a creation time cannot answer a question about
one, so those entries are excluded rather than assumed — saying "made after
Tuesday" about a time nobody knows is an answer made up.

An archive is a kind of its own, by extension: `zip`, `7z`, `rar`, `tar`, `gz`,
`iso`, `jar`, `dmg` and the rest. By extension rather than by opening every
file, because opening every file on a disk is the search taking all afternoon —
and opening them is the separate, deliberate thing described under
[Looking inside](#looking-inside).

## Threads

A search is nearly all waiting — on a directory listing, on the metadata of each
entry — so the work is shared out. A filesystem is not a list that can be cut
into equal parts, though: it is a tree whose shape nobody knows until it has
been walked. So the threads share one stack of directories still to read.
Whoever is free takes the next one and puts what it finds back for everybody,
and the walk ends when the stack is empty **and** nobody is still reading.

This checkout, 113 382 entries across 10 745 folders:

| threads | time | hits |
|---|---|---|
| 1 | 171 ms | 64 |
| 2 | 95 ms | 64 |
| 4 | 55 ms | 64 |
| auto | 26 ms | 64 |

The same answer every time, whatever the machine does with it — a walk whose
result depended on the thread count is one nobody could trust twice. The count
can be pinned on the form, because a search at full tilt competes with whatever
else is reading the same disk.

The walk runs on its own threads and the screen asks whether it has finished, so
the window keeps drawing while a disk is being read.

## Keeping a dictionary

The dictionary is the part worth keeping. Paths change with the machine and
dates change with the case, but the list of things worth looking for — the
filenames a piece of malware drops, what a leak looks like, the extensions that
should never be on a share — is written once and used for years, by everybody.

| | |
|---|---|
| **Save / Load / Forget** | dictionaries kept beside the module in `dictionaries.json`, picked from a list by name |
| **Export / Import** | one dictionary as a plain text file — for the ones you send |

Exported as **text**, because a dictionary already is one: a term to a line.
That is what somebody pastes into a ticket, keeps beside the code it is about,
or writes in an editor having never run this module — and it reads back without
ceremony.

```text
# Limen search dictionary: Exfil sweep
invoice
"annual report"
/\.p12$/
```

Importing takes any text file. Notes and blank lines are already part of the
format, so there is nothing for a file to fail to be — and the first line is
read back as the name only if this module wrote it.

Loading a dictionary leaves the **paths alone**: where somebody is about to
look is the machine's business, not the dictionary's.

`dictionaries.json` holds what *this* machine kept, so it is not in this
repository.

## Looking inside

Two boxes under the dictionary, both off by default.

| | |
|---|---|
| **Search inside file contents** | plain text · PDF · `.docx` `.xlsx` `.pptx` · OpenDocument · `.doc` `.xls` `.ppt` (best effort) |
| **Look inside archives** | `.zip` and everything built on it (`.jar` `.apk` `.epub` …) · `.tar` · `.tar.gz` · `.gz` · `.7z` — member names, and their contents with the first box ticked |

Matching a name costs a `readdir` and a `stat`; reading a file costs hundreds of
times more. So it is a **second pass over what the first one left standing** —
the dictionary, the kind, the size and the dates all narrow it first — and with
an **empty dictionary nothing is opened at all**, because there is nothing to
look for.

A term means the same thing inside a file as it does in a name, applied to one
**line** at a time: `/^ERROR \d+$/` anchors to the line, which is what anybody
writing it meant. Each result says which terms found it, where (its name, its
text, or the member it was in), and the line itself.

Word, Excel, PowerPoint and OpenDocument files are zips of XML, and their text cannot be
matched as XML: Word splits a sentence wherever the formatting changes, so
*annual report* is stored as `annual</w:t>…<w:t> report`. Searched as markup the
phrase is simply not there, and the answer would be "not found" about a document
that says it on its first page. So the runs are put back together first, and
only then read.

A PDF is not text either: it is glyphs placed on a page, each in whatever
encoding its font declares, so it goes through a real parser. A PDF that is a
photograph of a page contains **no text at all** — that comes back as a file
read with nothing in it, which is why the counters matter.

The pre-2007 `.doc`, `.xls` and `.ppt` are **best effort, and said to be**: a
correct reader follows the piece table in the table stream to put the document
back together, and what this does is read the runs of text out of the streams
that hold them. It answers "does this file mention X", which is the question a
search is asking. It is not a document reader and does not pretend to be.

What is never opened, whatever is ticked:

- **Anything that is not a file.** A character device, a block device, a pipe,
  a socket. `/dev/zero` reports a size of zero, so it passes every ceiling
  there is, and reading it does not end.
- **The kernel's own filesystems** — `/proc`, `/sys`, `/dev` and the rest, found
  from `/proc/self/mountinfo` by filesystem *type* rather than by name, because
  a container mounts them where it likes. Nothing under them is a file on a
  disk. Name one as a path to search and it is searched: the rule is "we do not
  wander in", not "you may not go there". The summary says how many were
  stepped around.

Every limit here exists because the files being read are the ones under
investigation:

| | |
|---|---|
| **16 MiB** | the most of one file held in memory — for the formats that must be buffered; text is streamed and has no such limit |
| **192 MiB** | the most every thread *together* may hold; a thread that cannot have its share waits for one |
| **NUL in the first 8 KiB** | not text; skipped (UTF-16 is read by its byte-order mark first) |
| **256 MiB / 10 000 members** | what one archive may be made to give up |
| **one level** | an archive inside an archive is not opened |
| **per file** | every parser runs caught, so a malformed document cannot take the tab down |

What it costs, measured on one tree of 341 207 entries, 16 threads, with every
format enabled:

| | entries | files opened | time |
|---|---|---|---|
| names only | 341 207 | 0 | ~0.1 s |
| contents + archives | 341 207 | 216 428 (91 800 not text, 200 too large, 150 unreadable) | 13.0 s |

Two orders of magnitude, which is the whole reason for the narrowing: give the
dictionary a kind, a size or a date and the second pass has a fraction as much
to open.

The summary says what came of it — `1 240 files read inside · 38 not text ·
4 too large · 11 could not be read` — because a search that looked inside
nothing and one that looked inside everything and found nothing are otherwise
the same empty table.

## What it does not do

- **Read a scanned PDF.** There is no text in a photograph of a page. That
  needs OCR, which is a different product.
- **Open `.rar`.** Its decoder is published under a licence that forbids using
  the source to build another implementation of the format, which makes it a
  bad fit for a GPL module.
- **Open disk images** — `.iso`, `.dmg`, `.cab`. They are filesystems, not
  archives, and each wants its own reader.
- **Open an archive inside an archive.** One level, so a few kilobytes of
  nesting cannot be made to unpack to terabytes.
- **Scan with IOC or YARA rules.** That is `loki`.
- **Follow symlinked directories**, which would send the walk round for ever.

## Building

```sh
cargo build --release          # a cdylib: libsearch.so / .dll / .dylib
scripts/package.sh             # the dist/ bundle Limen installs
```

Against a working-tree SDK rather than the published one:

```sh
LIMEN_SDK_PATH=/path/to/Limen/src/limen-sdk-rust scripts/package.sh
```

A real search, run by hand with its numbers printed:

```sh
LIMEN_SEARCH_AT=/some/tree cargo test -- --ignored live --nocapture
```

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE).
