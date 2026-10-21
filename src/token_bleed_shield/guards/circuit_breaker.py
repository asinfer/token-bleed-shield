"""Sliding-window drift and loop circuit breaker for autonomous agent loops.

The breaker works in two phases:

- :meth:`DriftCircuitBreaker.pre_check` runs **before** a tool executes. It predicts whether
  executing the action would create a violation and can therefore prevent the call outright.
  This is the phase that makes the tool preventive rather than forensic: blocking the second
  identical call saves a call that a post-hoc detector would only have reported afterwards.
- :meth:`DriftCircuitBreaker.record` runs **after** the tool returns. It updates streaks and
  evaluates failures, which are only observable once a call has happened.

:meth:`DriftCircuitBreaker.record_and_evaluate` remains as a compatibility wrapper for callers
that only want the original post-hoc behaviour.
"""

import hashlib
import json
from typing import Any, Dict, List, Optional

from pydantic import BaseModel, Field

from ..config import CircuitBreakerConfig, Enforcement
from ..contract import ContractSnapshot
from ..policy import CircuitBreakerPolicy, PolicyDecision, Signal
from .signals import classify_error, find_forbidden_command


class AgentAction(BaseModel):
    """A single tool invocation attempted by an agent."""

    tool_name: str
    arguments: Dict[str, Any] = Field(default_factory=dict)
    return_code: Optional[int] = 0
    error_message: Optional[str] = None

    @property
    def error_signature(self) -> str:
        """Bucketed failure signature for this action (``"none"`` when it succeeded)."""
        return classify_error(self.error_message, self.return_code)

    @property
    def failed(self) -> bool:
        """True when this action represents a failure."""
        if self.error_message:
            return True
        return self.return_code is not None and self.return_code != 0


class BreakerVerdict(BaseModel):
    """The outcome of evaluating an agent action."""

    tripped: bool
    """True when at least one detection rule fired."""

    severity: str  # "OK", "WARNING", "CRITICAL"
    reason: Optional[str] = None
    suggested_action: Optional[str] = None
    consecutive_repetitions: int = 0
    consecutive_errors: int = 0
    signals: List[str] = Field(default_factory=list)
    enforcement: Enforcement = Enforcement.WARN
    """The configured enforcement for the most severe signal that fired."""

    block: bool = False
    """True when the caller must prevent the action from executing."""

    escalate: bool = False
    """True when the decision should be handed to a human."""


class DriftCircuitBreaker:
    """Detects repetitive tool loops and model drift before tokens bleed exponentially."""

    def __init__(
        self,
        config: Optional[CircuitBreakerConfig] = None,
        contract: Optional[ContractSnapshot] = None,
        policy: Optional[CircuitBreakerPolicy] = None,
    ):
        self.config = config or CircuitBreakerConfig()
        self.contract = contract
        self.policy = policy or CircuitBreakerPolicy(self.config)
        self.history: List[AgentAction] = []

    # ------------------------------------------------------------------ helpers

    def _hash_args(self, args: Dict[str, Any]) -> str:
        serialized = json.dumps(args, sort_keys=True, default=str)
        return hashlib.sha256(serialized.encode("utf-8")).hexdigest()[:16]

    def _window(self) -> List[AgentAction]:
        return self.history[-self.config.sliding_window_size :]

    def _trailing_identical_streak(
        self, action: AgentAction, priors: List[AgentAction]
    ) -> int:
        """Number of consecutive identical ``(tool, arguments)`` calls, counting ``action``.

        ``priors`` must exclude ``action`` itself, otherwise the action matches itself and the
        streak is inflated by one.
        """
        streak = 1
        current_hash = self._hash_args(action.arguments)
        for prior in reversed(priors):
            if prior.tool_name == action.tool_name and self._hash_args(prior.arguments) == current_hash:
                streak += 1
            else:
                break
        return streak

    def _trailing_same_tool_streak(
        self, action: AgentAction, priors: List[AgentAction]
    ) -> int:
        """Number of consecutive calls to the same tool, counting ``action``."""
        streak = 1
        for prior in reversed(priors):
            if prior.tool_name == action.tool_name:
                streak += 1
            else:
                break
        return streak

    def _trailing_error_streak(self) -> int:
        """Number of consecutive failures at the end of the history."""
        streak = 0
        for prior in reversed(self.history):
            if prior.failed:
                streak += 1
            else:
                break
        return streak

    def _trailing_signature_streak(self, signature: str) -> int:
        """Number of consecutive failures sharing ``signature`` at the end of the history."""
        if signature == "none":
            return 0
        streak = 0
        for prior in reversed(self.history):
            if prior.error_signature == signature:
                streak += 1
            else:
                break
        return streak

    @staticmethod
    def _oscillation_pair(names: List[str]) -> Optional[tuple]:
        """Return the oscillating pair when the last four tool names form an A -> B -> A -> B."""
        if len(names) < 4:
            return None
        tail = names[-4:]
        if len(set(tail)) == 2 and tail[3] == tail[1] and tail[2] == tail[0]:
            return (tail[3], tail[2])
        return None

    def _verdict(self, decisions: List[PolicyDecision]) -> Optional[BreakerVerdict]:
        """Fold policy decisions into a single verdict, or ``None`` when nothing fired."""
        # Rules configured as `off` are suppressed entirely rather than reported and ignored.
        active = [d for d in decisions if d.enforcement is not Enforcement.OFF]
        if not active:
            return None

        order = {"CRITICAL": 0, "WARNING": 1, "OK": 2}
        ranked = sorted(active, key=lambda d: order.get(d.severity, 3))
        primary = ranked[0]

        # The reported enforcement is the most restrictive one that fired, so it always
        # agrees with the `block` and `escalate` flags below.
        strictness = {Enforcement.BLOCK: 0, Enforcement.ASK: 1, Enforcement.WARN: 2}
        strictest = min(active, key=lambda d: strictness.get(d.enforcement, 3)).enforcement

        return BreakerVerdict(
            tripped=True,
            severity=primary.severity,
            reason=" ".join(d.reason for d in ranked),
            suggested_action=primary.suggested_action,
            signals=[d.signal.value for d in ranked],
            enforcement=strictest,
            block=any(d.block for d in active),
            escalate=any(d.escalate for d in active),
        )

    # -------------------------------------------------------------- pre-check

    def pre_check(self, action: AgentAction) -> Optional[BreakerVerdict]:
        """Evaluate a proposed action *before* it executes.

        Returns a verdict when the action should be prevented, otherwise ``None``. Because the
        history is already known, this phase can predict the violation and spare the call
        entirely instead of reporting it afterwards.
        """
        decisions: List[PolicyDecision] = []
        priors = self._window()

        forbidden = self.contract.forbidden_commands if self.contract else []
        offending = find_forbidden_command(action.arguments, forbidden)
        if offending:
            prefix = self._command_prefix()
            decisions.append(
                self.policy.decide(
                    Signal.FORBIDDEN_COMMAND,
                    reason=(
                        f"Command `{offending}` is forbidden by the environment contract"
                        + (f"; the pinned toolchain requires `{prefix}`." if prefix else ".")
                    ),
                    suggested_action=(
                        "Do not execute. Use the pinned command prefix from the environment "
                        "contract instead of the forbidden binary."
                    ),
                )
            )

        identical_streak = self._trailing_identical_streak(action, priors)
        if identical_streak > self.config.max_identical_tool_args:
            decisions.append(
                self.policy.decide(
                    Signal.IDENTICAL_ARGS,
                    reason=(
                        f"Agent invoked `{action.tool_name}` with identical arguments "
                        f"{identical_streak} times in a row."
                    ),
                    suggested_action=(
                        "Do not execute. The previous call with these arguments already ran; "
                        "either the result was ignored or the arguments need to change."
                    ),
                )
            )

        same_tool_streak = self._trailing_same_tool_streak(action, priors)
        if same_tool_streak > self.config.max_consecutive_same_tools:
            decisions.append(
                self.policy.decide(
                    Signal.SAME_TOOL_STREAK,
                    reason=(
                        f"Agent invoked `{action.tool_name}` {same_tool_streak} times in a row "
                        "with differing arguments (reconnaissance sweep)."
                    ),
                    suggested_action=(
                        "Do not execute. Repeatedly sweeping the same tool without changing "
                        "strategy is reconnaissance, not progress."
                    ),
                )
            )

        names = [a.tool_name for a in self._window()] + [action.tool_name]
        pair = self._oscillation_pair(names)
        if pair:
            decisions.append(
                self.policy.decide(
                    Signal.OSCILLATION,
                    reason=(
                        f"Detected cyclic oscillation pattern between `{pair[0]}` and `{pair[1]}`."
                    ),
                    suggested_action=(
                        "Do not execute. Break the cycle and require explicit user direction "
                        "before continuing."
                    ),
                )
            )

        return self._verdict(decisions)

    # ----------------------------------------------------------------- record

    def record(self, action: AgentAction) -> BreakerVerdict:
        """Record the outcome of an executed action and evaluate the resulting state."""
        self.history.append(action)
        decisions: List[PolicyDecision] = []
        # The action is already in the history, so priors must exclude it to avoid the
        # action matching itself.
        priors = self.history[:-1][-self.config.sliding_window_size :]

        identical_streak = self._trailing_identical_streak(action, priors)
        if identical_streak > self.config.max_identical_tool_args:
            decisions.append(
                self.policy.decide(
                    Signal.IDENTICAL_ARGS,
                    reason=(
                        f"Agent invoked `{action.tool_name}` with identical arguments "
                        f"{identical_streak} times in a row."
                    ),
                    suggested_action=(
                        "Stop automated retries. The previous call with these arguments already "
                        "ran; either the result was ignored or the arguments need to change."
                    ),
                )
            )

        same_tool_streak = self._trailing_same_tool_streak(action, priors)
        if same_tool_streak > self.config.max_consecutive_same_tools:
            decisions.append(
                self.policy.decide(
                    Signal.SAME_TOOL_STREAK,
                    reason=(
                        f"Agent invoked `{action.tool_name}` {same_tool_streak} times in a row "
                        "with differing arguments (reconnaissance sweep)."
                    ),
                    suggested_action=(
                        "Stop automated retries. Repeatedly sweeping the same tool without "
                        "changing strategy is reconnaissance, not progress."
                    ),
                )
            )

        error_streak = self._trailing_error_streak()
        if error_streak >= self.config.max_consecutive_errors:
            decisions.append(
                self.policy.decide(
                    Signal.ERROR_CASCADE,
                    reason=(
                        f"Agent encountered {error_streak} consecutive command/tool execution "
                        "failures."
                    ),
                    suggested_action=(
                        "Stop automated retries. Check environment configuration or the "
                        "pre-flight contract (.agent-env.md)."
                    ),
                )
            )

        signature_streak = self._trailing_signature_streak(action.error_signature)
        if signature_streak >= self.config.max_repeated_error_signature:
            decisions.append(
                self.policy.decide(
                    Signal.REPEATED_ERROR_SIGNATURE,
                    reason=(
                        f"Agent repeated the same failure signature "
                        f"(`{action.error_signature}`) {signature_streak} times consecutively."
                    ),
                    suggested_action=(
                        "Stop automated retries. The same failure is repeating, so retrying "
                        "unchanged will fail again."
                    ),
                )
            )

        pair = self._oscillation_pair([a.tool_name for a in self._window()])
        if pair:
            decisions.append(
                self.policy.decide(
                    Signal.OSCILLATION,
                    reason=(
                        f"Detected cyclic oscillation pattern between `{pair[0]}` and `{pair[1]}`."
                    ),
                    suggested_action=(
                        "Break the cycle. Require explicit user direction before continuing."
                    ),
                )
            )

        verdict = self._verdict(decisions)
        if verdict is None:
            return BreakerVerdict(
                tripped=False,
                severity="OK",
                consecutive_repetitions=identical_streak,
                consecutive_errors=error_streak,
            )

        verdict.consecutive_repetitions = identical_streak
        verdict.consecutive_errors = error_streak
        return verdict

    def record_and_evaluate(self, action: AgentAction) -> BreakerVerdict:
        """Record an executed action and evaluate it. Compatibility wrapper for the old API."""
        return self.record(action)

    def _command_prefix(self) -> Optional[str]:
        """The pinned command prefix from the contract, if one is known."""
        if not self.contract:
            return None
        for toolchain in self.contract.toolchains:
            if toolchain.command_prefix:
                return toolchain.command_prefix
        return None

    def reset(self) -> None:
        self.history.clear()
