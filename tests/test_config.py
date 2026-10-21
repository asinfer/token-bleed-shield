from token_bleed_shield.config import (
    CircuitBreakerConfig,
    Enforcement,
    RuleConfig,
    ShieldConfig,
    Signal,
)


def test_enforcement_levels_are_stable_strings():
    assert Enforcement.BLOCK.value == "block"
    assert Enforcement.WARN.value == "warn"
    assert Enforcement.ASK.value == "ask"
    assert Enforcement.OFF.value == "off"


def test_load_reads_rules_from_disk(tmp_path):
    path = tmp_path / ".tokenbleedshield.yaml"
    path.write_text(
        "circuit_breaker:\n"
        "  default_enforcement: warn\n"
        "  rules:\n"
        "    forbidden_command:\n"
        "      enforcement: block\n",
        encoding="utf-8",
    )

    config = ShieldConfig.load(path)

    assert config.circuit_breaker.default_enforcement is Enforcement.WARN
    assert config.circuit_breaker.rules["forbidden_command"].enforcement is Enforcement.BLOCK


def test_load_falls_back_to_defaults_when_file_is_missing(tmp_path):
    config = ShieldConfig.load(tmp_path / "does-not-exist.yaml")

    assert config.circuit_breaker.default_enforcement is Enforcement.WARN
    assert config.circuit_breaker.max_repeated_error_signature == 2
    assert config.budget_gate.max_context_tokens == 32000


def test_save_and_load_round_trip(tmp_path):
    path = tmp_path / "config.yaml"

    config = ShieldConfig()
    config.circuit_breaker.rules["forbidden_command"] = RuleConfig(
        enforcement=Enforcement.BLOCK
    )
    config.save(path)

    reloaded = ShieldConfig.load(path)
    assert reloaded.circuit_breaker.rules["forbidden_command"].enforcement is (
        Enforcement.BLOCK
    )


def test_save_writes_plain_strings_for_enums(tmp_path):
    path = tmp_path / "config.yaml"

    config = ShieldConfig()
    config.circuit_breaker.default_enforcement = Enforcement.ASK
    config.save(path)

    raw = path.read_text(encoding="utf-8")
    assert "ask" in raw
    assert "Enforcement." not in raw
    assert "!!python" not in raw


def test_default_circuit_breaker_config_uses_the_recommended_rules():
    config = CircuitBreakerConfig()

    assert config.default_enforcement is Enforcement.WARN
    assert config.rules[Signal.FORBIDDEN_COMMAND.value].enforcement is Enforcement.BLOCK
    assert config.rules[Signal.IDENTICAL_ARGS.value].enforcement is Enforcement.BLOCK
    assert config.rules[Signal.OSCILLATION.value].enforcement is Enforcement.WARN


def test_recommended_rules_are_not_shared_between_instances():
    first = CircuitBreakerConfig()
    second = CircuitBreakerConfig()

    first.rules[Signal.OSCILLATION.value] = RuleConfig(enforcement=Enforcement.OFF)

    assert second.rules[Signal.OSCILLATION.value].enforcement is Enforcement.WARN
