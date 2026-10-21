"""Signal primitives shared by the circuit breaker.

Two jobs live here:

1. **Error-signature bucketing.** Raw error strings are noisy, unbounded, and agent-controlled.
   Bucketing them into a small closed vocabulary is what makes "the same failure twice"
   detectable at all, and it keeps untrusted text out of anything that later feeds a model.
2. **Command-head extraction.** The forbidden-command check must match the binary being
   *invoked*, never a substring. ``uv run python app.py`` is legal under a ``uv`` contract;
   only a direct invocation of a forbidden binary is a violation.
"""

import re
from typing import Any, Dict, List, Optional, Tuple

#: Argument keys that are treated as a shell command when present.
COMMAND_KEYS: Tuple[str, ...] = (
    "command",
    "cmd",
    "command_line",
    "commandLine",
    "shell_command",
    "shellCommand",
    "script",
    "argv",
)

#: Error signature vocabulary. Deliberately coarse: the only distinction that really matters
#: for loop detection is "wrong binary" (a loop) versus "file missing" (normal progress).
ERROR_SIGNATURES: Tuple[str, ...] = (
    "none",
    "command_not_found",
    "permission_denied",
    "timeout",
    "rate_limited",
    "network",
    "import_error",
    "syntax_error",
    "not_found",
    "unknown",
)

# Ordered: the first match wins, so "command not found" must be tested before "not found".
_SIGNATURE_PATTERNS: Tuple[Tuple[str, str], ...] = (
    (
        "command_not_found",
        r"command not found|not recognized as an internal or external command"
        r"|is not recognized as an internal|executable file not found",
    ),
    ("permission_denied", r"permission denied|access is denied|unauthorized|forbidden"),
    ("timeout", r"timed out|timeout|deadline exceeded"),
    ("rate_limited", r"rate limit|too many requests|\b429\b"),
    (
        "network",
        r"connection refused|connection reset|could not resolve|getaddrinfo"
        r"|network is unreachable|network error|dns",
    ),
    (
        "import_error",
        r"modulenotfounderror|no module named|cannot find module|importerror"
        r"|cannot load module",
    ),
    (
        "syntax_error",
        r"syntaxerror|unexpected token|parse error|invalid syntax|indentationerror"
        r"|compileerror",
    ),
    ("not_found", r"no such file|not found|does not exist|could not find|cannot find"),
)

_SEGMENT_SPLIT = re.compile(r"&&|\|\||;|\||\n")
_ENV_ASSIGNMENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
_BINARY_NAME = re.compile(r"^[A-Za-z0-9._+-]+$")
_VERSION_SUFFIX = re.compile(r"[\d.]+$")


def classify_error(message: Optional[str], return_code: Optional[int]) -> str:
    """Bucket a failure into a stable signature from :data:`ERROR_SIGNATURES`.

    Returns ``"none"`` when the action did not fail.
    """
    if not message and (return_code is None or return_code == 0):
        return "none"

    if not message:
        return "unknown"

    lowered = message.lower()
    for signature, pattern in _SIGNATURE_PATTERNS:
        if re.search(pattern, lowered):
            return signature
    return "unknown"


def normalize_binary(name: str) -> str:
    """Normalize a binary name for comparison.

    Strips a trailing version suffix so ``python3.11`` matches a forbidden ``python`` while
    ``pythonic`` does not.
    """
    return _VERSION_SUFFIX.sub("", name.lower())


def _head_of_segment(segment: str) -> Optional[str]:
    """Return the leading binary of a single command segment, if there is one."""
    text = segment.strip()
    if not text:
        return None

    tokens = text.split()
    index = 0
    while index < len(tokens) and _ENV_ASSIGNMENT.match(tokens[index]):
        index += 1
    if index >= len(tokens):
        return None

    head = tokens[index].strip("'\"")
    if not head:
        return None

    # A leading path means the binary is being invoked directly (``/usr/bin/python``).
    # Only the basename identifies it.
    if "/" in head or "\\" in head:
        head = re.split(r"[/\\]", head)[-1]
        head = head.strip("'\"")

    if not head or not _BINARY_NAME.match(head):
        return None
    return head.lower()


def extract_command_heads(arguments: Dict[str, Any]) -> List[str]:
    """Extract the leading binary of every command segment present in ``arguments``.

    Known command keys are preferred. Failing that, string values that look like a command
    invocation (i.e. contain whitespace, so a bare path such as ``"src"`` is ignored) are
    considered.
    """
    candidates: List[str] = []

    for key in COMMAND_KEYS:
        value = arguments.get(key)
        if isinstance(value, str) and value.strip():
            candidates.append(value)

    if not candidates:
        for value in arguments.values():
            if (
                isinstance(value, str)
                and value.strip()
                and "\n" not in value
                and " " in value.strip()
                and len(value) < 200
            ):
                candidates.append(value)

    heads: List[str] = []
    for candidate in candidates:
        for segment in _SEGMENT_SPLIT.split(candidate):
            head = _head_of_segment(segment)
            if head:
                heads.append(head)
    return heads


def find_forbidden_command(
    arguments: Dict[str, Any], forbidden: List[str]
) -> Optional[str]:
    """Return the first forbidden binary invoked by ``arguments``, if any.

    ``forbidden`` holds command names as written in the environment contract. Both sides are
    normalized so ``python3.11`` matches a forbidden ``python``.
    """
    if not forbidden:
        return None

    normalized_forbidden = {normalize_binary(name) for name in forbidden if name}
    for head in extract_command_heads(arguments):
        if normalize_binary(head) in normalized_forbidden:
            return head
    return None
