import argparse
import os
import sys

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QApplication

from kraken import debug
from kraken.shell.main_window import MainWindow
from kraken.ui.fonts import apply_ui_font, load_fonts


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="kraken", add_help=True)
    parser.add_argument(
        "--debug",
        nargs="?",
        const="",
        metavar="FILE",
        help="Write a diagnostic trace (actions, processes, memory) to FILE. "
        "Defaults to ~/.kraken/logs/; '-' writes to stderr.",
    )
    parser.add_argument(
        "--debug-trace",
        action="store_true",
        help="With --debug, also log every mouse press and key press. "
        "Installs an application-wide event filter, which can itself "
        "destabilise QtWebEngine — use only when --debug is not enough.",
    )
    parser.add_argument(
        "--debug-heartbeat",
        type=float,
        default=debug.HEARTBEAT_DEFAULT,
        metavar="SECONDS",
        help="With --debug, sample memory on a timer as well as on actions, "
        "so drift while the app sits idle is visible. "
        f"Default {debug.HEARTBEAT_DEFAULT:g}s; 0 disables.",
    )
    # Unknown arguments are left alone: Qt reads its own (-style, -platform)
    # straight out of sys.argv when the QApplication is built.
    args, rest = parser.parse_known_args(argv[1:])
    args.qt_argv = [argv[0], *rest]
    return args


def main() -> int:
    args = _parse_args(sys.argv)
    # The flag wins; the environment lets a launcher or AppImage turn tracing on
    # where passing argv is awkward.
    settings = None
    if args.debug is not None:
        settings = debug.Settings(
            path=args.debug or None,
            trace_input=args.debug_trace,
            heartbeat=args.debug_heartbeat,
        )
    elif (from_env := debug.from_environment()) is not None:
        settings = from_env
    if settings is not None:
        path = debug.start(
            settings.path,
            trace_input=settings.trace_input,
            heartbeat=settings.heartbeat,
        )
        if path is not None:
            print(f"kraken: debug log -> {path}", file=sys.stderr, flush=True)

    # Cap Chromium at one shared renderer process: by default every browser
    # tab gets its own (~80MB apiece). Must be in the environment before
    # QtWebEngine initializes; an externally set value is respected.
    os.environ.setdefault(
        "QTWEBENGINE_CHROMIUM_FLAGS", "--renderer-process-limit=1"
    )
    # Required before the QApplication exists for QtWebEngine (the browser
    # panel imports it lazily, after app startup).
    QApplication.setAttribute(Qt.ApplicationAttribute.AA_ShareOpenGLContexts)
    app = QApplication(args.qt_argv)
    app.setApplicationName("Kraken")
    app.setApplicationDisplayName("Kraken")
    debug.install(app)
    load_fonts()
    apply_ui_font(app)
    window = MainWindow()
    window.show()
    debug.log("app.started")
    code = app.exec()
    # Reached only on a clean exit; the marker's absence is what tells you the
    # log ends at a crash rather than at a quit.
    debug.shutdown(code)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
