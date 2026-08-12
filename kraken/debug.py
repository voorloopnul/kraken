"""Crash diagnostics: an opt-in trace of what the app did and what it cost.

Kraken drives a lot of native machinery — QtWebEngine renderers, libghostty-vt
through ctypes, PTYs, and a `pi` process per session — so when it goes down it
usually goes down in C or C++, with no Python traceback left behind. This
module records the Python-visible half of the story to a file, so the last
lines before a crash say what the app was doing when it died.

It is off unless asked for (``--debug``, or ``KRAKEN_DEBUG`` in the
environment) and a cheap no-op when off.

Three things make the log usable *after* a hard crash:

* the file is line-buffered, so every record reaches the OS the moment it is
  written — a segfault loses nothing already logged;
* ``faulthandler`` writes the fatal signal and every thread's Python stack into
  that same file, so the fault lands in context rather than in a lost stderr;
* a clean shutdown ends with an explicit ``exit`` record. **A log that stops
  without one is a crash**, and the last ``action`` line before it is the
  suspect.

Records are one line each, ``key=value`` throughout so awk and grep can take
them apart::

    21:04:12.913    12.802 action   panel.toggle side=right visible=True | \
rss=412.3MB tree=1432.8MB d=+1.2MB procs=6 fds=143 threads=12

The memory snapshot is appended to `action` records only: `tree` covers this
process *and* its descendants (renderers, shells, agents), which is the number
that actually grows when Kraken leaks.
"""

from __future__ import annotations

import ctypes
import faulthandler
import os
import signal
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path

_DARWIN = sys.platform == "darwin"

# Seconds between heartbeat samples. Frequent enough that an overnight run
# still fits in a readable file (~1400 records/day), fine enough to turn slow
# drift into a visible slope.
HEARTBEAT_DEFAULT = 60.0


@dataclass(frozen=True)
class Settings:
    """How debugging was asked for, from the command line or the environment."""

    path: str | None = None  # None means the default path, "-" means stderr
    trace_input: bool = False
    heartbeat: float = HEARTBEAT_DEFAULT  # seconds; 0 disables

# The open log stream, or None when debugging is off. Every entry point checks
# this first, so an un-started module costs one global lookup per call.
_file = None
_path: Path | None = None
_close_file = False  # False when _file is a borrowed stream (stderr)
_start = 0.0  # monotonic origin for the elapsed column
_lock = threading.Lock()  # worker threads log too (async_run, voice)
_last_tree_rss = 0  # previous action's tree RSS, for the delta
_trace_input = False  # whether raw input tracing was asked for
_tracer = None  # the input event filter, kept alive here
_hooks_installed = False  # excepthooks are process-wide and installed once
_heartbeat_seconds = HEARTBEAT_DEFAULT
_heartbeat_timer = None
_last_action_at = 0.0  # monotonic time of the last action, for `idle=`


# ---------------------------------------------------------------------------
# Process metrics
#
# Linux reads all four numbers straight out of /proc. macOS has no /proc, so
# each one has a darwin path below. Three of the four come from libproc, which
# answers about this process without leaving it: PROC_PIDTASKINFO carries both
# the resident size and the thread count, and PROC_PIDLISTFDS the open
# descriptors. Only the tree needs the whole process table, and only that
# forks a `ps`.
#
# Both libproc calls are made with a real buffer. Asking with a null one
# returns how much space the answer *could* need rather than how much it takes
# — for the fd list that is the table's capacity plus slop, nearly double the
# truth, and for threads it is not answered at all.
# ---------------------------------------------------------------------------

_PROC_PIDLISTFDS = 1  # entries are struct proc_fdinfo: two 32-bit fields
_PROC_FDINFO_SIZE = 8
_PROC_PIDTASKINFO = 4  # one struct proc_taskinfo
# Read out by offset rather than laid out in ctypes: the struct is 18 fields of
# a stable public ABI, and exactly two of them are wanted here.
_TASKINFO_SIZE = 96
_TASKINFO_RSS = (8, 16)  # pti_resident_size, uint64
_TASKINFO_THREADS = (84, 88)  # pti_threadnum, int32
_libproc: ctypes.CDLL | None = None
_libproc_looked_up = False


def _ps_table() -> dict[int, tuple[int, int]]:
    """pid -> (ppid, rss in bytes) for every visible process, from one `ps`.

    Read fresh each time. This was cached for a quarter second, back when one
    record took two readings from it — but a cache outliving the gap between
    two *records* made consecutive records report identical memory and a zero
    delta, which is exactly the moment a spawn or a leak is what the log is
    being read for. The readings a record wants no longer both come from here,
    so there is one `ps` per record either way.
    """
    table: dict[int, tuple[int, int]] = {}
    try:
        # Popen rather than run() so the reader's own pid is known: ps is our
        # child and lists itself, which would otherwise add a process and a
        # couple of megabytes to every tree reading we take.
        reader = subprocess.Popen(
            ["/bin/ps", "-axo", "pid=,ppid=,rss="],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
    except OSError:
        return table
    try:
        out, _ = reader.communicate(timeout=5)
    except (OSError, subprocess.SubprocessError):
        # Including TimeoutExpired, which leaves the child running: a `ps` that
        # hung is still ours to kill and reap. Leaving it would leak the kind
        # of process this module exists to notice, and the next reading would
        # count it.
        reader.kill()
        try:
            reader.communicate(timeout=5)
        except (OSError, subprocess.SubprocessError):
            pass
        return table
    for line in out.splitlines():
        parts = line.split()
        if len(parts) != 3:
            continue
        try:
            pid, ppid, rss = (int(part) for part in parts)
        except ValueError:
            continue
        if pid == reader.pid:
            continue
        table[pid] = (ppid, rss * 1024)
    return table


def _proc_pidinfo(flavor: int, buffer, size: int) -> int:
    """Bytes proc_pidinfo wrote into `buffer` for this process."""
    global _libproc, _libproc_looked_up
    if not _libproc_looked_up:
        _libproc_looked_up = True
        try:
            lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
            lib.proc_pidinfo.restype = ctypes.c_int
            lib.proc_pidinfo.argtypes = [
                ctypes.c_int,
                ctypes.c_int,
                ctypes.c_uint64,
                ctypes.c_void_p,
                ctypes.c_int,
            ]
            _libproc = lib
        except (OSError, AttributeError):
            _libproc = None
    if _libproc is None:
        return 0
    return _libproc.proc_pidinfo(os.getpid(), flavor, 0, buffer, size)


def _darwin_taskinfo() -> tuple[int, int]:
    """(resident bytes, threads) for this process, in the one syscall that
    carries both. Zeroes if libproc would not answer, which the callers read
    as "ask something else"."""
    buf = ctypes.create_string_buffer(_TASKINFO_SIZE)
    if _proc_pidinfo(_PROC_PIDTASKINFO, buf, _TASKINFO_SIZE) < _TASKINFO_SIZE:
        return 0, 0
    raw = buf.raw
    rss = int.from_bytes(raw[slice(*_TASKINFO_RSS)], sys.byteorder)
    threads = int.from_bytes(
        raw[slice(*_TASKINFO_THREADS)], sys.byteorder, signed=True
    )
    return rss, max(threads, 0)


def process_rss() -> int:
    """Resident set size of this process alone, in bytes."""
    if _DARWIN:
        # The `ps` table can answer this too, and did, but it costs a
        # subprocess; libproc has the same number a syscall away. The table is
        # still the fallback for a machine whose libproc would not load.
        return _darwin_taskinfo()[0] or _ps_table().get(os.getpid(), (0, 0))[1]
    try:
        with open("/proc/self/status") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        pass
    return 0


def tree_stats() -> tuple[int, int]:
    """Resident set size in bytes of this process and all its descendants
    (QtWebEngine renderers, terminal shells, agent processes), and how many
    processes that covers."""
    if _DARWIN:
        return _darwin_tree_stats()
    total = 0
    count = 0
    stack = [os.getpid()]
    while stack:
        pid = stack.pop()
        proc = Path(f"/proc/{pid}")
        try:
            with open(proc / "status") as f:
                for line in f:
                    if line.startswith("VmRSS:"):
                        total += int(line.split()[1]) * 1024
                        break
            count += 1
            # Children are listed per thread, so walk every task dir.
            for task in (proc / "task").iterdir():
                children = (task / "children").read_text().split()
                stack.extend(int(child) for child in children)
        except OSError:
            continue  # process exited mid-scan
    return total, count


def _darwin_tree_stats() -> tuple[int, int]:
    """tree_stats from the `ps` snapshot: descendants have to be found by
    inverting the parent links, since nothing on macOS lists a process's
    children directly."""
    table = _ps_table()
    children: dict[int, list[int]] = {}
    for pid, (ppid, _) in table.items():
        children.setdefault(ppid, []).append(pid)
    total = 0
    count = 0
    stack = [os.getpid()]
    seen = set()
    while stack:
        pid = stack.pop()
        # A pid recycled into a cycle would otherwise hang the walk, and this
        # runs inside crash diagnostics, where a hang is the worst outcome.
        if pid in seen or pid not in table:
            continue
        seen.add(pid)
        total += table[pid][1]
        count += 1
        stack.extend(children.get(pid, ()))
    return total, count


def process_tree_rss() -> int:
    """Resident set size in bytes of this process and all its descendants."""
    return tree_stats()[0]


def open_fds() -> int:
    """How many file descriptors this process holds. A steadily climbing count
    is its own kind of crash: PTYs, sockets and pipes all land here, and the
    app dies on EMFILE long after the leak started."""
    if _DARWIN:
        # Sized, then filled. The sizing call reports the fd table's capacity
        # plus slop rather than its occupancy — roughly double the truth, and
        # growing in doubling steps, so a leak would sit invisible inside the
        # slack until the table happened to grow. The bytes the second call
        # writes are the descriptors that are actually open.
        capacity = _proc_pidinfo(_PROC_PIDLISTFDS, None, 0)
        if capacity <= 0:
            return 0
        buf = ctypes.create_string_buffer(capacity)
        written = _proc_pidinfo(_PROC_PIDLISTFDS, buf, capacity)
        return max(written, 0) // _PROC_FDINFO_SIZE
    try:
        return len(os.listdir("/proc/self/fd"))
    except OSError:
        return 0


def thread_count() -> int:
    if _DARWIN:
        return _darwin_taskinfo()[1] or _darwin_thread_count_from_ps()
    try:
        return len(os.listdir("/proc/self/task"))
    except OSError:
        return 0


def _darwin_thread_count_from_ps() -> int:
    """Fallback for a libproc that would not load: `ps -M` prints one line per
    thread under a header. It costs a subprocess, which is why it is the
    second choice — but it counts the native threads that
    threading.active_count() cannot see."""
    try:
        out = subprocess.run(
            ["/bin/ps", "-M", "-p", str(os.getpid())],
            capture_output=True,
            text=True,
            timeout=5,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return 0
    return max(len(out.splitlines()) - 1, 0)


def format_bytes(size: int) -> str:
    """Human-readable size for the UI (coarse, stable width)."""
    if size >= 1024**3:
        return f"{size / 1024**3:.1f} GB"
    return f"{size // 1024**2} MB"


def _mb(size: int) -> str:
    """Log-precision size: always MB with one decimal, no space, so a record
    stays one whitespace-separated token per field."""
    return f"{size / 1024**2:.1f}MB"


def _memory_suffix() -> str:
    global _last_tree_rss
    rss = process_rss()
    tree, procs = tree_stats()
    delta = tree - _last_tree_rss if _last_tree_rss else 0
    _last_tree_rss = tree
    return (
        f"| rss={_mb(rss)} tree={_mb(tree)} d={'+' if delta >= 0 else '-'}"
        f"{_mb(abs(delta))} procs={procs} fds={open_fds()} threads={thread_count()}"
    )


# ---------------------------------------------------------------------------
# Writing
# ---------------------------------------------------------------------------


def enabled() -> bool:
    return _file is not None


def _write(kind: str, message: str, memory: bool = False) -> None:
    if _file is None:
        return
    now = time.time()
    stamp = f"{time.strftime('%H:%M:%S', time.localtime(now))}.{int(now % 1 * 1000):03d}"
    line = f"{stamp} {time.monotonic() - _start:9.3f} {kind:<8} {message}"
    if memory:
        line = f"{line}  {_memory_suffix()}"
    try:
        with _lock:
            _file.write(line + "\n")
    except (OSError, ValueError):
        # A closed or broken sink must never take the app down with it — the
        # whole point of this module is to survive whatever is going wrong.
        pass


def _fields(fields: dict) -> str:
    return " ".join(f"{key}={value}" for key, value in fields.items())


def action(what: str, **fields) -> None:
    """Record a user-driven action — a click, a menu choice, a session switch —
    with a memory snapshot taken right after it. This is the line you read
    first when the log ends mid-crash."""
    global _last_action_at
    if _file is None:
        return
    _last_action_at = time.monotonic()
    _write("action", f"{what} {_fields(fields)}".rstrip(), memory=True)


def log(what: str, **fields) -> None:
    """Record something the app did on its own: a process starting, an event
    arriving, a panel rebuilding. No memory snapshot — these are frequent, and
    the surrounding `action` records already bracket them."""
    if _file is None:
        return
    _write("event", f"{what} {_fields(fields)}".rstrip())


def proc(what: str, **fields) -> None:
    """Record a child-process transition (spawn, exit, kill). Carries a memory
    snapshot: every one of these moves the tree total, and a spawn that is
    never matched by an exit is a leak."""
    if _file is None:
        return
    _write("proc", f"{what} {_fields(fields)}".rstrip(), memory=True)


def error(what: str, **fields) -> None:
    if _file is None:
        return
    _write("error", f"{what} {_fields(fields)}".rstrip())


def exception(what: str, exc: BaseException) -> None:
    """Record a caught exception with its traceback, indented under the record
    so the one-line-per-record shape still holds for grep."""
    if _file is None:
        return
    import traceback

    text = "".join(
        traceback.format_exception(type(exc), exc, exc.__traceback__)
    ).rstrip()
    _write("error", f"{what} {type(exc).__name__}: {exc}")
    for line in text.splitlines():
        _write("error", f"    {line}")


# ---------------------------------------------------------------------------
# Start-up
# ---------------------------------------------------------------------------


def default_path() -> Path:
    stamp = time.strftime("%Y%m%d-%H%M%S")
    return Path.home() / ".kraken" / "logs" / f"kraken-{stamp}-{os.getpid()}.log"


def from_environment() -> Settings | None:
    """Debug settings from the environment, or None if not asked for. Lets the
    AppImage and desktop launchers turn this on where passing argv is awkward:

        KRAKEN_DEBUG=1                  log to the default path
        KRAKEN_DEBUG=/path/to/file      log there ("-" for stderr)
        KRAKEN_DEBUG_TRACE=1            also trace raw input events
        KRAKEN_DEBUG_HEARTBEAT=10       sample every 10s (0 disables)
    """
    value = os.environ.get("KRAKEN_DEBUG", "")
    if not value or value == "0":
        return None
    try:
        heartbeat = float(os.environ.get("KRAKEN_DEBUG_HEARTBEAT", HEARTBEAT_DEFAULT))
    except ValueError:
        heartbeat = HEARTBEAT_DEFAULT
    return Settings(
        path=None if value == "1" else value,
        trace_input=os.environ.get("KRAKEN_DEBUG_TRACE", "") not in ("", "0"),
        heartbeat=heartbeat,
    )


def start(
    destination: str | None = None,
    trace_input: bool = False,
    heartbeat: float = HEARTBEAT_DEFAULT,
) -> Path | None:
    """Begin logging to `destination` ("-" for stderr, None for the default
    path under ~/.kraken/logs). Returns the path written to, or None for
    stderr. Calling it twice keeps the first sink.

    `heartbeat` only takes effect once install() is given a QApplication —
    sampling needs an event loop to tick on."""
    global _file, _path, _close_file, _start, _last_tree_rss, _trace_input
    global _heartbeat_seconds
    if _file is not None:
        return _path
    _start = time.monotonic()
    _trace_input = trace_input
    _heartbeat_seconds = heartbeat

    if destination == "-":
        _file, _path, _close_file = sys.stderr, None, False
    else:
        path = Path(destination).expanduser() if destination else default_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        # buffering=1 is line buffering: each record hits the OS as it is
        # written, so a segfault costs nothing already logged. errors="replace"
        # because terminal output and tool results reach this file unfiltered.
        _file = open(path, "a", buffering=1, encoding="utf-8", errors="replace")
        _path, _close_file = path, True

    _install_process_hooks()
    _write("boot", "=" * 60)
    _banner()
    # Seed the delta baseline so the first action's `d=` is measured from
    # start-up rather than showing the whole process as growth.
    _last_tree_rss = tree_stats()[0]
    if trace_input:
        _write("boot", "input tracing enabled")
    return _path


def _banner() -> None:
    """Everything a crash report needs about the machine, written once up
    front: which display server, which Qt, which ghostty library."""
    from PySide6 import __version__ as pyside_version
    from PySide6.QtCore import qVersion

    _write("boot", f"kraken pid={os.getpid()} argv={sys.argv}")
    _write(
        "boot",
        f"python={sys.version.split()[0]} pyside={pyside_version} qt={qVersion()}",
    )
    _write(
        "boot",
        "session=%s platform=%s desktop=%s"
        % (
            os.environ.get("XDG_SESSION_TYPE", "?"),
            os.environ.get("QT_QPA_PLATFORM", "default"),
            os.environ.get("XDG_CURRENT_DESKTOP", "?"),
        ),
    )
    try:
        from kraken.terminal import ghostty_vt

        _write("boot", f"ghostty-vt={ghostty_vt._LIB_PATH}")
    except Exception as exc:  # the library is optional until a terminal opens
        _write("boot", f"ghostty-vt=unavailable ({type(exc).__name__}: {exc})")


def _install_process_hooks() -> None:
    """Catch the three ways the process can die noisily: a fatal signal, an
    unhandled exception on the GUI thread, and one on a worker thread."""
    global _hooks_installed
    # faulthandler is re-pointed at each new sink, but the excepthooks wrap
    # whatever was there before — installing them twice would nest the wrapper
    # inside itself and log every exception once per start/shutdown cycle.
    # faulthandler writes to the fd directly, bypassing our buffer, so the
    # fatal traceback lands in the same file as everything else.
    faulthandler.enable(file=_file, all_threads=True)
    # A hung UI is the other half of the problem: `kill -USR1 <pid>` dumps
    # every thread's stack into the log without touching the app.
    if hasattr(signal, "SIGUSR1"):
        faulthandler.register(signal.SIGUSR1, file=_file, all_threads=True)

    if _hooks_installed:
        return
    _hooks_installed = True

    previous_hook = sys.excepthook

    def hook(exc_type, exc, tb) -> None:
        # PySide aborts the process on an exception that escapes a slot, so
        # this is often the last thing that happens before the crash.
        _write("error", f"unhandled {exc_type.__name__}: {exc}")
        import traceback

        for line in "".join(
            traceback.format_exception(exc_type, exc, tb)
        ).rstrip().splitlines():
            _write("error", f"    {line}")
        previous_hook(exc_type, exc, tb)

    sys.excepthook = hook

    previous_thread_hook = threading.excepthook

    def thread_hook(args) -> None:
        _write(
            "error",
            f"unhandled in thread {args.thread.name if args.thread else '?'}: "
            f"{args.exc_type.__name__}: {args.exc_value}",
        )
        previous_thread_hook(args)

    threading.excepthook = thread_hook


def install(app) -> None:
    """Wire the Qt-side hooks. Call once, after the QApplication exists."""
    if _file is None:
        return
    _install_message_handler()
    app.aboutToQuit.connect(lambda: log("app.about-to-quit"))
    if _heartbeat_seconds > 0:
        _start_heartbeat(app)
    if _trace_input:
        _install_input_tracer(app)


def _heartbeat() -> None:
    """Sample memory on a timer, whether or not anything happened.

    Action records only sample when the user does something, so a process that
    grows while sitting idle leaves no trace between them — which is exactly
    the shape of "it crashed after I left it open overnight". `idle=` is the
    seconds since the last action, so growth caused by use can be told apart
    from growth that happens on its own: a rising `tree` with a rising `idle`
    is a leak nobody triggered.
    """
    if _file is None:
        return
    since = time.monotonic() - (_last_action_at or _start)
    _write("sample", f"heartbeat idle={since:.0f}s", memory=True)


def _start_heartbeat(app) -> None:
    global _heartbeat_timer
    from PySide6.QtCore import QTimer

    # Parented to the application so it dies with it, and so nothing else has
    # to keep it alive. The sample itself is a handful of /proc reads.
    _heartbeat_timer = QTimer(app)
    _heartbeat_timer.setInterval(max(1, int(_heartbeat_seconds * 1000)))
    _heartbeat_timer.timeout.connect(_heartbeat)
    _heartbeat_timer.start()
    _write("boot", f"heartbeat every {_heartbeat_seconds:g}s")


def _install_message_handler() -> None:
    """Route Qt's own diagnostics into the log. These matter more than they
    look: "QObject::setParent: Cannot set parent", "Cannot create children for
    a parent in a different thread" and friends are Qt telling you about the
    exact misuse that is about to segfault."""
    from PySide6.QtCore import QtMsgType, qInstallMessageHandler

    levels = {
        QtMsgType.QtDebugMsg: "debug",
        QtMsgType.QtInfoMsg: "info",
        QtMsgType.QtWarningMsg: "warning",
        QtMsgType.QtCriticalMsg: "critical",
        QtMsgType.QtFatalMsg: "fatal",
    }

    def handler(mode, context, message) -> None:
        where = ""
        if context.file:
            where = f" at={context.file}:{context.line}"
        _write("qt", f"[{levels.get(mode, '?')}] {message}{where}")
        # Keep Qt's messages on stderr as well: installing a handler otherwise
        # silences the console output the app had without --debug.
        print(f"[qt] {message}", file=sys.stderr, flush=True)

    qInstallMessageHandler(handler)


def shutdown(code: int | None = None) -> None:
    """Close the log with the marker that tells a reader this was a clean exit.
    Its absence at the end of a file is the crash signal."""
    global _file, _heartbeat_timer
    if _file is None:
        return
    if _heartbeat_timer is not None:
        # Stopped before the file closes: a tick landing on a dead sink is
        # harmless but pointless, and the timer outlives us if the app doesn't.
        try:
            _heartbeat_timer.stop()
            _heartbeat_timer.deleteLater()
        except RuntimeError:
            pass  # its C++ half went with the application
        _heartbeat_timer = None
    _write("exit", f"clean shutdown code={code}", memory=True)
    # Unhook faulthandler before the file goes: it holds the raw fd, and a
    # fault after the close would write into a descriptor we no longer own.
    faulthandler.disable()
    if hasattr(signal, "SIGUSR1"):
        faulthandler.unregister(signal.SIGUSR1)
    if _close_file:
        try:
            _file.close()
        except OSError:
            pass
    _file = None


# ---------------------------------------------------------------------------
# Raw input tracing (opt-in within opt-in)
# ---------------------------------------------------------------------------


def _describe(obj) -> str:
    """Name a widget and its first few ancestors, defensively: this runs on
    objects that may be mid-destruction."""
    import shiboken6

    try:
        if not shiboken6.isValid(obj):
            return "<dead>"
        name = type(obj).__name__
        if obj.objectName():
            name += f"#{obj.objectName()}"
        parts = [name]
        parent = obj.parent()
        while parent is not None and len(parts) < 4 and shiboken6.isValid(parent):
            parts.append(type(parent).__name__)
            parent = parent.parent()
        return "<".join(parts)
    except Exception:
        return "<?>"


def _install_input_tracer(app) -> None:
    """Log every mouse press, drop and key press, application-wide.

    Deliberately separate from the rest of this module and off by default.
    An application-wide event filter makes PySide wrap *every* QObject the app
    delivers to, including QtWebEngine's internals — which is known to crash
    (see the _EdgeGrip note in shell/main_window.py, where an app filter was
    avoided for exactly this reason). Turn it on when action-level tracing
    isn't enough to locate a crash, and be ready for it to change the failure
    it is meant to be observing.
    """
    global _tracer
    from PySide6.QtCore import QEvent, QObject, Qt

    buttons = {
        Qt.MouseButton.LeftButton: "left",
        Qt.MouseButton.RightButton: "right",
        Qt.MouseButton.MiddleButton: "middle",
    }

    class _InputTracer(QObject):
        def __init__(self):
            super().__init__()
            self._typed = 0  # printable keystrokes since the last other event

        def _flush_typing(self) -> None:
            if self._typed:
                _write("input", f"key.typing keys={self._typed}")
                self._typed = 0

        def eventFilter(self, obj, event) -> bool:
            try:
                kind = event.type()
                if kind == QEvent.Type.MouseButtonPress:
                    self._flush_typing()
                    pos = event.globalPosition().toPoint()
                    _write(
                        "input",
                        f"click button={buttons.get(event.button(), 'other')} "
                        f"at={pos.x()},{pos.y()} on={_describe(obj)}",
                    )
                elif kind == QEvent.Type.MouseButtonDblClick:
                    self._flush_typing()
                    _write("input", f"double-click on={_describe(obj)}")
                elif kind == QEvent.Type.Drop:
                    self._flush_typing()
                    _write("input", f"drop on={_describe(obj)}")
                elif kind == QEvent.Type.KeyPress:
                    modifiers = event.modifiers()
                    plain = modifiers in (
                        Qt.KeyboardModifier.NoModifier,
                        Qt.KeyboardModifier.ShiftModifier,
                    )
                    # Printable keys typed without a command modifier are
                    # counted, not recorded: a debug log is a file people paste
                    # into bug reports, and it must not become a keylogger.
                    if plain and event.text().isprintable() and event.text().strip():
                        self._typed += 1
                    else:
                        self._flush_typing()
                        _write(
                            "input",
                            f"key key={event.key()} mods={int(modifiers.value)} "
                            f"on={_describe(obj)}",
                        )
            except Exception:
                pass  # tracing must never break event delivery
            return False

    _tracer = _InputTracer()
    app.installEventFilter(_tracer)
