"""The machine-readable environment contract.

``.agent-env.md`` is a human-readable contract, but an agent guardrail needs to read it
programmatically. Rather than shipping two files that can drift apart, the markdown embeds a
fenced JSON block delimited by explicit markers. One artifact, two audiences.

The parsed :class:`ContractSnapshot` is what the circuit breaker consults before a tool runs.
"""

import json
from pathlib import Path
from typing import List, Optional

from pydantic import BaseModel, Field

CONTRACT_VERSION = 1
CONTRACT_BEGIN_MARKER = "<!-- TBS-CONTRACT-JSON -->"
CONTRACT_END_MARKER = "<!-- /TBS-CONTRACT-JSON -->"


class ContractParseError(ValueError):
    """Raised when a contract file cannot be parsed."""


class ToolchainContract(BaseModel):
    """A single pinned toolchain and the commands that must not be used directly."""

    name: str
    version: Optional[str] = None
    command_prefix: Optional[str] = None
    forbidden: List[str] = Field(default_factory=list)


class ContractSnapshot(BaseModel):
    """Structured view of the environment contract."""

    contract_version: int = CONTRACT_VERSION
    os_name: str = ""
    os_release: str = ""
    architecture: str = ""
    primary_shell: str = ""
    workspace_root: str = ""
    git_branch: Optional[str] = None
    detected_project_files: List[str] = Field(default_factory=list)
    python: Optional[ToolchainContract] = None
    node: Optional[ToolchainContract] = None
    rust: Optional[ToolchainContract] = None

    @property
    def toolchains(self) -> List[ToolchainContract]:
        """All pinned toolchains that carry a contract, in a stable order."""
        return [tc for tc in (self.python, self.node, self.rust) if tc is not None]

    @property
    def forbidden_commands(self) -> List[str]:
        """Union of every toolchain's forbidden command names, lowercased and de-duplicated."""
        names = set()
        for toolchain in self.toolchains:
            for name in toolchain.forbidden:
                names.add(name.lower())
        return sorted(names)


def render_json_block(snapshot: ContractSnapshot) -> str:
    """Render the fenced JSON block that is embedded in the contract markdown."""
    payload = json.dumps(snapshot.model_dump(exclude_none=True), indent=2, sort_keys=False)
    return (
        f"{CONTRACT_BEGIN_MARKER}\n"
        f"```json\n"
        f"{payload}\n"
        f"```\n"
        f"{CONTRACT_END_MARKER}"
    )


def extract_json_payload(text: str) -> str:
    """Return the raw JSON string embedded between the contract markers."""
    start = text.find(CONTRACT_BEGIN_MARKER)
    if start == -1:
        raise ContractParseError(
            f"Contract is missing the {CONTRACT_BEGIN_MARKER} marker. "
            "Regenerate it with `tbs probe`."
        )

    end = text.find(CONTRACT_END_MARKER, start)
    if end == -1:
        raise ContractParseError(
            f"Contract is missing the {CONTRACT_END_MARKER} marker. "
            "Regenerate it with `tbs probe`."
        )

    block = text[start + len(CONTRACT_BEGIN_MARKER) : end].strip()
    if block.startswith("```"):
        block = block[3:]
        if block.lower().startswith("json"):
            block = block[4:]
        block = block.strip()
        if block.endswith("```"):
            block = block[:-3].strip()

    if not block:
        raise ContractParseError("Contract JSON block is empty. Regenerate it with `tbs probe`.")

    return block


def parse_contract(text: str) -> ContractSnapshot:
    """Parse a contract markdown document into a :class:`ContractSnapshot`."""
    payload = extract_json_payload(text)
    try:
        data = json.loads(payload)
    except json.JSONDecodeError as exc:
        raise ContractParseError(f"Contract JSON block is not valid JSON: {exc}") from exc

    if not isinstance(data, dict):
        raise ContractParseError("Contract JSON block must contain a JSON object.")

    try:
        return ContractSnapshot(**data)
    except Exception as exc:
        raise ContractParseError(f"Contract JSON block does not match the expected schema: {exc}") from exc


def load_contract(path: Path) -> ContractSnapshot:
    """Read and parse a contract file from disk."""
    if not path.is_file():
        raise ContractParseError(f"Contract file not found: {path}. Run `tbs probe` first.")
    return parse_contract(path.read_text(encoding="utf-8"))
