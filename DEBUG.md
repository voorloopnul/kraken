# Debugging Pupo

Pupo drives a lot of machinery it does not own — a `pi` process per session, a
pty and a shell per terminal tab, an ssh client per remote command, and Qt's
scene graph under all of it. When it goes wrong it usually goes wrong in one of
those, and the interesting evidence is what the app was doing at the time and
what it cost.

`--debug` records that to a file. It is off unless asked for, and a relaxed
atomic load when off.

## Turning it on

```sh
pupo --debug                        # ~/.pupo/logs/pupo-<date>-<pid>.log
pupo --debug /tmp/pupo.log          # a specific file
pupo --debug -                      # stderr, to pipe
pupo --debug --debug-heartbeat 10   # sample memory every 10s
pupo --debug --debug-trace          # also record every key and click
```

From a checkout, `cargo run --release -- --debug`.

The environment does the same, for a `.desktop` launcher or an AppImage wrapper
where passing arguments is awkward:

| Variable | Effect |
| --- | --- |
| `PUPO_DEBUG=1` | log to the default path |
| `PUPO_DEBUG=/path/to/file` | log there (`-` for stderr) |
| `PUPO_DEBUG=0` *(or unset)* | off |
| `PUPO_DEBUG_TRACE=1` | also trace raw input |
| `PUPO_DEBUG_HEARTBEAT=10` | sample every 10 s (`0` disables) |

The flag wins where both are given. The chosen path is printed to stderr at
start-up. Logs are appended, so a path reused across runs keeps every run, each
starting with a `boot` banner.

## What a log looks like

```
00:14:08.063     0.000 boot     pupo pid=856118 argv=["/usr/bin/pupo", "--debug"]
00:14:08.063     0.000 boot     session=wayland platform=default desktop=ubuntu:GNOME
00:14:08.078     0.016 event    app.started
00:14:08.206     0.143 proc     terminal.spawn pid=856126 program=/bin/bash remote=false  | rss=104.7MB tree=108.8MB d=+108.8MB procs=2 fds=10 threads=5
00:14:08.214     0.151 proc     terminal.tab-opened id=1  | rss=104.9MB tree=110.5MB d=+1.7MB procs=2 fds=11 threads=6
00:14:09.072     1.010 mem      heartbeat idle=1s  | rss=121.0MB tree=129.2MB d=+18.7MB procs=2 fds=11 threads=6
00:14:10.320     2.257 action   chat.submit chars=42 images=0 files=0  | rss=121.2MB tree=129.4MB d=+0.2MB procs=2 fds=11 threads=6
00:14:10.402     2.339 proc     pi.start pid=856140 cwd=/home/pascal/Workspace/pupo remote=false  | rss=121.4MB tree=268.0MB d=+138.6MB procs=3 fds=13 threads=6
00:14:11.295     3.232 proc     terminal.shutdown pid=856126  | rss=126.1MB tree=134.1MB d=-133.9MB procs=2 fds=11 threads=6
00:14:11.338     3.276 exit     clean shutdown code=0  | rss=126.2MB tree=125.7MB d=-8.3MB procs=1 fds=9 threads=5
```

One record per line: wall-clock time, seconds since start-up, the record kind,
then the message and its `key=value` fields. Everything is greppable and
`awk`-able.

### Record kinds

| Kind | Meaning | Memory snapshot |
| --- | --- | --- |
| `boot` | machine facts, written once at start-up | – |
| `action` | something the user did: a click, a menu choice, a session switch | yes |
| `event` | something the app did on its own | – |
| `proc` | a child process appeared or went away | yes |
| `mem` | a timed heartbeat, taken whether or not anything happened | yes |
| `error` | a failure; a caught one indents its detail beneath the record | – |
| `exit` | the clean-shutdown marker | yes |

`event` records are frequent and deliberately cheap; the `action` records
around them already bracket the interesting window.

### The memory snapshot

Appended to `action`, `proc`, `mem` and `exit` records, after the `|`:

| Field | Meaning |
| --- | --- |
| `rss` | resident set size of the Pupo process alone |
| `tree` | **this process and every descendant** — `pi`, shells, ssh |
| `d` | change in `tree` since the previous snapshot |
| `procs` | how many processes that tree covers |
| `fds` | open file descriptors |
| `threads` | live threads |

`tree` is the number worth watching. Most of Pupo's footprint lives in child
processes, so a per-process reading would miss almost all of it — a `pi` spawn
shows as `+138.6MB` in `tree` while `rss` barely moves.

### The heartbeat

Action records only sample when you do something. A process that grows while
sitting untouched — the shape of *"it was fine until I left it open
overnight"* — would leave no trace between them. So memory is also sampled on a
timer, every 60 seconds by default, on a thread of its own: a sample taken on
the event loop stops exactly when the event loop is the thing that has wedged.

```
00:15:09.072    61.010 mem      heartbeat idle=58s  | rss=121.0MB tree=129.2MB d=+0.0MB procs=2 fds=11 threads=6
```

`idle=` is the seconds since the last `action`, and it is the field that
separates growth caused by use from growth that happens on its own: **a rising
`tree` beside a rising `idle` is a leak nobody triggered**, which is a very
different bug from one that costs memory per click.

Drop `--debug-heartbeat` to 5–10 s when actively hunting; leave it at 60 for a
long unattended run (~1400 records a day, which stays readable). At 60 s the
samples make a clean series to plot:

```sh
grep " mem " pupo.log | grep -o "tree=[0-9.]*" | cut -d= -f2 > tree.txt
```

A flat line is health. A staircase that only climbs is the leak, and the
`action` records interleaved with it say which step started it.

## Reading one

**A log that just stops is a crash.** A clean exit always ends with
`exit  clean shutdown code=…`. If that line is missing the app did not shut
down, and the last `action` before the end is the suspect.

```sh
tail -5 ~/.pupo/logs/pupo-*.log            # did it end cleanly?
grep " action " pupo.log | tail -20        # what was happening just before?
```

**Leaks show up as unpaired records.** These come in pairs, and each should give
its memory back:

- `pi.start` → `pi.terminate` → `pi.exit`, or `pi.kill` when the terminate was
  ignored
- `terminal.spawn` → `terminal.shutdown`
- `terminal.tab-opened` → `terminal.tab-closed`

A spawn with no matching exit, a `procs` count that only climbs, or a `tree`
that never comes back down after a teardown is the leak.

```sh
grep -E "pi\.(start|terminate|kill|exit)" pupo.log   # balanced?
grep -o "procs=[0-9]*" pupo.log | uniq -c            # monotonic?
```

**`pi.kill` is worth reading on its own.** It means a `pi` ignored `SIGTERM`
and had to be killed — and a pi that will not die is also one still holding its
memory and its SSH connections.

**`fds=` climbing steadily is its own crash.** Ptys, sockets and pipes all land
there, and the app dies on `EMFILE` long after the leak began.

**Which model answered is the agent's word, not the picker's.** `model.select`
records what was asked for; the model a turn actually ran on comes back from
`pi` itself. Where they disagree, the second one is true.

## Input tracing

`--debug-trace` adds an `event terminal.key` record per keystroke sent to a
terminal, with Qt's key code, its modifier flags and the text the event carried:

```
00:16:02.114    12.4 event    terminal.key key=0x1000012 mods=0x4000000 text=""
```

It is off by default because it is noisy and because a log of everything typed
is not something to write to disk unasked. Use it when a chord is not arriving
where it should — a key that reaches the shell as the wrong bytes looks
identical from outside to one that never arrived at all.

## What is *not* in here

Pupo is Rust: a panic already prints its own backtrace to stderr (set
`RUST_BACKTRACE=1` for the full one), and there is no equivalent of a Python
faulthandler dump to fold into the log. A crash inside Qt or a child process
shows here only as the log stopping — which is exactly what the last `action`
line is for.
