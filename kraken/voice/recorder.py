"""Microphone capture through QtMultimedia.

QAudioSource hands back a QIODevice that streams raw PCM in whatever format
we ask for, so asking for exactly what the models want — 16 kHz, mono,
signed 16-bit — means no resampling, no encoder, and no temp files: the
buffer we accumulate is the model's input.
"""

from __future__ import annotations

from array import array

from PySide6.QtCore import QObject, Signal
from PySide6.QtMultimedia import QAudioFormat, QAudioSource, QMediaDevices

SAMPLE_RATE = 16000
_BYTES_PER_SAMPLE = 2


def has_input_device() -> bool:
    return not QMediaDevices.defaultAudioInput().isNull()


class Recorder(QObject):
    """One recording at a time. `level` is emitted as audio arrives so the
    caller can show that something is being heard."""

    level = Signal(float)  # peak amplitude of the last chunk, 0..1

    def __init__(self, parent: QObject | None = None):
        super().__init__(parent)
        self._source: QAudioSource | None = None
        self._buffer = bytearray()

    @property
    def is_recording(self) -> bool:
        return self._source is not None

    @property
    def seconds(self) -> float:
        return len(self._buffer) / (SAMPLE_RATE * _BYTES_PER_SAMPLE)

    def start(self) -> bool:
        """Begin capturing. False if there's no usable input device — the
        caller should say so rather than silently record silence."""
        if self.is_recording:
            return True
        device = QMediaDevices.defaultAudioInput()
        if device.isNull():
            return False

        fmt = QAudioFormat()
        fmt.setSampleRate(SAMPLE_RATE)
        fmt.setChannelCount(1)
        fmt.setSampleFormat(QAudioFormat.SampleFormat.Int16)
        if not device.isFormatSupported(fmt):
            fmt = device.preferredFormat()
            # Qt resamples into our format when the device can't produce it
            # natively; only the sample format has to be one we can read.
            fmt.setSampleFormat(QAudioFormat.SampleFormat.Int16)
            fmt.setChannelCount(1)
            fmt.setSampleRate(SAMPLE_RATE)

        self._buffer.clear()
        self._source = QAudioSource(device, fmt, self)
        stream = self._source.start()
        if stream is None:
            self._source = None
            return False
        stream.readyRead.connect(lambda: self._drain(stream))
        return True

    def _drain(self, stream) -> None:
        chunk = bytes(stream.readAll())
        if not chunk:
            return
        self._buffer.extend(chunk)
        # Decode as signed samples rather than eyeballing the high byte: in
        # two's complement a whisper-quiet negative sample has a high byte of
        # 0xFF, which would read as a full-scale peak and make the silence
        # check useless.
        samples = array("h")
        samples.frombytes(chunk[: len(chunk) // 2 * 2])
        if not samples:
            return
        peak = max(max(samples), -min(samples))
        self.level.emit(min(peak / 32768.0, 1.0))

    def stop(self) -> bytes:
        """Stop and return everything captured as raw PCM."""
        if self._source is not None:
            self._source.stop()
            self._source.deleteLater()
            self._source = None
        pcm = bytes(self._buffer)
        self._buffer.clear()
        return pcm

    def cancel(self) -> None:
        self.stop()
