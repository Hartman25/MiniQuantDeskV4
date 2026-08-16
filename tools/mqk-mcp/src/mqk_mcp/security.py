from __future__ import annotations

from pathlib import Path

_ALLOWED_SECRET_EXCEPTIONS = {".env.local.example"}
_DENIED_EXACT = {
    ".env",
    ".env.local",
    "secrets.env",
    "secrets.local",
    "id_rsa",
    "id_ed25519",
}
_DENIED_PARTS = {".git", ".ssh"}
_DENIED_SUFFIXES = {".pem", ".key", ".p12", ".pfx"}


class AccessDenied(ValueError):
    pass


def _deny_secret_path(path: Path) -> None:
    lowered_parts = {part.lower() for part in path.parts}
    if lowered_parts & _DENIED_PARTS:
        raise AccessDenied("access to repository metadata or SSH material is denied")

    name = path.name.lower()
    if name in _ALLOWED_SECRET_EXCEPTIONS:
        return
    if name in _DENIED_EXACT or (name.startswith(".env.") and name not in _ALLOWED_SECRET_EXCEPTIONS):
        raise AccessDenied("access to environment/secret files is denied")
    if path.suffix.lower() in _DENIED_SUFFIXES:
        raise AccessDenied("access to credential/key files is denied")


def resolve_repo_path(root: Path, raw_path: str, *, must_exist: bool = True) -> Path:
    if not isinstance(raw_path, str) or not raw_path.strip():
        raise AccessDenied("path must be a non-empty repository-relative string")
    if "\x00" in raw_path:
        raise AccessDenied("NUL bytes are not allowed in paths")

    candidate = Path(raw_path)
    if candidate.is_absolute():
        raise AccessDenied("absolute paths are not allowed")

    root = root.resolve()
    combined = root / candidate
    try:
        resolved = combined.resolve(strict=must_exist)
    except FileNotFoundError:
        raise

    try:
        resolved.relative_to(root)
    except ValueError as exc:
        raise AccessDenied("path escapes the configured repository root") from exc

    rel = resolved.relative_to(root)
    _deny_secret_path(rel)
    return resolved


def ensure_text_file(path: Path, *, max_bytes: int) -> None:
    if not path.is_file():
        raise AccessDenied("requested path is not a regular file")
    size = path.stat().st_size
    if size > max_bytes:
        raise AccessDenied(f"file exceeds read limit ({size} > {max_bytes} bytes)")
    sample = path.read_bytes()[:8192]
    if b"\x00" in sample:
        raise AccessDenied("binary files are not readable through this MCP")
