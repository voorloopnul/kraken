"""The speech-to-text model: where it lives on disk and how it gets there.

Kraken transcribes with Parakeet TDT 0.6B v2 (English), stored under
~/.kraken/models/ in the layout its upstream Hugging Face repo uses so the
files can be handed straight to onnx-asr. It is a transducer rather than an
autoregressive decoder, so despite the parameter count it transcribes a
spoken prompt in a fraction of its duration on a CPU, and it punctuates and
capitalises — worth the one-off download for prompts full of identifiers,
paths and library names.

The int8 export is the one we want: the fp32 encoder alone is 2.4 GB.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from kraken.agent.config import CONFIG_DIR

MODELS_DIR = CONFIG_DIR / "models"


@dataclass(frozen=True)
class VoiceModel:
    key: str
    label: str
    repo: str
    # Repo-relative paths, in download order, with their approximate size in
    # bytes. The sizes only drive the progress bar, so they may drift a little.
    files: tuple[tuple[str, int], ...]

    @property
    def directory(self) -> Path:
        return MODELS_DIR / self.key

    @property
    def download_bytes(self) -> int:
        return sum(size for _name, size in self.files)

    @property
    def download_label(self) -> str:
        return f"{self.download_bytes / 1_000_000:.0f} MB"

    def is_installed(self) -> bool:
        return all((self.directory / name).exists() for name, _size in self.files)


PARAKEET = VoiceModel(
    key="parakeet-tdt-0.6b-v2",
    label="Parakeet TDT 0.6B v2",
    repo="istupakov/parakeet-tdt-0.6b-v2-onnx",
    files=(
        ("encoder-model.int8.onnx", 652_200_000),
        ("decoder_joint-model.int8.onnx", 9_000_000),
        ("vocab.txt", 10_000),
        # Tiny, but not optional: it tells onnx-asr this export wants 128 mel
        # bins. Without it the preprocessor builds 80 and the encoder rejects
        # the input outright.
        ("config.json", 100),
    ),
)


class Cancelled(Exception):
    """Raised out of download() when the caller asked it to stop."""


def download(
    model: VoiceModel,
    on_progress: Callable[[int, int], None] | None = None,
    should_cancel: Callable[[], bool] | None = None,
) -> None:
    """Fetch `model` into ~/.kraken/models/<key>/, skipping files already
    there. `on_progress` is called with (bytes done, bytes total) across the
    whole model, from this thread — never the GUI thread.

    `should_cancel` is polled as bytes arrive; when it goes true the download
    raises Cancelled. Partial files stay in the hub cache, so a later retry
    resumes rather than starting over."""
    from huggingface_hub import hf_hub_download
    from tqdm import tqdm as _tqdm

    total = model.download_bytes
    done = 0

    class _Reporter(_tqdm):
        """Turns hf_hub_download's progress bar into a callback. Sizes are
        summed across files so the bar tracks the whole model, and the
        declared size is only a starting estimate — once a file's real total
        is known the remaining budget is adjusted with it."""

        def __init__(self, *args, **kwargs):
            kwargs["disable"] = True
            super().__init__(*args, **kwargs)
            self._reported = 0

        def update(self, n=1):
            super().update(n)
            if should_cancel is not None and should_cancel():
                raise Cancelled
            if on_progress is None:
                return
            self._reported += n or 0
            on_progress(min(done + self._reported, total), total)

    model.directory.mkdir(parents=True, exist_ok=True)
    for name, size in model.files:
        if should_cancel is not None and should_cancel():
            raise Cancelled
        hf_hub_download(
            model.repo,
            name,
            local_dir=model.directory,
            tqdm_class=_Reporter,
        )
        done += size
        if on_progress is not None:
            on_progress(min(done, total), total)
