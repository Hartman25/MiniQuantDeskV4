from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Union

BytesLike = Union[bytes, bytearray, memoryview]


def sha256_bytes(data: BytesLike) -> str:
    h = hashlib.sha256()
    h.update(data)
    return h.hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()