"""Linter and minifier for JSON Schema and MCP tool specifications."""

import json
from typing import Any, Dict, List, Optional

from pydantic import BaseModel, Field

from ..config import ToolLinterConfig


class ToolStats(BaseModel):
    name: str
    original_tokens: int
    minified_tokens: int
    savings_percent: float


class LinterReport(BaseModel):
    total_tools: int
    original_tokens: int
    minified_tokens: int
    saved_tokens_per_call: int
    saved_tokens_projected_10_turns: int
    savings_percent: float
    tools: List[ToolStats] = Field(default_factory=list)
    minified_schema: List[Dict[str, Any]] = Field(default_factory=list)


class ToolSchemaLinter:
    """Minifies and prunes tool definitions to slash repeated prompt tokens."""

    def __init__(self, config: Optional[ToolLinterConfig] = None):
        # A default argument is evaluated once at import time, so a mutable default would be
        # shared by every instance in the process and mutated by whoever configures it.
        self.config = config or ToolLinterConfig()

    @staticmethod
    def estimate_tokens(obj: Any) -> int:
        serialized = json.dumps(obj, separators=(",", ":"))
        # Standard token heuristic for dense JSON (approx 3.7 chars per token)
        return max(1, int(len(serialized) / 3.7))

    def _minify_node(self, node: Any) -> Any:
        if isinstance(node, dict):
            minified = {}
            for k, v in node.items():
                if self.config.strip_titles and k == "title":
                    continue
                if self.config.strip_examples and k in ("examples", "example"):
                    continue
                if k == "description" and isinstance(v, str):
                    # Strip leading/trailing whitespaces and truncate if over threshold
                    cleaned = " ".join(v.strip().split())
                    if len(cleaned) > self.config.max_description_length:
                        cleaned = cleaned[: self.config.max_description_length].rstrip() + "..."
                    minified[k] = cleaned
                else:
                    minified[k] = self._minify_node(v)
            return minified
        elif isinstance(node, list):
            return [self._minify_node(item) for item in node]
        return node

    def minify_tool(self, tool: Dict[str, Any]) -> Dict[str, Any]:
        """Minifies a single OpenAI/Anthropic/MCP tool definition."""
        return self._minify_node(tool)

    def lint_tools(self, tools: List[Dict[str, Any]]) -> LinterReport:
        tool_stats = []
        minified_list = []
        total_orig = 0
        total_mini = 0

        for tool in tools:
            # Detect tool name
            name = tool.get("name") or tool.get("function", {}).get("name", "unnamed_tool")
            orig_tokens = self.estimate_tokens(tool)
            minified = self.minify_tool(tool)
            mini_tokens = self.estimate_tokens(minified)

            total_orig += orig_tokens
            total_mini += mini_tokens

            savings = 0.0
            if orig_tokens > 0:
                savings = round(((orig_tokens - mini_tokens) / orig_tokens) * 100, 1)

            tool_stats.append(
                ToolStats(
                    name=name,
                    original_tokens=orig_tokens,
                    minified_tokens=mini_tokens,
                    savings_percent=savings,
                )
            )
            minified_list.append(minified)

        overall_savings = 0.0
        if total_orig > 0:
            overall_savings = round(((total_orig - total_mini) / total_orig) * 100, 1)

        saved_per_call = total_orig - total_mini

        return LinterReport(
            total_tools=len(tools),
            original_tokens=total_orig,
            minified_tokens=total_mini,
            saved_tokens_per_call=saved_per_call,
            saved_tokens_projected_10_turns=saved_per_call * 10,
            savings_percent=overall_savings,
            tools=tool_stats,
            minified_schema=minified_list,
        )
