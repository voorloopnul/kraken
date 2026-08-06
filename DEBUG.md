# Debugging Kraken

Kraken drives a lot of native machinery — QtWebEngine renderers, libghostty-vt
through ctypes, PTYs, and a `pi` process per session — so when it crashes it
usually crashes in C or C++, and Python has no traceback to offer. The debug
mode records the Python-visible half of the story to a file, so the last lines
before a crash say what the app was doing when it died.

It is off unless asked for, and a no-op when off.

## Turning it on

```bash
./Kraken-x86_64.AppImage --debug                  # ~/.kraken/logs/kraken-<date>-<pid>.log
./Kraken-x86_64.AppImage --debug /tmp/kraken.log  # a specific file
./Kraken-x86_64.AppImage --debug -                # stderr, to pipe
./Kraken-x86_64.AppImage --debug --debug-heartbeat 10   # sample every 10s
```

From a source checkout, `uv run python main.py --debug`.

The environment does the same, for `.desktop` launchers and AppImage wrappers
where passing arguments is awkward:

| Variable | Effect |
| --- | --- |
| `KRAKEN_DEBUG=1` | log to the default path |
| `KRAKEN_DEBUG=/path/to/file` | log there (`-` for stderr) |
| `KRAKEN_DEBUG=0` *(or unset)* | off |
| `KRAKEN_DEBUG_TRACE=1` | also trace raw input — see [Input tracing](#input-tracing) |
| `KRAKEN_DEBUG_HEARTBEAT=10` | sample every 10s — see [The heartbeat](#the-heartbeat) |

The command-line flag wins over the environment. The chosen path is printed to
stderr at start-up. Logs are appended, so a path reused across runs keeps every
run, each starting with a `boot` banner.

## What a log looks like

```
12:17:22.073   0.000 boot    kraken pid=891107 argv=['main.py', '--debug']
12:17:22.073   0.000 boot    python=3.14.4 pyside=6.11.1 qt=6.11.1
12:17:22.073   0.000 boot    session=wayland platform=default desktop=ubuntu:GNOME
12:17:22.078   0.005 boot    ghostty-vt=/home/pascal/Workspace/alpine/vendor/ghostty/zig-out/lib/libghostty-vt.so
12:17:22.486   0.413 action  workspace.select path=/home/pascal/Workspace/alpine remote=False new=True  | rss=153.4MB tree=153.4MB d=+80.5MB procs=1 fds=20 threads=21
12:17:22.510   0.437 event   session.create path=(new) live=1
12:17:22.835   0.762 action  panel.toggle side=right visible=True  | rss=163.5MB tree=163.5MB d=+10.1MB procs=1 fds=20 threads=21
12:17:22.840   0.767 proc    terminal.spawn pid=891233 program=/bin/bash fd=19 remote=False  | rss=164.9MB tree=166.1MB d=+2.6MB procs=2 fds=21 threads=21
12:17:24.660   2.587 proc    pi.start cwd=/home/pascal/Workspace/alpine remote=False args=['--mode', 'rpc']
12:17:27.060   4.987 action  session.new  | rss=179.3MB tree=333.7MB d=+146.0MB procs=3 fds=25 threads=21
12:17:27.756   5.683 proc    terminal.shutdown pid=891233 exited=False reaped=False  | tree=184.3MB d=-145.9MB procs=2 fds=21
12:17:27.762   5.688 event   terminal.freed
12:17:28.061   5.987 exit    clean shutdown code=0  | rss=175.1MB tree=175.1MB d=-9.2MB procs=1 fds=20 threads=21
```

One record per line: wall-clock time, seconds since start-up, the record kind,
then `key=value` fields. Everything is greppable and `awk`-able.

### Record kinds

| Kind | Meaning | Memory snapshot |
| --- | --- | --- |
| `boot` | machine and build facts, written once at start-up | – |
| `action` | something the user did: a click, a menu choice, a session switch | yes |
| `sample` | a timed heartbeat, taken whether or not anything happened | yes |
| `event` | something the app did on its own | – |
| `proc` | a child process or native resource appeared or went away | yes |
| `error` | a failure, with the traceback indented beneath it when there is one | – |
| `qt` | Qt's own diagnostics, forwarded here *and* to stderr | – |
| `exit` | the clean-shutdown marker | yes |

`event` records are frequent and deliberately cheap; the `action` records
around them already bracket the interesting window.

### The heartbeat

Action records only sample when you do something. A process that grows while
sitting untouched — the shape of *"it crashed after I left it open
overnight"* — would leave no trace between them. So memory is also sampled on
a timer, every 60 seconds by default:

```
12:46:34.064   3.095 sample  heartbeat idle=3s   | rss=156.2MB tree=156.2MB d=+79.3MB procs=1 fds=20 threads=21
12:46:37.063   6.094 sample  heartbeat idle=6s   | rss=156.3MB tree=156.3MB d=+0.1MB procs=1 fds=20 threads=21
12:46:40.063   9.094 sample  heartbeat idle=9s   | rss=156.3MB tree=156.3MB d=+0.0MB procs=1 fds=20 threads=21
```

`idle=` is the seconds since the last `action`. It is what separates growth
caused by use from growth that happens on its own: **a rising `tree` alongside
a rising `idle` is a leak nobody triggered**, and that is a very different bug
from one that costs memory per click.

`--debug-heartbeat SECONDS` changes the interval; `0` switches it off. Drop it
to 5–10 s when actively hunting, leave it at 60 for a long unattended run
(~1400 records a day, which stays readable).

At 60 s the samples make a clean series to plot:

```bash
grep " sample " kraken.log | grep -o "tree=[0-9.]*" | cut -d= -f2 > tree.txt
```

A flat line is health. A staircase that only ever climbs is the leak, and the
`action` records interleaved with it tell you which step started it.

### The memory snapshot

Appended to `action`, `proc` and `exit` records, after the `|`:

| Field | Meaning |
| --- | --- |
| `rss` | resident set size of the Kraken process alone |
| `tree` | **this process and every descendant** — renderers, shells, `pi`, ssh |
| `d` | change in `tree` since the previous snapshot |
| `procs` | how many processes that tree covers |
| `fds` | open file descriptors |
| `threads` | live threads |

`tree` is the number worth watching. Most of Kraken's footprint lives in child
processes, so a per-process reading would miss almost all of it — a `pi` spawn
shows up as `+146.0MB` in `tree` while `rss` barely moves.

## Reading one

**A log that just stops is a crash.** A clean exit always ends with
`exit  clean shutdown code=…`. If that line is missing, the app did not shut
down — and the last `action` record before the end is your suspect.

```bash
tail -5 ~/.kraken/logs/kraken-*.log        # did it end cleanly?
grep -n "^.*action" kraken.log | tail -20  # what was happening just before?
```

**A fatal signal appends its own stack.** `faulthandler` writes the faulting
thread's Python stack — and every other thread's — directly into the same file,
so a segfault lands in context rather than in a lost stderr.

**A hang is not a crash.** If the UI locks up but the process lives:

```bash
kill -USR1 <pid>
```

That dumps every thread's stack into the log without disturbing the process.
Do it two or three times a few seconds apart: a stack that never moves is the
one that is stuck.

**Leaks show up as unpaired records.** These come in pairs, and each should
give its memory back:

- `pi.start` → `pi.terminate` / `pi.kill` / `pi.exit`
- `terminal.spawn` → `terminal.shutdown` → `terminal.freed`
- `session.create` → `session.retire`
- `voice.transcribe.start` → `voice.transcribe.end`

A spawn with no matching exit, a `procs` count that only climbs, or a `tree`
that never comes back down after a teardown is the leak.

```bash
grep -E "pi\.(start|exit|terminate|kill)" kraken.log     # balanced?
grep -o "procs=[0-9]*" kraken.log | uniq -c              # monotonic?
awk -F'tree=' '/tree=/{split($2,a," ");print a[1]}' kraken.log   # plot it
```

For a leak that accumulates *without* you touching anything, read the
`sample` records instead — see [The heartbeat](#the-heartbeat).

**`fds=` climbing steadily is its own crash.** PTYs, sockets and pipes all land
there, and the app dies on `EMFILE` long after the leak began.

**Read the `qt` records.** "Cannot create children for a parent in a different
thread", "QObject::setParent: Cannot set parent" and friends are Qt naming the
exact misuse that is about to segfault it. They are forwarded to stderr as
well, so behaviour without `--debug` is unchanged.

**A turn names its model, and it is the agent's answer, not the picker's.**
`chat.submit` and `session.streaming` carry the model the agent reported
through `get_state` — deliberately not the value the model menu last sent,
which would only ever agree with itself:

```
14:22:03.114  action  chat.submit chars=41 images=0 files=0 provider=anthropic model=claude-opus-4-6
14:22:03.140  proc    session.streaming streaming=True focused=True model=claude-opus-4-6 path=…
```

So a `model.select` that does not show up in the next turn's `model=` is a
selection that never reached the agent:

```bash
grep -E "model\.select|chat\.submit" kraken.log   # chosen, then used
```

`model=(pending)` means no `get_state` has come back yet — the first message of
a brand-new session, before the agent has said what it is running. It resolves
on the turn after.

## What is instrumented

The bias is toward native-heavy paths, because that is where the crashes are.

| Area | Records |
| --- | --- |
| Agent (`agent/pi_rpc.py`) | `pi.start`, `pi.terminate`, `pi.kill`, `pi.exit` (with exit code and pending callbacks), `pi.error`, `pi.stderr` |
| Sessions (`shell/workspace_view.py`) | `session.create`, `session.open`, `session.new`, `session.remove`, `session.retire`, `session.streaming`, `chat.submit`, `chat.stop`, `model.select`, `effort.select`, `view.shutdown` |
| Terminal (`terminal/widget.py`) | `terminal.spawn` (pid, program, fd), `terminal.child-exit`, `terminal.shutdown`, `terminal.freed` |
| Browser (`browser/widget.py`, `shell/panels/browser.py`) | `browser.navigate`, `browser.render-terminated` (status and exit code), `browser.tabs-discarded` (last tab closed, web views destroyed) |
| Window (`shell/main_window.py`) | `workspace.select`, `workspace.add`, `workspace.add-remote`, `workspace.edit-remote`, `workspace.remove`, `panel.toggle`, `theme.set`, `settings.open`, `chat.font-size`, `window.state`, `window.close`, `browser.screenshot`, `app.shutdown-views`, `view.discard` |
| Dock (`shell/dock.py`) | `dock.drop` — reparenting a live panel between splitters |
| Diff (`shell/panels/diff.py`) | `diff.open` |
| Git (`shell/title_bar.py`) | `branch.checkout` |
| Voice (`voice/service.py`) | `voice.transcribe.start`, `voice.transcribe.end` |
| Lifecycle (`main.py`) | `app.started`, `app.about-to-quit`, the boot banner, the exit marker |
| Timer (`debug.py`) | `heartbeat`, every 60 s regardless of activity |

`terminal.shutdown` deliberately brackets the riskiest block in the codebase —
a PTY, a child process and a set of libghostty handles all freed by hand. A log
that stops between `terminal.shutdown` and `terminal.freed` says the crash was
in there.

Teardown paths that used to swallow exceptions (`except Exception: pass`) now
log them. Those were hiding exactly the failures that leave a `pi` or a shell
running.

## Input tracing

`--debug-trace` (with `--debug`) additionally logs every mouse press, double
click, drop and key press, naming the widget that received it:

```
12:31:04.117   8.201 input   click button=left at=1180,340 on=QToolButton#panelButton<SideBar<QWidget
12:31:05.802   9.886 input   key.typing keys=23
12:31:06.104  10.188 input   key key=16777220 mods=0 on=ChatInput<CenterPanel<DockPanel
```

**This is a separate flag on purpose.** It installs an application-wide event
filter, which makes PySide wrap every QObject the app delivers to — including
QtWebEngine's internals, which is known to crash it. `shell/main_window.py`
avoids an app-wide filter for exactly this reason (see the `_EdgeGrip` note).
Reach for it when action-level tracing is not enough to locate a crash, and
expect it to be capable of perturbing the failure it is meant to be observing.

Printable keystrokes typed without a command modifier are **counted, not
recorded** (`key.typing keys=23`). Modified and navigation keys are logged by
name. A debug log is a file people paste into bug reports; it must not become
a keylogger.

## Sharing a log

It contains file paths, workspace and branch names, remote SSH destinations,
model ids, and any `pi` stderr output. It does not contain prompt text,
transcript content, or typed characters. Skim it before posting.

## Adding instrumentation

`kraken/debug.py` has no dependencies beyond the standard library and is safe
to import anywhere, including before the `QApplication` exists.

```python
from kraken import debug

debug.action("panel.toggle", side=side, visible=visible)  # user did something
debug.log("session.create", path=path, live=len(self.controllers))  # app did something
debug.proc("pi.start", cwd=cwd, args=args)                # process/native resource moved
debug.error("pi.error", detail=message)
debug.exception("view.shutdown failed", exc)              # inside an except block
```

Every one is a no-op when debugging is off, so no call site needs a guard.
Naming is `area.thing` in lower kebab case, so `grep '^.*action  session\.'`
picks out a whole area.

Two rules worth keeping:

- **Do not instrument hot paths.** Nothing per-frame, per-paint, or per-PTY-read.
  The arguments are built whether or not the log is on.
- **Prefer `proc` for anything that spawns or frees.** It carries the memory
  snapshot, which is what turns a record into leak evidence.

Metrics helpers are here too, and the title bar's memory label reads from the
same ones so the label and the log cannot disagree: `process_rss()`,
`tree_stats()`, `process_tree_rss()`, `open_fds()`, `thread_count()`,
`format_bytes()`.

## Tests

`tests/test_debug_log.py` covers the properties that make the log usable after
a crash rather than the wording of any record: that it is off by default, that
records are readable with no flush and no close, that a clean exit is
distinguishable from a crash, that `tree` really counts descendants, that the
heartbeat keeps sampling while nothing happens and resets `idle=` on an action,
and that input tracing names the clicked widget without recording what was
typed.

```bash
uv run pytest tests/test_debug_log.py
```
