# Pupo

Pupo is a native desktop front end for the [Pi coding agent](https://github.com/earendil-works/pi-coding-agent),
written in Rust with a Qt/QML interface. It is a port of **Kraken**, and keeps
its shape: a chat transcript beside the panes you need while the agent works.

**Main features:**

- a chat / agent transcript
- an embedded terminal
- an embedded browser
- a diff pane — the files changed since the last commit, with the lines added
  and removed in each; click a file to read its diff, syntax highlighted, over
  the dimmed app
- a files pane — the workspace as a tree, the way an editor draws one; click a
  file to read it over the dimmed app, and drag files in and out of the project
  (or use the row menu, which also takes a folder)
- workspace/project switching, local and over SSH

## How it is put together

Two crates, and the split between them is the point:

- **`crates/pupo-core`** — everything the app knows how to *do*, with no
  dependency on Qt: the theme and type scales, persistent state, the Pi RPC
  client and its on-disk configuration, remote SSH workspaces, git, the chat
  pipeline (markdown, syntax highlighting, the transcript model), the terminal
  engine, the dock's layout rules, the file tree and the rules a copy in or out
  of it obeys. It is unit-tested without a display.
- **`crates/pupo-qt`** — the interface: a thin layer of `QObject` bridges over
  that core, and the QML that draws it. The QML tree, the fonts and the icons
  are compiled into the binary, so a checkout and a packaged AppImage both find
  them without assuming anything about the layout around them.

One piece of Kraken is deliberately not ported the same way:

- Syntax highlighting is `syntect` rather than pygments, with the palette mapped
  onto the same colours.

The terminal, like Kraken's, is **libghostty-vt** — Ghostty's own VT core. Where
Kraken reached it through ctypes, Pupo goes through the `libghostty-vt` crate,
which builds it from Ghostty's source with Zig and links it statically, so there
is no shared object to ship beside the binary. The engine is still drivable from
a test by feeding it a byte string, which is how every escape sequence in
`terminal::vt` is checked.

What that buys over the hand-written engine it replaced: the escape sequences
Ghostty knows and this tree never implemented, history that reflows when the
window is resized, selection that understands wrapped lines, and scrollback that
costs a **memory budget** rather than a line count — three terminals holding
about 4,500 rows of history each cost ~26 MB, where fixed-width cells cost ~47 MB
for the same depth no matter how short the lines were.

## Requirements

- Linux, X11 or Wayland
- Rust 1.90+
- **Zig 0.15.2**, on `PATH`, to build the terminal engine
- Qt 6.5 or newer: QtQuick, QtQuick Controls, Layouts, Dialogs
- `pi` on `PATH` for the agent, and `git` for the diff and git panes

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
tar xf zig-x86_64-linux-0.15.2.tar.xz -C ~/.local/share/zig
export PATH="$HOME/.local/share/zig/zig-x86_64-linux-0.15.2:$PATH"
```

The version is not a preference: `libghostty-vt-sys` pins a Ghostty commit, and
that commit's `build.zig` refuses anything but 0.15.x. The first build clones
Ghostty and compiles it, which takes a minute; after that it is cached.
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

Pupo stores its own state in:

- `~/.pupo/state.json` — workspaces, SSH hosts, panel layout, font sizes
- `~/.pupo/screenshots` — captures of the browser pane, attached to a prompt
- `~/.pupo/remotes` — the local anchor folder for each remote workspace
- `~/.pupo/logs` — diagnostic traces, when `--debug` asks for one

Pi's own configuration and sessions are read from `~/.pi/agent/`, which Pupo
shares with `pi` rather than duplicating.

## Debugging a crash

`--debug` writes a trace of what the app was doing — actions, child processes,
Qt's own warnings — and what it cost in memory:

```sh
cargo run --release -- --debug        # ~/.pupo/logs/pupo-<date>-<pid>.log
```

A log that stops without its `exit  clean shutdown` marker ended in a crash, and
the last `action` line before it is the suspect.

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
`APPIMAGE_EXTRACT_AND_RUN=1 ./dist/Pupo-x86_64.AppImage`.
