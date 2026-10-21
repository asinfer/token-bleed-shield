from token_bleed_shield.prober.contract_generator import ContractGenerator
from token_bleed_shield.prober.system_prober import EnvironmentSnapshot, SystemProber, ToolchainInfo


def test_system_prober_capture():
    prober = SystemProber()
    snapshot = prober.capture()

    assert snapshot.os_name != ""
    assert snapshot.architecture != ""
    assert snapshot.workspace_root != ""
    assert snapshot.primary_shell != ""


def test_contract_generator():
    snapshot = EnvironmentSnapshot(
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
            forbidden_alternatives=["python", "pip"],
            command_prefix="uv run",
        ),
        detected_project_files=["pyproject.toml"],
    )

    markdown = ContractGenerator.render_markdown(snapshot)
    assert "# HOST ENVIRONMENT CONTRACT" in markdown
    assert "uv run" in markdown
    assert "NEVER invoke `python`, `pip`" in markdown
    assert "Zero Path Reconnaissance" in markdown
