# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from pathlib import Path

import pytest

from karma_team.prompting.composer import compose
from karma_team.prompting.validation import sample_contexts


@pytest.mark.unit
@pytest.mark.parametrize("kind", ["coding-task", "review", "test", "scheduler"])
def test_agent_prompts_keep_comment_drafts_outside_git_and_clean_after_success(kind):
    context = sample_contexts(kind)[0]
    text = compose(kind, workspace=None, project=None, user=None, context=context).text
    assert 'mktemp "${TMPDIR:-/tmp}/karma-team-comment.XXXXXX"' in text
    assert '--body-file "$COMMENT_FILE" --remove-body-file' in text
    assert "cleanup is opt-in" in text
    assert "Failed uploads and files changed during the upload are retained" in text
    assert "On an older CLI" in text
    assert "--body-file ./.karma-team-blocked.md" not in text
    if kind != "scheduler":
        assert "### Final workspace check" in text
        assert "After uploading the final workpad and comments" in text


@pytest.mark.unit
def test_legacy_fragments_include_comment_cleanup_guidance():
    root = Path(__file__).resolve().parents[3] / "prompting" / "fragments"
    cli = (root / "04_karma_team_cli.md").read_text()
    ending = (root / "14_ending_run.md").read_text()
    assert "--remove-body-file" in cli
    assert "--body-file ./.karma-team-blocked.md" not in cli
    assert "### Final workspace check" in ending
