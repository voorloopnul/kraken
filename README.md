# Kraken

Kraken is a native desktop front end for the [Pi coding agent](https://github.com/earendil-works/pi-coding-agent),
written in Rust with a Qt/QML interface: a chat transcript beside the panes you
need while the agent works.

**Main features:**

- a chat / agent transcript
- a history pane — the Pi sessions recorded for this workspace folder, and the
  one running now; pin the ones you keep coming back to
- an embedded terminal
- an embedded browser
- a git pane, in two tabs: **Changes**, the files touched since the last commit
  with the lines added and removed in each — click one to read its diff, syntax
  highlighted, over the dimmed app — and **Commits**, the repository's commit
  graph, newest first
- a files pane — the workspace as a tree, the way an editor draws one; click a
  file to read it over the dimmed app, and copy files in and out of the project.
  A name that is already taken stops the copy and asks: replace, keep both, or
  cancel. Works the same on a workspace reached over SSH (see below)
- workspace/project switching, local and over SSH

## Commit and push

In the Git pane's **Changes** tab, check the files you want to commit, or use
**Select all**. New rows start unchecked; your choices survive refreshes and
workspace switches, and newly discovered files never join the selection on
their own. Clicking a filename still opens its diff.

Enter a commit message (with an optional multiline body) at the bottom and
click **Commit**. This commits only checked files, including their unstaged
edits, new files and deletions. Unchecked files stay out even if already staged,
and their staged changes are preserved. A rename includes both its old and new
names. Commit is disabled until at least one file and a message are supplied.

**Push** runs a normal `git push` using the repository's configured destination.
It does not choose a remote, set an upstream, or request a force push. Configure
an upstream and credentials in the terminal first if Git asks for them.

Both actions run in the background, locally or over SSH, with their result
shown below the buttons. A failed commit keeps the message and selection for
retrying; it does not reset the index, so review any staged changes first.
Drafts and in-flight results stay with their
workspace when you switch away; drafts are kept for this app run, not on disk.

## The files pane over SSH

A remote workspace's files are read and written over the SSH connection the
rest of the app already holds open, so the pane behaves the same whichever
machine the project is on. Three mechanisms, and no `scp` or `rsync` on either
side:

- **Listing** is one `find -mindepth 1 -maxdepth 1 -printf` per directory,
  NUL-terminated so a name containing a tab or a newline survives, with the
  entry's own type and its target's type so a symlink to a folder opens like a
  folder. One round trip per branch opened, on the multiplexed connection.
- **Copying** is a `tar` stream through the same `ssh` invocation everything
  else goes through. Going out, the far side unpacks into a staging directory
  beside the destination and moves the result into place — atomic, so a reader
  never sees a half-written folder appear, and it is what lets a copy land under
  a name of our choosing when the answer to a collision is "keep both". Coming
  back, the bytes stage locally and are then placed by the same local code that
  places any other copy, so the naming and replacing rules are written once.
- **Previewing** is one `head -c`, sized against the limit. The file's size and
  type come from the row that was clicked — the tree already listed them — so
  reading a file is one round trip rather than a `stat` and then a `cat`.

None of this happens on the UI thread. The tree does no I/O at all: it holds
what has been read and says what it still wants, and the panel fetches that on a
worker. Every load carries a generation, so a slow listing answering about a
workspace you have since left is discarded rather than filed under the new one.

### What a remote workspace cannot do

**Dragging a file out of the pane into another application** is offered for a
local workspace only. The desktop's drag protocol wants a path the receiving
application can open, and a file on the far side of an SSH connection has none
until it has been fetched — which cannot be done inside the gesture without
freezing the window on a transfer of unknown size. *Copy out of the workspace…*
in the row menu does the same job with the destination chosen first and the
transfer on a worker.

Everything else is unchanged: dropping files in from another application, the
file chooser, the row menu, the preview and the collision prompt all work on a
remote workspace exactly as they do on a local one.

## How it is put together

Two crates, and the split between them is the point:

- **`crates/kraken-core`** — everything the app knows how to *do*, with no
  dependency on Qt: the theme and type scales, persistent state, the Pi RPC
  client and its on-disk configuration, remote SSH workspaces, git, the chat
  pipeline (markdown, syntax highlighting, the transcript model), the terminal
  engine, the dock's layout rules, the file tree and the rules a copy in or out
  of it obeys (on either machine). It is unit-tested without a display — and,
  because the tree does no I/O of its own, the whole of it is testable by
  handing it invented listings.
- **`crates/kraken-qt`** — the interface: a thin layer of `QObject` bridges over
  that core, and the QML that draws it. The QML tree, the fonts and the icons
  are compiled into the binary, so a checkout and a packaged AppImage both find
  them without assuming anything about the layout around them. One file in it is
  C++ rather than Rust — `bridge/clipboard.rs` holds a `cpp!` block reaching
  `QClipboard` for a pasted image, which neither QML nor the Rust bindings can
  see — and it is the only one; anything else that wants Qt goes through the
  bindings.

Two things inside that core are worth naming, because neither is written here:

- Syntax highlighting is `syntect`, with its palette mapped onto the theme's own
  colours, so a diff and a preview are lit the same way as the transcript.
- The terminal is **libghostty-vt** — Ghostty's VT core, reached through the
  `libghostty-vt` crate, which builds it from Ghostty's source with Zig and links
  it statically, so there is no shared object to ship beside the binary. The
  engine is drivable from a test by feeding it a byte string, which is how every
  escape sequence in `terminal::vt` is checked.

What that buys over a hand-written engine: the escape sequences Ghostty knows
and nobody here would have implemented, history that reflows when the window is
resized, selection that understands wrapped lines, and scrollback that costs a
**memory budget** rather than a line count — three terminals holding
about 4,500 rows of history each cost ~26 MB, where fixed-width cells cost ~47 MB
for the same depth no matter how short the lines were.

## Requirements

- Linux, X11 or Wayland
- Rust 1.90+
- **Zig 0.15.2**, on `PATH`, to build the terminal engine
- Qt 6.5 or newer: QtQuick, QtQuick Controls, Layouts, Dialogs
- `pi` on `PATH` for the agent, and `git` for the git pane

Ubuntu/Debian:

```sh
sudo apt install qt6-base-dev qt6-declarative-dev qml6-module-qtquick-controls \
                 qml6-module-qtquick-layouts qml6-module-qtquick-dialogs \
                 libqt6svg6 build-essential pkg-config
```

Zig is not packaged at the version this needs, so take the tarball. It is needed
only to build; nothing links to it at runtime:

```sh
curl -LO https://ziglang.org/download/0.15.2/zig-x86_64-linux-0.15.2.tar.xz
mkdir -p ~/.local/share/zig
tar xf zig-x86_64-linux-0.15.2.tar.xz -C ~/.local/share/zig
export PATH="$HOME/.local/share/zig/zig-x86_64-linux-0.15.2:$PATH"
```

The version is not a preference: `libghostty-vt-sys` pins a Ghostty commit whose
`build.zig.zon` sets a `minimum_zig_version` of 0.15.2, and whose source does not
compile under 0.16 — a newer toolchain gets past the check and then fails in the
build. Neither is caught before the dependency graph is compiled: a missing or
wrong `zig` surfaces as a build-script panic from `libghostty-vt-sys`, minutes
in. The first build clones Ghostty and compiles it, which takes a minute; after
that it is cached.
`.cargo/config.toml` pins the Zig optimize mode to `ReleaseFast` even in debug
builds, because a `Debug` build of the VT core makes feeding a terminal roughly
a hundred times slower.

The embedded browser additionally needs QtWebEngine, which is a separate
package and is optional — without it the browser panel says so and the rest of
the app is unaffected:

```sh
sudo apt install qml6-module-qtwebengine
```

If Qt is installed somewhere unusual, set `QMAKE` to its `qmake6`.

## Run

```sh
cargo run --release
```

## Storage

Kraken stores its own state in:

- `~/.kraken/state.json` — workspaces, SSH hosts, panel layout, font sizes
- `~/.kraken/screenshots` — captures of the browser pane, attached to a prompt
- `~/.kraken/remotes` — the local anchor folder for each remote workspace
- `~/.kraken/ssh` — the control sockets multiplexing each remote's connection
- `~/.kraken/ext` — the pi extension, unpacked
- `~/.kraken/logs` — diagnostic traces, when `--debug` asks for one

`KRAKEN_HOME` moves all of it somewhere else, which is how the tests keep off a
developer's own `~/.kraken`.

Pi's own configuration and sessions are read from `~/.pi/agent/`, which Kraken
shares with `pi` rather than duplicating.

## Debugging a crash

`--debug` writes a trace of what the app was doing — actions, child processes,
Qt's own warnings — and what it cost in memory:

```sh
cargo run --release -- --debug        # ~/.kraken/logs/kraken-<date>-<pid>.log
```

A log that stops without its `exit  clean shutdown` marker ended in a crash, and
the last `action` line before it is the suspect. [DEBUG.md](DEBUG.md) covers what
goes into the trace and how to read it.

## Tests

```sh
cargo test --workspace
```

The core's tests need no display and no network: the terminal engine is driven
with byte strings, the agent client with recorded JSONL, and git with throwaway
repositories built in a temp directory.

## AppImage

```sh
./scripts/build-appimage.sh
```

If FUSE is unavailable, launch the result with
`APPIMAGE_EXTRACT_AND_RUN=1 ./dist/Kraken-x86_64.AppImage`.
