# Kraken

Kraken is a native desktop app, similar to OpenAI Codex Desktop and Claude Code Desktop.
It is built on the [Pi coding agent](https://github.com/earendil-works/pi-coding-agent), is lightweight on token usage, and is well suited for local LLM use.


**Main features:**

- [x] a chat / agent transcript
- [x] an embedded terminal
- [x] an embedded browser
- [x] a diff pane — the files changed since the last commit, with the lines
  added and removed in each; click a file to read its diff, syntax
  highlighted, over the dimmed app
- [x] workspace/project switching
- [x] voice input — dictate a prompt, transcribed locally on the CPU

## Minimum Requirements

- Linux ( tested mostly with Ubuntu )

The AppImage bundles its own Python, Qt, Node.js and the Pi agent, so nothing
needs to be installed alongside it.

## Quick start

Download `Kraken-x86_64.AppImage` from the releases page, then:

```bash
chmod +x Kraken-x86_64.AppImage
./Kraken-x86_64.AppImage
```

To build one from a checkout — needs [uv](https://docs.astral.sh/uv/), `curl`,
and a built `libghostty-vt` (see Notes):

```bash
packaging/appimage/build_appimage.sh
```

## Storage

Kraken stores app state in:

- `~/.kraken/state.json`
- `~/.kraken/models` — speech-to-text weights, downloaded on first use

Pi sessions are read from:

- `~/.pi/agent/sessions`

## Voice input

The mic button next to the prompt box records from the default input device
and transcribes it into the prompt. Everything runs locally through
onnxruntime; audio never leaves the machine.

The first click asks to download the model — Parakeet TDT 0.6B v2, English,
~661 MB — into `~/.kraken/models`. It is accurate on code and technical
terms, and punctuates and capitalises.

Click to start, click again to transcribe, `Esc` to discard the take.

## Debugging a crash

`--debug` writes a trace of what the app was doing — actions, child processes,
Qt's own warnings — and what it cost in memory, to a file:

```bash
./Kraken-x86_64.AppImage --debug        # ~/.kraken/logs/kraken-<date>-<pid>.log
```

A log that stops without its `exit  clean shutdown` marker ended in a crash,
and the last `action` line before it is the suspect. See [DEBUG.md](DEBUG.md)
for the log format, how to read one for leaks and hangs, and what is
instrumented.

## Tests

```bash
uv sync --group dev
uv run pytest
```

Widget tests run against Qt's `offscreen` platform, so they need no display.
They drive real widgets through the same panels the app assembles: transcript
behaviour in particular depends on the panel around it, and a widget tested in
isolation will happily pass with the bug still in it.

## Notes

- The app is currently Linux-focused.
- The terminal backend depends on vendored Ghostty sources and `libghostty-vt`.
