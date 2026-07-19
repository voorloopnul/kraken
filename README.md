# Kraken

Kraken is a native desktop app, similar to OpenAI Codex Desktop and Claude Code Desktop.
It is built on the [Pi coding agent](https://github.com/earendil-works/pi-coding-agent), is lightweight on token usage, and is well suited for local LLM use.


**Main features:**

- [x] a chat / agent transcript
- [x] an embedded terminal
- [x] an embedded browser
- [x] workspace/project switching

## Minimum Requirements

Before running the installer be sure you have the following:

- Linux ( tested mostly with Ubuntu )
- Python >= 3.8
- Node.js 22.19+ and npm (for installing the Pi agent CLI)

## Quick start

Download the installer from the releases page and execute it:

```bash
python3 kraken.pyz
```
The packaged launcher can self-install into `~/.local/share/kraken` and create a shortcut in `~/.local/bin`.

## Storage

Kraken stores app state in:

- `~/.kraken/state.json`

Pi sessions are read from:

- `~/.pi/agent/sessions`

## Notes

- The app is currently Linux-focused.
- The terminal backend depends on vendored Ghostty sources and `libghostty-vt`.
