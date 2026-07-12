# Kraken

Kraken is a desktop front-end for the [Pi coding agent](https://github.com/earendil-works/pi-coding-agent).
It combines:

- a chat / agent transcript
- a local terminal
- an embedded browser
- git history and workspace switching
- session history for Pi conversations

## Requirements

- Linux
- Python 3.14
- [uv](https://docs.astral.sh/uv/)
- Node.js 22.19+ and npm (for installing the Pi agent CLI)
- A working `pi` CLI in `PATH`
- Qt / PySide6 runtime support

## Quick start

### From source

```bash
uv sync
uv run python main.py
```

### Packaged zipapp

Build:

```bash
bash packaging/build_pyz.sh
```

Run:

```bash
./kraken.pyz
```

The packaged launcher can self-install into `~/.local/share/kraken` and create a shortcut in `~/.local/bin`.

## What it does

- Keeps one workspace per project folder
- Restores Pi sessions from `~/.pi/agent/sessions`
- Supports multiple live sessions per workspace
- Shows terminal tabs powered by Ghostty's terminal core
- Embeds a browser for links and screenshots
- Provides a git history panel and branch switching
- Supports light and dark themes

## Storage

Kraken stores app state in:

- `~/.kraken/state.json`

Pi sessions are read from:

- `~/.pi/agent/sessions`

## Notes

- The app is currently Linux-focused.
- The terminal backend depends on vendored Ghostty sources and `libghostty-vt`.
- If the Pi agent is unavailable, the UI still opens, but chat features will be limited.

## License

License information will be added before release.
