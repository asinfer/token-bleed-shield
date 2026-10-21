from token_bleed_shield.config import BudgetGateConfig
from token_bleed_shield.guards.context_budget import ContextBudgetGate


def test_context_budget_evaluation():
    config = BudgetGateConfig(max_context_tokens=1000, warn_threshold_tokens=500)
    gate = ContextBudgetGate(config)

    clean_content = "This is a clean and concise prompt asking to refactor a function."
    assessment = gate.evaluate(clean_content)
    assert assessment.is_safe is True
    assert assessment.should_reject is False
    assert len(assessment.warnings) == 0


def test_context_budget_detects_repeated_stacktraces():
    gate = ContextBudgetGate()
    st = (
        "Traceback (most recent call last):\n"
        "  File 'main.py', line 12, in <module>\n"
        "    foo()\n"
        "ZeroDivisionError: division by zero\n"
    )
    bloated_content = st * 4

    assessment = gate.evaluate(bloated_content)
    categories = [w.category for w in assessment.warnings]
    assert "Repeated Stacktraces" in categories
