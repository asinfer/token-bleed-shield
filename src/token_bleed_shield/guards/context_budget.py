"""Pre-flight context budget gating to prevent token overflow and runaway charges."""

import re
from typing import List, Optional

from pydantic import BaseModel, Field

from ..config import BudgetGateConfig


class BloatWarning(BaseModel):
    category: str
    description: str
    estimated_tokens: int
    recommendation: str


class BudgetAssessment(BaseModel):
    total_characters: int
    estimated_tokens: int
    max_tokens_allowed: int
    utilization_percent: float
    is_safe: bool
    should_reject: bool
    warnings: List[BloatWarning] = Field(default_factory=list)


class ContextBudgetGate:
    """Pre-evaluates outgoing prompt and context payloads before API dispatch."""

    def __init__(self, config: Optional[BudgetGateConfig] = None):
        # A default argument is evaluated once at import time, so a mutable default would be
        # shared by every instance in the process and mutated by whoever configures it.
        self.config = config or BudgetGateConfig()

    def estimate_tokens(self, text: str) -> int:
        return max(1, int(len(text) / self.config.char_per_token_ratio))

    def _detect_stacktrace_bloat(self, text: str) -> Optional[BloatWarning]:
        # Count tracebacks
        matches = list(re.finditer(r"(Traceback \(most recent call last\):[\s\S]+?(?=\n\S|\Z))", text))
        if len(matches) > 2:
            raw_len = sum(len(m.group(0)) for m in matches)
            tok = self.estimate_tokens("a" * raw_len)
            return BloatWarning(
                category="Repeated Stacktraces",
                description=f"Detected {len(matches)} stacktrace dumps in context.",
                estimated_tokens=tok,
                recommendation="Prune intermediate stack traces; keep only the latest exception summary.",
            )
        return None

    def _detect_base64_or_hex_bloat(self, text: str) -> Optional[BloatWarning]:
        # Detect long base64/hex blobs (>= 256 chars contiguous)
        matches = re.findall(r"[A-Za-z0-9+/=]{256,}", text)
        if matches:
            blob_chars = sum(len(m) for m in matches)
            tok = self.estimate_tokens("a" * blob_chars)
            return BloatWarning(
                category="Binary/Base64 Bloat",
                description=f"Found {len(matches)} large encoded data blobs (~{blob_chars} chars).",
                estimated_tokens=tok,
                recommendation="Store binary assets as file references instead of inlining into context.",
            )
        return None

    def _detect_massive_log_dumps(self, text: str) -> Optional[BloatWarning]:
        lines = text.splitlines()
        log_patterns = sum(1 for line in lines if re.match(r"^\s*(\[?\d{4}-\d{2}-\d{2}|INFO|DEBUG|WARN|ERROR)", line))
        if log_patterns > 100:
            tok = self.estimate_tokens("\n".join(lines[:log_patterns]))
            return BloatWarning(
                category="Unfiltered Log Dump",
                description=f"Contains {log_patterns} lines of raw system logs.",
                estimated_tokens=tok,
                recommendation="Filter logs to only error lines or summarize log patterns.",
            )
        return None

    def evaluate(self, content: str) -> BudgetAssessment:
        char_count = len(content)
        tokens = self.estimate_tokens(content)
        max_allowed = self.config.max_context_tokens
        utilization = round((tokens / max_allowed) * 100, 1)

        warnings: List[BloatWarning] = []

        # Check for specific bloat patterns
        st_warn = self._detect_stacktrace_bloat(content)
        if st_warn:
            warnings.append(st_warn)

        b64_warn = self._detect_base64_or_hex_bloat(content)
        if b64_warn:
            warnings.append(b64_warn)

        log_warn = self._detect_massive_log_dumps(content)
        if log_warn:
            warnings.append(log_warn)

        is_safe = tokens <= self.config.warn_threshold_tokens
        should_reject = (tokens > max_allowed) and self.config.reject_oversized

        if tokens > self.config.warn_threshold_tokens:
            warnings.append(
                BloatWarning(
                    category="Context Window Near Capacity",
                    description=f"Context ({tokens} tokens) exceeds warning threshold ({self.config.warn_threshold_tokens} tokens).",
                    estimated_tokens=tokens,
                    recommendation="Offload non-critical historical turns to disk and retain only active goals.",
                )
            )

        return BudgetAssessment(
            total_characters=char_count,
            estimated_tokens=tokens,
            max_tokens_allowed=max_allowed,
            utilization_percent=utilization,
            is_safe=is_safe,
            should_reject=should_reject,
            warnings=warnings,
        )
