from token_bleed_shield.config import CircuitBreakerConfig, Enforcement, RuleConfig
from token_bleed_shield.guards.circuit_breaker import AgentAction, DriftCircuitBreaker


def test_circuit_breaker_identical_repetition():
    breaker = DriftCircuitBreaker()

    action1 = AgentAction(tool_name="search", arguments={"path": "src", "pattern": "py"})
    verdict1 = breaker.record_and_evaluate(action1)
    assert verdict1.tripped is False

    # Second identical action trips breaker
    action2 = AgentAction(tool_name="search", arguments={"path": "src", "pattern": "py"})
    verdict2 = breaker.record_and_evaluate(action2)
    assert verdict2.tripped is True
    assert verdict2.reason and "identical arguments" in verdict2.reason


def test_circuit_breaker_error_cascade():
    breaker = DriftCircuitBreaker()

    # Distinct failure signatures mean the agent is making progress, not looping.
    for index, message in enumerate(("a: command not found", "b: permission denied")):
        verdict = breaker.record_and_evaluate(
            AgentAction(
                tool_name="shell",
                arguments={"cmd": f"fail_{index}"},
                return_code=1,
                error_message=message,
            )
        )
        assert verdict.tripped is False

    # A third consecutive failure trips the cascade.
    verdict_fail = breaker.record_and_evaluate(
        AgentAction(
            tool_name="shell",
            arguments={"cmd": "fail_final"},
            return_code=1,
            error_message="c: connection refused",
        )
    )
    assert verdict_fail.tripped is True
    assert verdict_fail.reason and "consecutive command/tool execution failures" in verdict_fail.reason


def test_circuit_breaker_repeated_error_signature():
    breaker = DriftCircuitBreaker()

    verdict = breaker.record_and_evaluate(
        AgentAction(
            tool_name="shell",
            arguments={"cmd": "python -m unittest"},
            return_code=1,
            error_message="python: command not found",
        )
    )
    assert verdict.tripped is False

    # The exact same failure again is a loop, and it is caught one step earlier
    # than the generic cascade.
    verdict_repeat = breaker.record_and_evaluate(
        AgentAction(
            tool_name="shell",
            arguments={"cmd": "python -m pytest"},
            return_code=1,
            error_message="python: command not found",
        )
    )
    assert verdict_repeat.tripped is True
    assert "repeated_error_signature" in verdict_repeat.signals


def test_circuit_breaker_blocks_an_identical_repeat_by_default():
    breaker = DriftCircuitBreaker()

    breaker.record_and_evaluate(
        AgentAction(tool_name="search", arguments={"path": "src", "pattern": "py"})
    )
    verdict = breaker.record_and_evaluate(
        AgentAction(tool_name="search", arguments={"path": "src", "pattern": "py"})
    )

    assert verdict.tripped is True
    assert verdict.block is True
    assert verdict.enforcement is Enforcement.BLOCK


def test_circuit_breaker_reports_without_blocking_when_configured_to_warn():
    config = CircuitBreakerConfig(
        rules={"identical_args": RuleConfig(enforcement=Enforcement.WARN)}
    )
    breaker = DriftCircuitBreaker(config)

    breaker.record_and_evaluate(
        AgentAction(tool_name="search", arguments={"path": "src", "pattern": "py"})
    )
    verdict = breaker.record_and_evaluate(
        AgentAction(tool_name="search", arguments={"path": "src", "pattern": "py"})
    )

    assert verdict.tripped is True
    assert verdict.block is False
    assert verdict.enforcement is Enforcement.WARN
