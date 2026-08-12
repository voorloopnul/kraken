"""Speech-to-text, loaded lazily and kept warm between recordings.

onnx-asr owns the mel front-end and the transducer decode loop for the NeMo
export, so there is nothing to do here but hand it samples. Loading costs a
second or two while onnxruntime maps the weights and builds its graph, so the
loaded model is cached: the first dictation of a session pays for it, later
ones don't.

Everything in here runs on a worker thread — no Qt objects are touched.
"""

from __future__ import annotations

from kraken.voice.models import PARAKEET

SAMPLE_RATE = 16000

_model = None


def _load():
    global _model
    if _model is None:
        import onnx_asr

        _model = onnx_asr.load_model(
            "nemo-parakeet-tdt-0.6b-v2",
            path=str(PARAKEET.directory),
            quantization="int8",
            # CPU only, which on this machine is the fast path. Left to choose,
            # onnxruntime puts CoreML first on a Mac, and CoreML cannot take an
            # int8 graph whole: it claimed 1528 of the encoder's 3249 nodes and
            # split what was left into 319 partitions, each boundary a copy
            # back and forth. Measured on an M-series laptop, that made loading
            # 3.02s against 0.64s and a six-second clip 0.29s against 0.14s —
            # the accelerator costing twice the time it saved. It is also where
            # every CoreML warning and "Context leak detected" line in the
            # app's output came from; without it the log is quiet.
            #
            # Worth re-measuring if the model stops being quantized, or if the
            # dependency ever moves off onnxruntime's CPU build (pyproject asks
            # for onnx-asr[cpu]) — this list would then also exclude a GPU.
            providers=["CPUExecutionProvider"],
        )
    return _model


def transcribe(pcm: bytes) -> str:
    """Transcribe 16 kHz mono signed-16-bit PCM. Returns "" for a recording
    with nothing in it, so a stray click can't insert junk."""
    import numpy as np

    samples = np.frombuffer(pcm, np.int16).astype(np.float32) / 32768.0
    if samples.size < SAMPLE_RATE // 4:  # under 0.25s: a mis-click, not speech
        return ""
    return str(_load().recognize(samples, sample_rate=SAMPLE_RATE)).strip()
