"""Voice input: record from the microphone and transcribe it locally.

Nothing here talks to the network at transcription time — models are ONNX
files under ~/.kraken/models, downloaded once from Hugging Face and then run
on the CPU through onnxruntime.
"""

import os

# Hugging Face's Xet transfer backend reports a file's bytes to the progress
# bar in a single lump at the end, which leaves the download dialog frozen at
# 0% for minutes on the 652 MB Parakeet weights. The plain HTTP path reports
# every 10 MB (and measured no slower on these one-off, undeduplicated
# downloads), so prefer it unless the user has said otherwise. Read at import
# time by huggingface_hub, hence set before anything imports it.
os.environ.setdefault("HF_HUB_DISABLE_XET", "1")
