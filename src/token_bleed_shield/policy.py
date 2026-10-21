"""Policy layer: detection signals, enforcement levels, and how they resolve to actions.

Separation of concerns:

- :mod:`token_bleed_shield.guards.circuit_breaker` *detects* (what happened).
- :mod:`token_bleed_shield.policy` *decides* (what should be done about it).

Keeping the two apart is what makes enforcement a preference rather than a hardcoded behaviour:
the same detection can warn in development and block in production purely by changing
``.tokenbleedshield.yaml``.
"""

from typing import Optional

from pydantic import BaseModel

from .config import CircuitBreakerConfig, Enforcement, Signal

__all__ = [
    "DEFAULT_SUGGESTED_ACTION",
    "SEVERITY_BY_SIGNAL",
    "CircuitBreakerPolicy",
    "PolicyDecision",
    "Signal",
]

SEVERITY_BY_SIGNAL = {
    Signal.FORBIDDEN_COMMAND: "CRITICAL",
    Signal.IDENTICAL_ARGS: "CRITICAL",
    Signal.REPEATED_ERROR_SIGNATURE: "CRITICAL",
    Signal.SAME_TOOL_STREAK: "WARNING",
    Signal.ERROR_CASCADE: "WARNING",
    Signal.OSCILLATION: "WARNING",
}

DEFAULT_SUGGESTED_ACTION = (
    "Stop automated retries. Check environment configuration or the pre-flight contract "
    "(.agent-env.md) before continuing."
)


class PolicyDecision(BaseModel):
    """The resolved outcome for a single detection."""

    signal: Signal
    enforcement: Enforcement
    block: bool
    """True when the caller must prevent the action from executing."""

    escalate: bool
    """True when the decision should be handed to a human."""

    severity: str
    reason: str
    suggested_action: str


class CircuitBreakerPolicy:
    """Resolves a detection into an enforcement decision using configuration."""

    def __init__(self, config: Optional[CircuitBreakerConfig] = None):
        self.config = config or CircuitBreakerConfig()

    def enforcement_for(self, signal: Signal) -> Enforcement:
        """Return the configured enforcement for ``signal``.

        A per-rule entry in ``circuit_breaker.rules`` wins; otherwise the global
        ``default_enforcement`` applies.
        """
        rule = self.config.rules.get(signal.value)
        if rule is not None:
            return rule.enforcement
        return self.config.default_enforcement

    def decide(
        self,
        signal: Signal,
        reason: str,
        suggested_action: str = DEFAULT_SUGGESTED_ACTION,
    ) -> PolicyDecision:
        """Resolve ``signal`` into a :class:`PolicyDecision`."""
        enforcement = self.enforcement_for(signal)
        return PolicyDecision(
            signal=signal,
            enforcement=enforcement,
            block=enforcement is Enforcement.BLOCK,
            escalate=enforcement is Enforcement.ASK,
            severity=SEVERITY_BY_SIGNAL.get(signal, "WARNING"),
            reason=reason,
            suggested_action=suggested_action,
        )
