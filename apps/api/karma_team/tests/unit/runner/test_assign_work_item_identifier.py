# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""The ``assign`` envelope carries the work item's ``PROJ-123`` key.

The runner exports it to the agent as ``KARMA_TEAM_ISSUE_IDENTIFIER``; the prompt
tells the agent that ``karma-team state list`` / ``karma-team workpad`` default to it.
Without it those calls fail with "requires ... KARMA_TEAM_ISSUE_IDENTIFIER".
"""

from __future__ import annotations

import pytest
from crum import impersonate

from karma_team.db.models import Issue, State
from karma_team.runner.models import AgentRun, AgentRunStatus
from karma_team.runner.services.matcher import _build_assign_msg


@pytest.fixture
def issue(workspace, project, create_user):
    with impersonate(create_user):
        state = State.objects.create(name="In Progress", project=project, group="started")
        return Issue.objects.create(
            name="Logistics portal",
            workspace=workspace,
            project=project,
            state=state,
            created_by=create_user,
        )


def _run(workspace, owner, work_item=None):
    return AgentRun.objects.create(
        owner=owner,
        workspace=workspace,
        work_item=work_item,
        prompt="x",
        status=AgentRunStatus.QUEUED,
    )


@pytest.mark.unit
def test_assign_msg_carries_work_item_identifier(db, workspace, create_user, project, issue):
    msg = _build_assign_msg(_run(workspace, create_user, work_item=issue))
    assert msg["work_item_identifier"] == f"{project.identifier}-{issue.sequence_id}"
    assert msg["project_name"] == project.name
    assert msg["work_item_id"] == str(issue.id)


@pytest.mark.unit
def test_assign_msg_without_work_item_has_no_identifier(db, workspace, create_user):
    msg = _build_assign_msg(_run(workspace, create_user))
    assert msg["work_item_identifier"] is None
    assert msg["project_name"] is None
    assert msg["work_item_id"] is None
