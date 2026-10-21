from token_bleed_shield.config import CircuitBreakerConfig, Enforcement, RuleConfig
from token_bleed_shield.policy import CircuitBreakerPolicy, Signal


def _config(**kwargs) -> CircuitBreakerConfig:
    return CircuitBreakerConfig(**kwargs)


def test_default_enforcement_applies_when_no_rule_is_configured():
    policy = CircuitBreakerPolicy(_config(default_enforcement=Enforcement.WARN))

    assert policy.enforcement_for(Signal.OSCILLATION) is Enforcement.WARN


def test_per_rule_configuration_overrides_the_default():
    config = _config(
        default_enforcement=Enforcement.WARN,
        rules={"forbidden_command": RuleConfig(enforcement=Enforcement.BLOCK)},
    )
    policy = CircuitBreakerPolicy(config)

    assert policy.enforcement_for(Signal.FORBIDDEN_COMMAND) is Enforcement.BLOCK
    assert policy.enforcement_for(Signal.OSCILLATION) is Enforcement.WARN


def test_block_enforcement_marks_the_decision_as_blocking():
    policy = CircuitBreakerPolicy(
        _config(rules={"identical_args": RuleConfig(enforcement=Enforcement.BLOCK)})
    )

    decision = policy.decide(Signal.IDENTICAL_ARGS, "because")

    assert decision.block is True
    assert decision.escalate is False
    assert decision.severity == "CRITICAL"


def test_ask_enforcement_escalates_instead_of_blocking():
    policy = CircuitBreakerPolicy(
        _config(rules={"error_cascade": RuleConfig(enforcement=Enforcement.ASK)})
    )

    decision = policy.decide(Signal.ERROR_CASCADE, "because")

    assert decision.block is False
    assert decision.escalate is True


def test_warn_enforcement_never_blocks():
    policy = CircuitBreakerPolicy(_config(default_enforcement=Enforcement.WARN))

    decision = policy.decide(Signal.SAME_TOOL_STREAK, "because")

    assert decision.block is False
    assert decision.escalate is False
    assert decision.severity == "WARNING"


def test_signal_values_are_the_config_keys():
    assert Signal.FORBIDDEN_COMMAND.value == "forbidden_command"
    assert Signal.IDENTICAL_ARGS.value == "identical_args"
    assert Signal.SAME_TOOL_STREAK.value == "same_tool_streak"
    assert Signal.REPEATED_ERROR_SIGNATURE.value == "repeated_error_signature"
    assert Signal.ERROR_CASCADE.value == "error_cascade"
    assert Signal.OSCILLATION.value == "oscillation"
