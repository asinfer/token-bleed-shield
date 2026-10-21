import json

from typer.testing import CliRunner

from token_bleed_shield.cli import app
from token_bleed_shield.prober.contract_generator import ContractGenerator
from token_bleed_shield.prober.system_prober import EnvironmentSnapshot, ToolchainInfo

runner = CliRunner()


def _snapshot() -> EnvironmentSnapshot:
    return EnvironmentSnapshot(
        os_name="Linux",
        os_release="6.1",
        architecture="x86_64",
        primary_shell="bash",
        workspace_root="/workspace",
        python_toolchain=ToolchainInfo(
            name="uv",
            available=True,
            version="uv 0.12.15",
            path="/usr/bin/uv",
            preferred_for=["package_management"],
            forbidden_alternatives=["python", "python3", "pip", "pipenv"],
            command_prefix="uv run",
        ),
    )


def _write_contract(path) -> None:
    path.write_text(ContractGenerator.render_markdown(_snapshot()), encoding="utf-8")


def _write_blocking_config(path) -> None:
    path.write_text(
        "circuit_breaker:\n"
        "  default_enforcement: warn\n"
        "  rules:\n"
        "    forbidden_command:\n"
        "      enforcement: block\n",
        encoding="utf-8",
    )


def test_contract_command_reports_the_forbidden_commands(tmp_path):
    contract = tmp_path / ".agent-env.md"
    _write_contract(contract)

    result = runner.invoke(app, ["contract", "--contract", str(contract)])

    assert result.exit_code == 0
    assert "forbidden_command" not in result.output
    assert "python" in result.output


def test_contract_command_fails_on_a_missing_file(tmp_path):
    result = runner.invoke(app, ["contract", "--contract", str(tmp_path / "nope.md")])

    assert result.exit_code == 1


def test_gate_requires_a_file():
    result = runner.invoke(app, ["gate"])

    assert result.exit_code == 1
    assert "No file specified" in result.output


def test_gate_fails_on_a_missing_file(tmp_path):
    result = runner.invoke(app, ["gate", "--file", str(tmp_path / "nope.txt")])

    assert result.exit_code == 1
    # Rich wraps console output, so compare against a whitespace-normalised copy.
    flattened = " ".join(result.output.split())
    assert "not found" in flattened


def test_gate_evaluates_a_real_file(tmp_path):
    payload = tmp_path / "context.txt"
    payload.write_text("a short prompt", encoding="utf-8")

    result = runner.invoke(app, ["gate", "--file", str(payload)])

    assert result.exit_code == 0
    assert "Estimated Tokens" in result.output


def test_breaker_replay_blocks_a_forbidden_command_before_execution(tmp_path):
    contract = tmp_path / ".agent-env.md"
    _write_contract(contract)
    config = tmp_path / "config.yaml"
    _write_blocking_config(config)

    history = tmp_path / "history.jsonl"
    history.write_text(
        json.dumps(
            {
                "tool_name": "run_shell_command",
                "arguments": {"command": "python -m unittest"},
                "return_code": 1,
                "error_message": "python: command not found",
            }
        )
        + "\n",
        encoding="utf-8",
    )

    result = runner.invoke(
        app,
        [
            "breaker",
            "--history",
            str(history),
            "--contract",
            str(contract),
            "--config",
            str(config),
        ],
    )

    assert result.exit_code == 0
    assert "BLOCKED" in result.output
    assert "Preventable before execution" in result.output


def test_breaker_replay_fails_on_a_missing_history(tmp_path):
    result = runner.invoke(app, ["breaker", "--history", str(tmp_path / "nope.jsonl")])

    assert result.exit_code == 1


def test_breaker_short_flags_do_not_collide(tmp_path):
    contract = tmp_path / ".agent-env.md"
    _write_contract(contract)
    config = tmp_path / "config.yaml"
    _write_blocking_config(config)

    history = tmp_path / "history.jsonl"
    history.write_text(
        json.dumps(
            {
                "tool_name": "run_shell_command",
                "arguments": {"command": "python -m unittest"},
                "return_code": 1,
                "error_message": "python: command not found",
            }
        )
        + "\n",
        encoding="utf-8",
    )

    result = runner.invoke(
        app,
        [
            "breaker",
            "--history",
            str(history),
            "-c",
            str(contract),
            "-C",
            str(config),
        ],
    )

    assert result.exit_code == 0
    assert "BLOCKED" in result.output


def test_breaker_enforcement_override_applies_to_every_rule(tmp_path):
    contract = tmp_path / ".agent-env.md"
    _write_contract(contract)

    # A config that would otherwise warn on the forbidden command.
    config = tmp_path / "config.yaml"
    config.write_text(
        "circuit_breaker:\n"
        "  default_enforcement: warn\n"
        "  rules:\n"
        "    forbidden_command:\n"
        "      enforcement: warn\n",
        encoding="utf-8",
    )

    history = tmp_path / "history.jsonl"
    history.write_text(
        json.dumps(
            {
                "tool_name": "run_shell_command",
                "arguments": {"command": "python -m unittest"},
                "return_code": 1,
                "error_message": "python: command not found",
            }
        )
        + "\n",
        encoding="utf-8",
    )

    result = runner.invoke(
        app,
        [
            "breaker",
            "--history",
            str(history),
            "--contract",
            str(contract),
            "--config",
            str(config),
            "--enforcement",
            "block",
        ],
    )

    assert result.exit_code == 0
    flattened = " ".join(result.output.split())
    assert "Enforcement: block" in flattened
