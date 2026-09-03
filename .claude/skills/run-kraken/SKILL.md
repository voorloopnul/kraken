---
name: run-kraken
description: Drive Kraken over X11 to see what a change looks like. Use when you need to check a panel renders at all.
---

# Running Kraken for a look

**Never put a window on the user's display without asking.** They work on
this machine at the same time, and a window that appears and takes focus
interrupts them.

## The X11 driver

`driver.py` drives the real window over X11 (`launch`, `shot`, `click`,
`type`, `key`, `quit`). Ask before using it, and quit the window as soon as
you are finished.

## Putting the app in a particular state

`~/.kraken/state.json` is read at startup: it carries the theme, the
workspace list and which panels are open, so a state that would take several
clicks to reach can be written to the file before launching instead.
