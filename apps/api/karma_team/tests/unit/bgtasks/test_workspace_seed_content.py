"""Regression tests for the guided work items copied into new workspaces."""

from __future__ import annotations

import json
from pathlib import Path


SEED_ISSUES = (
    Path(__file__).resolve().parents[3] / "seeds" / "data" / "issues.json"
)


def test_runner_setup_work_item_uses_the_self_hosted_cli_installer():
    issues = json.loads(SEED_ISSUES.read_text(encoding="utf-8"))
    issue = next(issue for issue in issues if issue["id"] == 3)
    body = issue["description_html"]

    assert "2. Install karma-team & register a runner" in issue["name"]
    assert "http://172.16.13.81:28000/api/cli/install.sh" in body
    assert "http://172.16.13.81:28000/api/cli/install.ps1" in body
    assert "curl --proto '=http,https' --tlsv1.2 -fsSL 'http://172.16.13.81:28000/api/cli/install.sh' | sh" in body
    assert "irm 'http://172.16.13.81:28000/api/cli/install.ps1' | iex" in body
    assert "Karma agent" in body
    assert "karma --version" in body
    assert "github.com/xerecter/karma-team/blob/main/docs/wiki/02-cloud-quickstart.md" not in body
    assert "karma-team auth login --url http://172.16.13.81:28000" in body
