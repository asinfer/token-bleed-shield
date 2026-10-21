from token_bleed_shield.config import CircuitBreakerConfig, Enforcement, RuleConfig
from token_bleed_shield.contract import ContractSnapshot, ToolchainContract
from token_bleed_shield.guards.circuit_breaker import AgentAction, DriftCircuitBreaker


def _contract() -> ContractSnapshot:
    return ContractSnapshot(
        contract_version=1,
        os_name="Linux",
        workspace_root="/workspace",
        python=ToolchainContract(
            name="uv",
            version="uv 0.12.15",
            command_prefix="uv run",
            forbidden=["python", "python3", "pip", "pipenv"],
        ),
    )


def _config(**kwargs) -> CircuitBreakerConfig:
    return CircuitBreakerConfig(**kwargs)


def _blocking_config() -> CircuitBreakerConfig:
    return _config(
        rules={
            "forbidden_command": RuleConfig(enforcement=Enforcement.BLOCK),
            "identical_args": RuleConfig(enforcement=Enforcement.BLOCK),
        }
    )


def _shell(command: str) -> AgentAction:
    return AgentAction(tool_name="run_shell_command", arguments={"command": command})


# --------------------------------------------------------------- forbidden commands


def test_forbidden_command_is_blocked_before_execution():
    breaker = DriftCircuitBreaker(_blocking_config(), contract=_contract())

    verdict = breaker.pre_check(_shell("python -m unittest"))

    assert verdict is not None
    assert verdict.tripped is True
    assert verdict.block is True
    assert "forbidden_command" in verdict.signals
    assert verdict.reason and "python" in verdict.reason


def test_pinned_prefix_is_not_blocked():
    breaker = DriftCircuitBreaker(_blocking_config(), contract=_contract())

    assert breaker.pre_check(_shell("uv run python app.py")) is None


def test_forbidden_command_is_not_detected_without_a_contract():
    breaker = DriftCircuitBreaker(_blocking_config())

    assert breaker.pre_check(_shell("python -m unittest")) is None


def test_forbidden_command_rule_can_be_switched_off():
    config = _config(
        rules={"forbidden_command": RuleConfig(enforcement=Enforcement.OFF)},
    )
    breaker = DriftCircuitBreaker(config, contract=_contract())

    assert breaker.pre_check(_shell("python -m unittest")) is None


def test_warn_enforcement_reports_without_blocking():
    config = _config(
        rules={"forbidden_command": RuleConfig(enforcement=Enforcement.WARN)},
    )
    breaker = DriftCircuitBreaker(config, contract=_contract())

    verdict = breaker.pre_check(_shell("python -m unittest"))

    assert verdict is not None
    assert verdict.tripped is True
    assert verdict.block is False
    assert verdict.enforcement is Enforcement.WARN


def test_forbidden_command_blocks_by_default():
    # The recommended defaults treat a contract violation as a provable waste.
    breaker = DriftCircuitBreaker(CircuitBreakerConfig(), contract=_contract())

    verdict = breaker.pre_check(_shell("python -m unittest"))

    assert verdict is not None
    assert verdict.block is True
    assert verdict.enforcement is Enforcement.BLOCK


def test_reported_enforcement_reflects_the_most_restrictive_rule():
    config = _config(
        default_enforcement=Enforcement.WARN,
        rules={"forbidden_command": RuleConfig(enforcement=Enforcement.BLOCK)},
    )
    breaker = DriftCircuitBreaker(config, contract=_contract())

    # A forbidden command and a reconnaissance sweep fire together; the stricter
    # enforcement must be the one reported.
    breaker.record(AgentAction(tool_name="search_files", arguments={"path": "src"}))
    breaker.record(AgentAction(tool_name="search_files", arguments={"path": "lib"}))

    verdict = breaker.pre_check(
        AgentAction(
            tool_name="search_files",
            arguments={"path": "tests", "command": "python -m pytest"},
        )
    )

    assert verdict is not None
    assert verdict.enforcement is Enforcement.BLOCK
    assert verdict.block is True


# ------------------------------------------------------------------- identical args


def test_second_identical_call_is_blocked_before_execution():
    breaker = DriftCircuitBreaker(_blocking_config(), contract=_contract())

    action = _shell("uv run pytest")
    breaker.record(action)

    verdict = breaker.pre_check(action)

    assert verdict is not None
    assert verdict.block is True
    assert "identical_args" in verdict.signals
    assert verdict.reason and "identical arguments" in verdict.reason


def test_identical_args_are_not_blocked_when_the_arguments_differ():
    breaker = DriftCircuitBreaker(_blocking_config(), contract=_contract())

    breaker.record(_shell("uv run pytest"))

    assert breaker.pre_check(_shell("uv run pytest tests/")) is None


# --------------------------------------------------------------- reconnaissance sweep


def test_reconnaissance_sweep_is_blocked_before_the_third_call():
    breaker = DriftCircuitBreaker(_config(), contract=_contract())

    breaker.record(AgentAction(tool_name="search_files", arguments={"path": "src"}))
    breaker.record(AgentAction(tool_name="search_files", arguments={"path": "lib"}))

    verdict = breaker.pre_check(AgentAction(tool_name="search_files", arguments={"path": "tests"}))

    assert verdict is not None
    assert "same_tool_streak" in verdict.signals


def test_two_calls_to_the_same_tool_are_not_a_sweep():
    breaker = DriftCircuitBreaker(_config(), contract=_contract())

    breaker.record(AgentAction(tool_name="search_files", arguments={"path": "src"}))

    assert breaker.pre_check(AgentAction(tool_name="search_files", arguments={"path": "lib"})) is None


# ---------------------------------------------------------------------- oscillation


def test_oscillation_is_blocked_before_the_fourth_call():
    breaker = DriftCircuitBreaker(_config(), contract=_contract())

    breaker.record(AgentAction(tool_name="search_files", arguments={"path": "a"}))
    breaker.record(AgentAction(tool_name="read_file", arguments={"path": "b"}))
    breaker.record(AgentAction(tool_name="search_files", arguments={"path": "c"}))

    verdict = breaker.pre_check(AgentAction(tool_name="read_file", arguments={"path": "d"}))

    assert verdict is not None
    assert "oscillation" in verdict.signals
    assert verdict.reason and "oscillation pattern" in verdict.reason


# ------------------------------------------------------------------------- record


def test_repeated_error_signature_trips_on_the_second_failure():
    breaker = DriftCircuitBreaker(_config(), contract=_contract())

    first = AgentAction(
        tool_name="run_shell_command",
        arguments={"command": "python -m unittest"},
        return_code=1,
        error_message="python: command not found",
    )
    breaker.record(first)

    second = AgentAction(
        tool_name="run_shell_command",
        arguments={"command": "python -m pytest"},
        return_code=1,
        error_message="python: command not found",
    )
    verdict = breaker.record(second)

    assert verdict.tripped is True
    assert "repeated_error_signature" in verdict.signals
    assert verdict.reason and "command_not_found" in verdict.reason


def test_distinct_failures_do_not_trip_the_cascade():
    breaker = DriftCircuitBreaker(_config(), contract=_contract())

    breaker.record(
        AgentAction(
            tool_name="run_shell_command",
            arguments={"command": "a"},
            return_code=1,
            error_message="a: command not found",
        )
    )

    verdict = breaker.record(
        AgentAction(
            tool_name="run_shell_command",
            arguments={"command": "b"},
            return_code=1,
            error_message="b: permission denied",
        )
    )

    assert verdict.tripped is False
    assert verdict.severity == "OK"


def test_error_cascade_trips_on_the_third_failure():
    breaker = DriftCircuitBreaker(_config(), contract=_contract())

    for command, message in (
        ("a", "a: command not found"),
        ("b", "b: permission denied"),
        ("c", "c: connection refused"),
    ):
        verdict = breaker.record(
            AgentAction(
                tool_name="run_shell_command",
                arguments={"command": command},
                return_code=1,
                error_message=message,
            )
        )

    assert verdict.tripped is True
    assert "error_cascade" in verdict.signals
    assert verdict.reason and "consecutive command/tool execution failures" in verdict.reason


def test_a_success_clears_the_error_streak():
    breaker = DriftCircuitBreaker(_config(), contract=_contract())

    breaker.record(
        AgentAction(
            tool_name="run_shell_command",
            arguments={"command": "a"},
            return_code=1,
            error_message="a: command not found",
        )
    )
    verdict = breaker.record(
        AgentAction(tool_name="run_shell_command", arguments={"command": "b"}, return_code=0)
    )

    assert verdict.consecutive_errors == 0
    assert verdict.tripped is False


def test_pre_check_does_not_mutate_history():
    breaker = DriftCircuitBreaker(_blocking_config(), contract=_contract())

    breaker.pre_check(_shell("python -m unittest"))
    breaker.pre_check(_shell("python -m unittest"))

    assert breaker.history == []


def test_reset_clears_history():
    breaker = DriftCircuitBreaker(_config(), contract=_contract())

    breaker.record(_shell("uv run pytest"))
    breaker.reset()

    assert breaker.history == []
