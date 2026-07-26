# Kraken

Kraken is a native desktop app, similar to OpenAI Codex Desktop and Claude Code Desktop.
It is built on the [Pi coding agent](https://github.com/earendil-works/pi-coding-agent), is lightweight on token usage, and is well suited for local LLM use.


**Main features:**

- [x] a chat / agent transcript
- [x] an embedded terminal
- [x] an embedded browser
- [x] workspace/project switching
- [x] voice input — dictate a prompt, transcribed locally on the CPU

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

## Notes

- The app is currently Linux-focused.
- The terminal backend depends on vendored Ghostty sources and `libghostty-vt`.
