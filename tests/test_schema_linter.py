from token_bleed_shield.config import ToolLinterConfig
from token_bleed_shield.tools.schema_linter import ToolSchemaLinter


def test_tool_schema_minification():
    config = ToolLinterConfig(strip_titles=True, max_description_length=60)
    linter = ToolSchemaLinter(config)

    raw_tool = {
        "name": "test_tool",
        "description": "This is an extremely detailed and verbose description that goes on and on and should definitely be truncated.",
        "parameters": {
            "type": "object",
            "title": "TestParametersTitleThatShouldBeRemoved",
            "properties": {
                "arg1": {
                    "type": "string",
                    "title": "Arg1Title",
                    "description": "A very long parameter description that exceeds sixty characters easily.",
                }
            },
        },
    }

    report = linter.lint_tools([raw_tool])
    assert report.total_tools == 1
    assert report.minified_tokens < report.original_tokens
    assert report.savings_percent > 0

    minified = report.minified_schema[0]
    # Check titles stripped
    assert "title" not in minified["parameters"]
    assert "title" not in minified["parameters"]["properties"]["arg1"]
    # Check truncated description
    assert minified["description"].endswith("...")
