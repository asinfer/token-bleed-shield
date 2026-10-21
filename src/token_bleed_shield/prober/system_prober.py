"""Deterministic system prober to discover local toolchains and runtimes."""

import os
import platform
import shutil
import subprocess
from pathlib import Path
from typing import List, Optional

from pydantic import BaseModel, Field


class ToolchainInfo(BaseModel):
    name: str
    available: bool
    version: Optional[str] = None
    path: Optional[str] = None
    preferred_for: List[str] = Field(default_factory=list)
    forbidden_alternatives: List[str] = Field(default_factory=list)
    command_prefix: Optional[str] = None


class EnvironmentSnapshot(BaseModel):
    os_name: str
    os_release: str
    architecture: str
    primary_shell: str
    workspace_root: str
    python_toolchain: Optional[ToolchainInfo] = None
    node_toolchain: Optional[ToolchainInfo] = None
    rust_toolchain: Optional[ToolchainInfo] = None
    detected_project_files: List[str] = Field(default_factory=list)
    git_branch: Optional[str] = None


class SystemProber:
    """Probes the host environment once to prevent LLM exploratory search loops."""

    def __init__(self, workspace_root: Optional[Path] = None):
        self.root = workspace_root or Path.cwd()

    def _get_command_version(self, cmd: List[str]) -> Optional[str]:
        try:
            res = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=2,
                shell=(platform.system() == "Windows"),
            )
            if res.returncode == 0:
                output = res.stdout.strip() or res.stderr.strip()
                return output.splitlines()[0] if output else None
        except Exception:
            pass
        return None

    def _detect_shell(self) -> str:
        if platform.system() == "Windows":
            if "PSModulePath" in os.environ:
                return "PowerShell"
            return "cmd.exe"
        shell = os.environ.get("SHELL", "")
        if "zsh" in shell:
            return "zsh"
        if "bash" in shell:
            return "bash"
        return "sh"

    def probe_python(self) -> Optional[ToolchainInfo]:
        has_uv = shutil.which("uv") is not None
        has_poetry = shutil.which("poetry") is not None
        has_python = shutil.which("python") is not None or shutil.which("python3") is not None

        if has_uv:
            ver = self._get_command_version(["uv", "--version"])
            forbidden = ["python", "python3", "pip", "pipenv"]
            if platform.system() == "Windows":
                # `py` is the Windows Python launcher and the classic wrong-binary trap.
                forbidden.append("py")
            return ToolchainInfo(
                name="uv",
                available=True,
                version=ver,
                path=shutil.which("uv"),
                preferred_for=["package_management", "script_execution", "venv_creation"],
                forbidden_alternatives=forbidden,
                command_prefix="uv run",
            )
        elif has_poetry:
            ver = self._get_command_version(["poetry", "--version"])
            return ToolchainInfo(
                name="poetry",
                available=True,
                version=ver,
                path=shutil.which("poetry"),
                preferred_for=["package_management", "script_execution"],
                forbidden_alternatives=["pip"],
                command_prefix="poetry run",
            )
        elif has_python:
            py_bin = shutil.which("python") or shutil.which("python3")
            ver = self._get_command_version([py_bin, "--version"]) if py_bin else None
            return ToolchainInfo(
                name="python-native",
                available=True,
                version=ver,
                path=py_bin,
                preferred_for=["script_execution"],
                forbidden_alternatives=[],
                command_prefix=Path(py_bin).name if py_bin else "python",
            )
        return None

    def probe_node(self) -> Optional[ToolchainInfo]:
        pnpm = shutil.which("pnpm")
        bun = shutil.which("bun")
        yarn = shutil.which("yarn")
        npm = shutil.which("npm")

        if pnpm and (self.root / "pnpm-lock.yaml").exists() or (pnpm and not (bun or yarn or npm)):
            ver = self._get_command_version(["pnpm", "--version"])
            return ToolchainInfo(
                name="pnpm",
                available=True,
                version=ver,
                path=pnpm,
                preferred_for=["package_management", "script_execution"],
                forbidden_alternatives=["npm", "yarn"],
                command_prefix="pnpm",
            )
        elif bun and (self.root / "bun.lockb").exists():
            ver = self._get_command_version(["bun", "--version"])
            return ToolchainInfo(
                name="bun",
                available=True,
                version=ver,
                path=bun,
                preferred_for=["package_management", "runtime"],
                forbidden_alternatives=["npm", "yarn"],
                command_prefix="bun",
            )
        elif npm:
            ver = self._get_command_version(["npm", "--version"])
            return ToolchainInfo(
                name="npm",
                available=True,
                version=ver,
                path=npm,
                preferred_for=["package_management"],
                forbidden_alternatives=[],
                command_prefix="npm",
            )
        return None

    def probe_rust(self) -> Optional[ToolchainInfo]:
        cargo = shutil.which("cargo")
        if cargo and ((self.root / "Cargo.toml").exists() or cargo):
            ver = self._get_command_version(["cargo", "--version"])
            return ToolchainInfo(
                name="cargo",
                available=True,
                version=ver,
                path=cargo,
                preferred_for=["build", "test", "run"],
                command_prefix="cargo",
            )
        return None

    def detect_project_files(self) -> List[str]:
        candidates = [
            "pyproject.toml",
            "uv.lock",
            "requirements.txt",
            "package.json",
            "pnpm-lock.yaml",
            "Cargo.toml",
            "go.mod",
            "Makefile",
            "docker-compose.yml",
            ".env.example",
        ]
        return [f for f in candidates if (self.root / f).exists()]

    def probe_git(self) -> Optional[str]:
        if (self.root / ".git").exists():
            return self._get_command_version(["git", "branch", "--show-current"])
        return None

    def capture(self) -> EnvironmentSnapshot:
        return EnvironmentSnapshot(
            os_name=platform.system(),
            os_release=platform.release(),
            architecture=platform.machine(),
            primary_shell=self._detect_shell(),
            workspace_root=str(self.root.resolve()),
            python_toolchain=self.probe_python(),
            node_toolchain=self.probe_node(),
            rust_toolchain=self.probe_rust(),
            detected_project_files=self.detect_project_files(),
            git_branch=self.probe_git(),
        )
