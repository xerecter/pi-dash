# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""The web edit form must receive the saved branch through every issue read path."""

from unittest.mock import patch

import pytest

from karma_team.db.models import Issue, ProjectMember, State


pytestmark = [pytest.mark.contract, pytest.mark.django_db]


@pytest.fixture
def issue(project, create_user):
    ProjectMember.objects.get_or_create(project=project, member=create_user, defaults={"role": 20})
    state = State.objects.create(project=project, name="Backlog", group="backlog", default=True)
    return Issue.objects.create(
        project=project,
        state=state,
        name="Saved branch",
        git_work_branch="feat/pinned",
        created_by=create_user,
    )


@pytest.fixture(autouse=True)
def background_tasks():
    with (
        patch("karma_team.app.views.issue.base.recent_visited_task.delay"),
        patch("karma_team.app.views.issue.base.issue_activity.delay"),
        patch("karma_team.app.views.issue.base.model_activity.delay"),
        patch("karma_team.app.views.issue.base.issue_description_version_task.delay"),
    ):
        yield


def project_url(issue):
    return f"/api/workspaces/{issue.workspace.slug}/projects/{issue.project_id}"


def issue_records(value):
    if isinstance(value, list):
        for item in value:
            yield from issue_records(item)
    elif isinstance(value, dict):
        if "id" in value and "sequence_id" in value:
            yield value
        else:
            for item in value.values():
                yield from issue_records(item)


@pytest.mark.parametrize("branch", ["feat/pinned", ""])
@pytest.mark.parametrize("expand", ["", "issue_reactions,issue_attachments,issue_link,parent"])
def test_edit_details_return_saved_branch(session_client, issue, branch, expand):
    Issue.objects.filter(pk=issue.pk).update(git_work_branch=branch)
    response = session_client.get(f"{project_url(issue)}/issues/{issue.id}/", {"expand": expand})
    assert response.status_code == 200
    assert response.json()["git_work_branch"] == branch
    issue.refresh_from_db()
    assert issue.git_work_branch == branch


@pytest.mark.parametrize(
    "path,params",
    [
        ("issues/", {}),
        ("issues/", {"group_by": "state_id"}),
        ("issues/", {"group_by": "state_id", "sub_group_by": "priority"}),
        ("v2/issues/", {}),
        ("v2/issues/", {"description": "true"}),
        ("issues/list/", {}),
        ("issues/list/", {"expand": "state"}),
        ("issues-detail/", {}),
    ],
)
def test_issue_lists_return_saved_branch(session_client, issue, path, params):
    response = session_client.get(f"{project_url(issue)}/{path}", {"issues": str(issue.id), **params})
    assert response.status_code == 200
    record = next(item for item in issue_records(response.json()) if item["id"] == str(issue.id))
    assert record["git_work_branch"] == "feat/pinned"


@pytest.mark.parametrize(
    "change,expected",
    [
        ({"name": "Renamed"}, "feat/pinned"),
        ({"git_work_branch": "feat/updated"}, "feat/updated"),
        ({"git_work_branch": ""}, ""),
    ],
)
def test_edit_round_trip_preserves_or_explicitly_changes_branch(session_client, issue, change, expected):
    url = f"{project_url(issue)}/issues/{issue.id}/"
    response = session_client.patch(url, change, format="json")
    assert response.status_code == 204
    detail = session_client.get(url)
    assert detail.status_code == 200
    assert detail.json()["git_work_branch"] == expected
    issue.refresh_from_db()
    assert issue.git_work_branch == expected
