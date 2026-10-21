"""Configuration models for Token Bleed Shield."""

from enum import Enum
from pathlib import Path
from typing import Dict, Optional

import yaml
from pydantic import BaseModel, Field


class Enforcement(str, Enum):
    """How a caller should react when a detection rule fires.

    This is deliberately a preference expressed in configuration rather than a hardcoded
    behaviour: the same rule can warn during development and block in production.
    """

    BLOCK = "block"
    """Stop the action before it executes."""

    WARN = "warn"
    """Report the detection but let the action proceed."""

    ASK = "ask"
    """Escalate to a human for an explicit decision."""

    OFF = "off"
    """Ignore this rule entirely."""


class Signal(str, Enum):
    """Stable identifiers for every detection the circuit breaker can raise.

    These strings are part of the public contract. They are used as configuration keys in
    ``.tokenbleedshield.yaml`` and are intended to become the signal vocabulary in the judge
    digest (Phase 2). Renaming one is a breaking change for both.
    """

    FORBIDDEN_COMMAND = "forbidden_command"
    """A command head that the environment contract explicitly forbids."""

    IDENTICAL_ARGS = "identical_args"
    """The same tool invoked with byte-identical arguments as the previous call."""

    SAME_TOOL_STREAK = "same_tool_streak"
    """The same tool invoked repeatedly with differing arguments (a reconnaissance sweep)."""

    REPEATED_ERROR_SIGNATURE = "repeated_error_signature"
    """The same bucketed failure signature repeated consecutively."""

    ERROR_CASCADE = "error_cascade"
    """Consecutive failures of any signature."""

    OSCILLATION = "oscillation"
    """A cyclic A -> B -> A -> B tool pattern."""


class ProberConfig(BaseModel):
    """Configuration for host environment probing."""

    prefer_uv: bool = True
    prefer_pnpm: bool = True
    include_git_info: bool = True
    include_os_details: bool = True
    contract_filename: str = ".agent-env.md"


class ToolLinterConfig(BaseModel):
    """Configuration for tool schema minification."""

    strip_titles: bool = True
    strip_examples: bool = False
    max_description_length: int = 140
    prune_empty_fields: bool = True


class BudgetGateConfig(BaseModel):
    """Configuration for pre-flight context budget checks."""

    max_context_tokens: int = 32000
    warn_threshold_tokens: int = 24000
    char_per_token_ratio: float = 3.8
    reject_oversized: bool = False


class RuleConfig(BaseModel):
    """Per-rule enforcement override."""

    enforcement: Enforcement = Enforcement.WARN


def recommended_rules() -> Dict[str, RuleConfig]:
    """The recommended enforcement for each rule.

    Invariant violations are blocked outright: a command the contract forbids, or a call
    repeating byte-identical arguments, is provably wasted. The remaining signals are
    heuristic, so they warn until they have been validated against a real workload.
    """
    return {
        Signal.FORBIDDEN_COMMAND.value: RuleConfig(enforcement=Enforcement.BLOCK),
        Signal.IDENTICAL_ARGS.value: RuleConfig(enforcement=Enforcement.BLOCK),
        Signal.REPEATED_ERROR_SIGNATURE.value: RuleConfig(enforcement=Enforcement.WARN),
        Signal.ERROR_CASCADE.value: RuleConfig(enforcement=Enforcement.WARN),
        Signal.SAME_TOOL_STREAK.value: RuleConfig(enforcement=Enforcement.WARN),
        Signal.OSCILLATION.value: RuleConfig(enforcement=Enforcement.WARN),
    }


class CircuitBreakerConfig(BaseModel):
    """Configuration for early drift and loop termination.

    Thresholds are *counts of tolerated repeats*:

    - ``max_identical_tool_args``: consecutive identical ``(tool, arguments)`` repeats that are
      allowed before the ``identical_args`` signal fires. ``1`` means the signal fires on the
      second identical call.
    - ``max_consecutive_same_tools``: consecutive calls to the same tool (arguments may differ)
      that are allowed before the ``same_tool_streak`` signal fires.
    - ``max_consecutive_errors``: consecutive failures of any kind tolerated before the
      ``error_cascade`` signal fires.
    - ``max_repeated_error_signature``: consecutive failures sharing the same bucketed
      signature tolerated before the ``repeated_error_signature`` signal fires.
    """

    max_consecutive_same_tools: int = 2
    max_identical_tool_args: int = 1
    max_consecutive_errors: int = 3
    max_repeated_error_signature: int = 2
    sliding_window_size: int = 6
    default_enforcement: Enforcement = Enforcement.WARN
    rules: Dict[str, RuleConfig] = Field(default_factory=recommended_rules)


class ShieldConfig(BaseModel):
    """Root configuration for Token Bleed Shield."""

    prober: ProberConfig = Field(default_factory=ProberConfig)
    tool_linter: ToolLinterConfig = Field(default_factory=ToolLinterConfig)
    budget_gate: BudgetGateConfig = Field(default_factory=BudgetGateConfig)
    circuit_breaker: CircuitBreakerConfig = Field(default_factory=CircuitBreakerConfig)

    @classmethod
    def load(cls, config_path: Optional[Path] = None) -> "ShieldConfig":
        """Load configuration from disk, falling back to defaults.

        Resolution order: explicit ``config_path`` -> ``.tokenbleedshield.yaml`` in the current
        working directory -> built-in defaults.
        """
        candidate = config_path
        if candidate is None:
            candidate = Path(".tokenbleedshield.yaml")

        if candidate.is_file():
            with open(candidate, "r", encoding="utf-8") as handle:
                data = yaml.safe_load(handle) or {}
            return cls(**data)

        return cls()

    def save(self, config_path: Path = Path(".tokenbleedshield.yaml")) -> None:
        # ``mode="json"`` renders enums as their plain string values, which keeps the YAML
        # readable and safely loadable instead of emitting python/object tags.
        payload = self.model_dump(mode="json")
        with open(config_path, "w", encoding="utf-8") as handle:
            yaml.dump(payload, handle, default_flow_style=False, sort_keys=False)
