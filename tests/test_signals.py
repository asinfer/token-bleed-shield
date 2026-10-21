from token_bleed_shield.guards.signals import (
    classify_error,
    extract_command_heads,
    find_forbidden_command,
    normalize_binary,
)


def test_classify_error_returns_none_for_success():
    assert classify_error(None, 0) == "none"
    assert classify_error("", 0) == "none"


def test_classify_error_distinguishes_wrong_binary_from_missing_file():
    assert classify_error("python: command not found", 1) == "command_not_found"
    assert classify_error(
        "'python' is not recognized as an internal or external command", 1
    ) == "command_not_found"
    assert classify_error("config.yaml: No such file or directory", 1) == "not_found"


def test_classify_error_buckets_common_failures():
    assert classify_error("Permission denied", 1) == "permission_denied"
    assert classify_error("Operation timed out", 1) == "timeout"
    assert classify_error("connection refused", 1) == "network"
    assert classify_error("429 Too Many Requests", 429) == "rate_limited"
    assert classify_error("ModuleNotFoundError: No module named 'x'", 1) == "import_error"
    assert classify_error("SyntaxError: invalid syntax", 1) == "syntax_error"
    assert classify_error("something entirely unexpected", 1) == "unknown"


def test_classify_error_handles_a_failure_without_a_message():
    assert classify_error(None, 1) == "unknown"


def test_normalize_binary_strips_version_suffixes():
    assert normalize_binary("python3.11") == "python"
    assert normalize_binary("python3") == "python"
    assert normalize_binary("PYTHON") == "python"
    assert normalize_binary("pythonic") == "pythonic"


def test_extract_command_heads_reads_the_leading_binary():
    assert extract_command_heads({"command": "python -m unittest"}) == ["python"]
    assert extract_command_heads({"cmd": "uv run pytest"}) == ["uv"]


def test_extract_command_heads_splits_chained_segments():
    heads = extract_command_heads({"command": "cd src && python app.py ; pip install x"})

    assert heads == ["cd", "python", "pip"]


def test_extract_command_heads_skips_environment_assignments():
    assert extract_command_heads({"command": "PYTHONPATH=. python app.py"}) == ["python"]


def test_extract_command_heads_uses_the_basename_of_an_absolute_path():
    assert extract_command_heads({"command": "/usr/bin/python app.py"}) == ["python"]


def test_extract_command_heads_ignores_values_that_are_not_commands():
    assert extract_command_heads({"path": "src", "pattern": "*.py"}) == []


def test_find_forbidden_command_matches_the_invoked_binary():
    assert find_forbidden_command({"command": "python -m unittest"}, ["python"]) == "python"


def test_find_forbidden_command_allows_the_pinned_prefix():
    # `uv run python app.py` is the sanctioned way to run Python under a uv contract.
    assert find_forbidden_command({"command": "uv run python app.py"}, ["python"]) is None


def test_find_forbidden_command_matches_a_chained_forbidden_binary():
    assert find_forbidden_command({"command": "uv run pytest && pip install x"}, ["pip"]) == "pip"


def test_find_forbidden_command_normalizes_versioned_binaries():
    assert find_forbidden_command({"command": "python3.11 app.py"}, ["python"]) == "python3.11"


def test_find_forbidden_command_does_not_match_a_path_argument():
    # `cat src/python` invokes `cat`; `python` is an argument, not the command head.
    assert find_forbidden_command({"command": "cat src/python"}, ["python"]) is None


def test_find_forbidden_command_returns_none_without_a_forbidden_list():
    assert find_forbidden_command({"command": "python app.py"}, []) is None
