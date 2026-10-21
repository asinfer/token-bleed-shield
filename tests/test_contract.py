import pytest

from token_bleed_shield.contract import (
    ContractParseError,
    ContractSnapshot,
    load_contract,
    parse_contract,
)
from token_bleed_shield.prober.contract_generator import ContractGenerator
from token_bleed_shield.prober.system_prober import EnvironmentSnapshot, ToolchainInfo


def _snapshot() -> EnvironmentSnapshot:
    return EnvironmentSnapshot(
        os_name="Windows",
        os_release="11",
        architecture="AMD64",
        primary_shell="PowerShell",
        workspace_root="C:\\test\\workspace",
        python_toolchain=ToolchainInfo(
            name="uv",
            available=True,
            version="uv 0.6.0",
            path="C:\\uv.exe",
            preferred_for=["package_management"],
            forbidden_alternatives=["python", "python3", "pip", "pipenv"],
            command_prefix="uv run",
        ),
        node_toolchain=ToolchainInfo(
            name="pnpm",
            available=True,
            version="pnpm 9.1.0",
            path="C:\\pnpm.exe",
            preferred_for=["package_management"],
            forbidden_alternatives=["npm", "yarn"],
            command_prefix="pnpm",
        ),
        detected_project_files=["pyproject.toml"],
        git_branch="main",
    )


def test_markdown_keeps_human_readable_prose():
    markdown = ContractGenerator.render_markdown(_snapshot())

    assert "# HOST ENVIRONMENT CONTRACT" in markdown
    assert "uv run" in markdown
    assert "NEVER invoke `python`, `python3`, `pip`, `pipenv`" in markdown
    assert "Zero Path Reconnaissance" in markdown


def test_markdown_embeds_machine_readable_block():
    markdown = ContractGenerator.render_markdown(_snapshot())

    assert "<!-- TBS-CONTRACT-JSON -->" in markdown
    assert "<!-- /TBS-CONTRACT-JSON -->" in markdown
    assert "```json" in markdown


def test_contract_round_trips_through_markdown():
    markdown = ContractGenerator.render_markdown(_snapshot())
    parsed = parse_contract(markdown)

    assert parsed.contract_version == 1
    assert parsed.os_name == "Windows"
    assert parsed.architecture == "AMD64"
    assert parsed.primary_shell == "PowerShell"
    assert parsed.workspace_root == "C:\\test\\workspace"
    assert parsed.git_branch == "main"
    assert parsed.detected_project_files == ["pyproject.toml"]
    assert parsed.python is not None
    assert parsed.python.command_prefix == "uv run"
    assert parsed.python.forbidden == ["python", "python3", "pip", "pipenv"]


def test_forbidden_commands_is_the_union_of_every_toolchain():
    markdown = ContractGenerator.render_markdown(_snapshot())
    parsed = parse_contract(markdown)

    assert parsed.forbidden_commands == [
        "npm",
        "pip",
        "pipenv",
        "python",
        "python3",
        "yarn",
    ]


def test_parse_rejects_a_document_without_markers():
    with pytest.raises(ContractParseError) as excinfo:
        parse_contract("# just some markdown\n")

    assert "TBS-CONTRACT-JSON" in str(excinfo.value)


def test_parse_rejects_a_malformed_json_block():
    document = "<!-- TBS-CONTRACT-JSON -->\n```json\n{not json}\n```\n<!-- /TBS-CONTRACT-JSON -->"

    with pytest.raises(ContractParseError) as excinfo:
        parse_contract(document)

    assert "not valid JSON" in str(excinfo.value)


def test_parse_rejects_a_schema_mismatch():
    document = "<!-- TBS-CONTRACT-JSON -->\n```json\n{\"contract_version\": \"nope\"}\n```\n<!-- /TBS-CONTRACT-JSON -->"

    with pytest.raises(ContractParseError) as excinfo:
        parse_contract(document)

    assert "does not match the expected schema" in str(excinfo.value)


def test_load_contract_reports_a_missing_file(tmp_path):
    with pytest.raises(ContractParseError) as excinfo:
        load_contract(tmp_path / "nope.md")

    assert "not found" in str(excinfo.value)


def test_snapshot_with_no_toolchains_has_no_forbidden_commands():
    snapshot = ContractSnapshot(os_name="Linux")

    assert snapshot.forbidden_commands == []
    assert snapshot.toolchains == []
