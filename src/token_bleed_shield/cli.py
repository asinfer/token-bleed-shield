import json
import sys
from pathlib import Path
from typing import List, Optional

import typer
from rich.console import Console
from rich.panel import Panel
from rich.table import Table

if sys.platform == "win32":
    try:
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")
    except Exception:
        pass

from .config import Enforcement, ShieldConfig
from .contract import ContractParseError, ContractSnapshot, load_contract
from .guards.circuit_breaker import AgentAction, BreakerVerdict, DriftCircuitBreaker
from .guards.context_budget import ContextBudgetGate
from .prober.contract_generator import ContractGenerator
from .prober.system_prober import SystemProber
from .tools.schema_linter import ToolSchemaLinter

app = typer.Typer(
    name="tbs",
    help="Token Bleed Shield: Preventive workflow to stop bleeding AI API costs before generation starts.",
    add_completion=False,
)
console = Console(legacy_windows=False)

DEFAULT_CONFIG_PATH = Path(".tokenbleedshield.yaml")
DEFAULT_CONTRACT_PATH = Path(".agent-env.md")


def _load_config(config_path: Optional[Path]) -> ShieldConfig:
    """Load configuration from disk, falling back to built-in defaults."""
    return ShieldConfig.load(config_path or DEFAULT_CONFIG_PATH)


def _load_contract(contract_path: Path) -> Optional[ContractSnapshot]:
    """Load the environment contract if it exists, otherwise return ``None``."""
    if not contract_path.is_file():
        return None
    try:
        return load_contract(contract_path)
    except ContractParseError as exc:
        console.print(f"[bold yellow]Warning:[/bold yellow] {exc}")
        return None


def _load_actions(history_file: Path) -> List[AgentAction]:
    """Parse a JSON or JSONL action history file."""
    content = history_file.read_text(encoding="utf-8").strip()
    if history_file.suffix == ".jsonl":
        records = [json.loads(line) for line in content.splitlines() if line.strip()]
    else:
        raw = json.loads(content)
        records = raw if isinstance(raw, list) else [raw]
    return [AgentAction(**record) for record in records]


@app.command()
def probe(
    output: Optional[Path] = typer.Option(
        None, "--output", "-o", help="Target path for generated .agent-env.md contract"
    ),
    json_output: Optional[Path] = typer.Option(
        None, "--json", help="Optional path to output raw JSON snapshot"
    ),
    config: Optional[Path] = typer.Option(
        None, "--config", "-C", help="Path to a Token Bleed Shield config file"
    ),
):
    """Probe the local host environment once and generate an immutable agent contract."""
    console.print(Panel.fit("[bold cyan]🛡️ Token Bleed Shield — Pre-flight Prober[/bold cyan]"))

    shield_config = _load_config(config)

    prober = SystemProber()
    with console.status("[bold green]Probing local toolchains and runtimes..."):
        snapshot = prober.capture()

    table = Table(title="Host Environment Snapshot", show_header=True, header_style="bold magenta")
    table.add_column("Component", style="dim", width=22)
    table.add_column("Detected Runtime / Policy")

    table.add_row("Operating System", f"{snapshot.os_name} {snapshot.os_release} ({snapshot.architecture})")
    table.add_row("Primary Shell", snapshot.primary_shell)
    table.add_row("Workspace Root", snapshot.workspace_root)

    if snapshot.python_toolchain:
        tc = snapshot.python_toolchain
        status = f"[green]{tc.name}[/green]"
        if tc.version:
            status += f" ({tc.version})"
        if tc.command_prefix:
            status += f" -> Run via `{tc.command_prefix}`"
        table.add_row("Python Toolchain", status)

    if snapshot.node_toolchain:
        tc = snapshot.node_toolchain
        status = f"[green]{tc.name}[/green]"
        if tc.version:
            status += f" ({tc.version})"
        table.add_row("Node Toolchain", status)

    if snapshot.rust_toolchain:
        table.add_row("Rust Toolchain", f"[green]{snapshot.rust_toolchain.name}[/green]")

    if snapshot.detected_project_files:
        table.add_row("Project Files", ", ".join(snapshot.detected_project_files))

    console.print(table)

    contract_path = ContractGenerator.write_contract(
        snapshot, output or Path(shield_config.prober.contract_filename)
    )
    console.print(f"\n[bold green]✔ Successfully locked runtime contract to:[/bold green] [underline]{contract_path}[/underline]")
    console.print("[dim]Inject this contract into your agent's system prompt to stop exploratory tool loops.[/dim]\n")

    if json_output:
        json_output.write_text(snapshot.model_dump_json(indent=2), encoding="utf-8")
        console.print(f"[green]✔ Saved JSON snapshot to:[/green] {json_output}")


@app.command("contract")
def show_contract(
    contract: Path = typer.Option(
        DEFAULT_CONTRACT_PATH, "--contract", "-c", help="Path to the environment contract"
    ),
):
    """Show the machine-readable environment contract that guardrails enforce."""
    console.print(Panel.fit("[bold cyan]📜 Environment Contract[/bold cyan]"))

    if not contract.is_file():
        console.print(f"[bold red]Error:[/bold red] Contract {contract} not found. Run `tbs probe` first.")
        raise typer.Exit(code=1)

    try:
        snapshot = load_contract(contract)
    except ContractParseError as exc:
        console.print(f"[bold red]Error:[/bold red] {exc}")
        raise typer.Exit(code=1)

    table = Table(show_header=True, header_style="bold magenta")
    table.add_column("Field", style="dim", width=20)
    table.add_column("Value")

    table.add_row("Contract Version", str(snapshot.contract_version))
    table.add_row("OS", f"{snapshot.os_name} {snapshot.os_release} ({snapshot.architecture})")
    table.add_row("Shell", snapshot.primary_shell)
    table.add_row("Workspace Root", snapshot.workspace_root)
    if snapshot.git_branch:
        table.add_row("Git Branch", snapshot.git_branch)
    if snapshot.detected_project_files:
        table.add_row("Project Files", ", ".join(snapshot.detected_project_files))

    for toolchain in snapshot.toolchains:
        label = toolchain.name.capitalize()
        detail = toolchain.name
        if toolchain.version:
            detail += f" ({toolchain.version})"
        if toolchain.command_prefix:
            detail += f" -> `{toolchain.command_prefix}`"
        table.add_row(label, detail)
        if toolchain.forbidden:
            forbidden = ", ".join(f"`{name}`" for name in toolchain.forbidden)
            table.add_row(f"{label} Forbidden", forbidden)

    console.print(table)

    if snapshot.forbidden_commands:
        console.print(
            f"\n[bold]Guardrail:[/bold] invocations of "
            f"[bold red]{', '.join(snapshot.forbidden_commands)}[/bold red] are blocked "
            "before execution.\n"
        )
    else:
        console.print("\n[dim]No forbidden commands declared in this contract.[/dim]\n")


@app.command("lint-tools")
def lint_tools(
    input_file: Path = typer.Option(..., "--input", "-i", help="JSON file containing tool definitions"),
    output_file: Optional[Path] = typer.Option(
        None, "--output", "-o", help="Optional path to write minified schema"
    ),
    strip_titles: bool = typer.Option(True, "--strip-titles/--keep-titles", help="Strip JSON schema 'title' fields"),
    max_desc_length: int = typer.Option(140, "--max-desc", help="Max character length for parameter descriptions"),
    config: Optional[Path] = typer.Option(
        None, "--config", "-C", help="Path to a Token Bleed Shield config file"
    ),
):
    """Minify tool and MCP schemas to cut 40-70% of static prompt token bloat."""
    console.print(Panel.fit("[bold cyan]🛠️ Tool Schema Minifier & Linter[/bold cyan]"))

    if not input_file.exists():
        console.print(f"[bold red]Error:[/bold red] Input file {input_file} not found.")
        raise typer.Exit(code=1)

    try:
        raw_data = json.loads(input_file.read_text(encoding="utf-8"))
        tools = raw_data if isinstance(raw_data, list) else raw_data.get("tools", [raw_data])
    except Exception as e:
        console.print(f"[bold red]Failed to parse JSON:[/bold red] {e}")
        raise typer.Exit(code=1)

    shield_config = _load_config(config)
    linter = ToolSchemaLinter(shield_config.tool_linter)
    linter.config.strip_titles = strip_titles
    linter.config.max_description_length = max_desc_length
    report = linter.lint_tools(tools)

    table = Table(title="Tool Optimization Breakdown", show_header=True, header_style="bold magenta")
    table.add_column("Tool Name", style="bold")
    table.add_column("Original Tokens", justify="right")
    table.add_column("Minified Tokens", justify="right")
    table.add_column("Saved %", justify="right", style="green")

    for t in report.tools:
        table.add_row(t.name, str(t.original_tokens), str(t.minified_tokens), f"{t.savings_percent}%")

    console.print(table)

    summary_panel = (
        f"[bold]Total Tools:[/bold] {report.total_tools}\n"
        f"[bold]Original Weight:[/bold] {report.original_tokens} tokens\n"
        f"[bold]Minified Weight:[/bold] {report.minified_tokens} tokens\n"
        f"[bold green]Saved per API call:[/bold green] {report.saved_tokens_per_call} tokens ({report.savings_percent}% reduction)\n"
        f"[bold cyan]Projected 10-turn savings:[/bold cyan] ~{report.saved_tokens_projected_10_turns:,} tokens saved!"
    )
    console.print(Panel(summary_panel, title="Token Savings Summary", border_style="green"))

    if output_file:
        output_file.write_text(json.dumps(report.minified_schema, indent=2), encoding="utf-8")
        console.print(f"[green]✔ Minified schemas saved to:[/green] {output_file}")


@app.command()
def gate(
    file: Optional[Path] = typer.Option(None, "--file", "-f", help="File to evaluate (prompt or context dump)"),
    max_tokens: int = typer.Option(32000, "--max-tokens", help="Maximum allowable context tokens"),
    warn_threshold: int = typer.Option(24000, "--warn", help="Warning threshold in tokens"),
    config: Optional[Path] = typer.Option(
        None, "--config", "-C", help="Path to a Token Bleed Shield config file"
    ),
):
    """Pre-flight check context payload before sending to LLM API."""
    console.print(Panel.fit("[bold cyan]🚪 Context Budget Gate[/bold cyan]"))

    shield_config = _load_config(config)

    if file is None:
        console.print("[bold red]Error:[/bold red] No file specified. Pass --file <path>.")
        raise typer.Exit(code=1)

    if not file.is_file():
        console.print(f"[bold red]Error:[/bold red] File {file} not found.")
        raise typer.Exit(code=1)

    content = file.read_text(encoding="utf-8")

    gate_checker = ContextBudgetGate(shield_config.budget_gate)
    gate_checker.config.max_context_tokens = max_tokens
    gate_checker.config.warn_threshold_tokens = warn_threshold

    assessment = gate_checker.evaluate(content)

    status_color = "green" if assessment.is_safe else "yellow"
    if assessment.should_reject:
        status_color = "red"

    console.print(
        f"Estimated Tokens: [{status_color}]{assessment.estimated_tokens:,}[/{status_color}] / "
        f"{assessment.max_tokens_allowed:,} ({assessment.utilization_percent}%)\n"
    )

    if assessment.warnings:
        for w in assessment.warnings:
            console.print(
                Panel(
                    f"[bold yellow]{w.description}[/bold yellow]\n\n"
                    f"[dim]Impact:[/dim] ~{w.estimated_tokens:,} tokens\n"
                    f"[bold green]Action:[/bold green] {w.recommendation}",
                    title=f"⚠️ {w.category}",
                    border_style="yellow",
                )
            )
    else:
        console.print("[bold green]✔ Context payload is clean and within budget limits.[/bold green]")


@app.command()
def breaker(
    history_file: Path = typer.Option(..., "--history", "-h", help="JSON or JSONL file with agent action logs"),
    contract: Path = typer.Option(
        DEFAULT_CONTRACT_PATH, "--contract", "-c", help="Path to the environment contract"
    ),
    enforcement: Optional[Enforcement] = typer.Option(
        None,
        "--enforcement",
        "-e",
        help="Override the enforcement level for every rule in this replay (block|warn|ask|off)",
    ),
    config: Optional[Path] = typer.Option(
        None, "--config", "-C", help="Path to a Token Bleed Shield config file"
    ),
):
    """Analyze agent tool history to detect infinite loops and model drift.

    Each action is evaluated twice: once *before* execution (what a live guardrail would have
    blocked) and once *after* (the post-hoc detection point).
    """
    console.print(Panel.fit("[bold cyan]⚡ Drift & Loop Circuit Breaker[/bold cyan]"))

    if not history_file.exists():
        console.print(f"[bold red]Error:[/bold red] History file {history_file} not found.")
        raise typer.Exit(code=1)

    shield_config = _load_config(config)
    if enforcement is not None:
        # A true override: every rule adopts the requested level, not just the default.
        shield_config.circuit_breaker.default_enforcement = enforcement
        for rule in shield_config.circuit_breaker.rules.values():
            rule.enforcement = enforcement

    snapshot = _load_contract(contract)
    if snapshot is None:
        console.print(
            "[dim]No environment contract found - forbidden-command detection is disabled. "
            "Run `tbs probe` to enable it.[/dim]\n"
        )
    else:
        console.print(
            f"Contract: [underline]{contract}[/underline] "
            f"(forbidden: {', '.join(snapshot.forbidden_commands) or 'none'})\n"
        )

    breaker_engine = DriftCircuitBreaker(shield_config.circuit_breaker, contract=snapshot)

    try:
        actions = _load_actions(history_file)
    except Exception as e:
        console.print(f"[bold red]Failed to load actions:[/bold red] {e}")
        raise typer.Exit(code=1)

    console.print(f"Loaded {len(actions)} historical agent actions.\n")

    blocked_steps: List[int] = []
    detected_steps: List[int] = []

    for idx, act in enumerate(actions, start=1):
        pre_verdict = breaker_engine.pre_check(act)
        post_verdict = breaker_engine.record(act)

        if pre_verdict is not None and pre_verdict.tripped:
            blocked_steps.append(idx)
            _print_step(idx, act, pre_verdict, blocked=True)
        elif post_verdict.tripped:
            detected_steps.append(idx)
            _print_step(idx, act, post_verdict, blocked=False)
        else:
            console.print(f"[dim]Step {idx}: allowed ({act.tool_name})[/dim]")

    console.print()
    summary = (
        f"[bold]Steps analysed:[/bold] {len(actions)}\n"
        f"[bold green]Preventable before execution:[/bold green] "
        f"{len(blocked_steps)} step(s){' -> ' + ', '.join(str(s) for s in blocked_steps) if blocked_steps else ''}\n"
        f"[bold yellow]Detected only after execution:[/bold yellow] "
        f"{len(detected_steps)} step(s){' -> ' + ', '.join(str(s) for s in detected_steps) if detected_steps else ''}\n"
        f"[dim]Replay assumes the agent retried identically after a block; real behaviour may diverge.[/dim]"
    )
    console.print(Panel(summary, title="Replay Summary", border_style="cyan"))


def _print_step(idx: int, act: AgentAction, verdict: BreakerVerdict, blocked: bool) -> None:
    """Render one replay step."""
    if blocked:
        header = f"[bold red]Step {idx}: BLOCKED before execution[/bold red]"
        border = "red"
    elif verdict.enforcement is Enforcement.ASK:
        header = f"[bold yellow]Step {idx}: ESCALATE[/bold yellow]"
        border = "yellow"
    else:
        header = f"[bold yellow]Step {idx}: detected after execution[/bold yellow]"
        border = "yellow"

    body = (
        f"[bold]Tool:[/bold] `{act.tool_name}`\n"
        f"[bold]Signals:[/bold] {', '.join(verdict.signals)}\n"
        f"[bold]Enforcement:[/bold] {verdict.enforcement.value}\n\n"
        f"[bold]Reason:[/bold] {verdict.reason}\n\n"
        f"[bold green]Action:[/bold green] {verdict.suggested_action}"
    )
    console.print(Panel(body, title=header, border_style=border))


@app.command()
def init(
    config: Optional[Path] = typer.Option(
        None, "--config", "-C", help="Path to a Token Bleed Shield config file"
    ),
):
    """Initialize Token Bleed Shield in the current repository."""
    console.print(Panel.fit("[bold cyan]🛡️ Initializing Token Bleed Shield[/bold cyan]"))

    target = config or DEFAULT_CONFIG_PATH
    shield_config = ShieldConfig.load(target)
    shield_config.save(target)
    console.print(f"[green]✔ Wrote configuration file:[/green] `{target}`")

    prober = SystemProber()
    snapshot = prober.capture()
    contract_path = ContractGenerator.write_contract(snapshot)
    console.print(f"[green]✔ Generated pre-flight contract:[/green] `{contract_path}`")
    console.print("\n[bold]You are all set![/bold] Token Bleed Shield is active in this project.")


if __name__ == "__main__":
    app()
