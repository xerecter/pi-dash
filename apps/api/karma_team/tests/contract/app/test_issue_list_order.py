from unittest.mock import patch

import pytest

from karma_team.db.models import Issue, ProjectMember, State

pytestmark = [pytest.mark.contract, pytest.mark.django_db]


@pytest.fixture
def ordered_project(project, create_user):
    ProjectMember.objects.get_or_create(project=project, member=create_user, defaults={"role": 20})
    state = State.objects.create(project=project, name="Backlog", group="backlog", default=True)
    items = {}
    for sequence in [1, 2, 3, 10, 11, 20]:
        item = Issue.objects.create(project=project, state=state, name=f"Item {sequence}", created_by=create_user)
        Issue.objects.filter(pk=item.pk).update(sequence_id=sequence)
        items[sequence] = item
    Issue.objects.filter(pk=items[1].pk).update(parent=items[10])
    Issue.objects.filter(pk=items[3].pk).update(parent=items[2])
    Issue.objects.filter(pk=items[11].pk).update(parent=items[1])
    with patch("karma_team.app.views.issue.base.recent_visited_task.delay"):
        yield project, items


def records(value):
    if isinstance(value, list):
        for child in value:
            yield from records(child)
    elif isinstance(value, dict):
        if "id" in value and "sequence_id" in value:
            yield value
        else:
            for child in value.values():
                yield from records(child)


def list_url(project):
    return f"/api/workspaces/{project.workspace.slug}/projects/{project.pk}/issues/"


@pytest.mark.parametrize(
    "grouping", [{}, {"group_by": "state_id"}, {"group_by": "state_id", "sub_group_by": "priority"}]
)
@pytest.mark.parametrize("previous_order", ["-created_at", "sort_order"])
def test_project_list_orders_before_pagination(session_client, ordered_project, grouping, previous_order):
    project, _ = ordered_project
    actual = []
    cursor = None
    for _ in range(3):
        params = {"layout": "list", "order_by": previous_order, "per_page": 2, **grouping}
        if cursor:
            params["cursor"] = cursor
        response = session_client.get(list_url(project), params)
        assert response.status_code == 200, response.content
        payload = response.json()
        actual.extend(item["sequence_id"] for item in records(payload["results"]))
        cursor = payload["next_cursor"]
    assert actual == [2, 10, 20, 1, 3, 11]
    assert len(set(actual)) == len(actual)


def test_filtered_children_still_sort_after_roots(session_client, ordered_project):
    project, items = ordered_project
    Issue.objects.filter(pk__in=[items[n].pk for n in [1, 20, 3]]).update(priority="high")
    response = session_client.get(
        list_url(project),
        {
            "layout": "list",
            "priority": "high",
        },
    )
    assert response.status_code == 200
    assert [item["sequence_id"] for item in records(response.json()["results"])] == [20, 1, 3]


def test_reparent_and_detach_change_list_position(session_client, ordered_project):
    project, items = ordered_project
    for parent, expected in [(items[10], [10, 20, 1, 2, 3, 11]), (None, [2, 10, 20, 1, 3, 11])]:
        Issue.objects.filter(pk=items[2].pk).update(parent=parent)
        response = session_client.get(list_url(project), {"layout": "list"})
        assert response.status_code == 200
        assert [item["sequence_id"] for item in records(response.json()["results"])] == expected


@pytest.mark.parametrize(
    "params",
    [
        {"order_by": "-created_at"},
        {"layout": "kanban", "order_by": "-created_at"},
        {"layout": "kanban", "order_by": "-created_at", "group_by": "state_id"},
        {"layout": "kanban", "order_by": "-created_at", "group_by": "state_id", "sub_group_by": "priority"},
    ],
)
def test_other_layouts_keep_requested_order(session_client, ordered_project, params):
    project, _ = ordered_project
    response = session_client.get(list_url(project), params)
    assert response.status_code == 200
    assert [item["sequence_id"] for item in records(response.json()["results"])] == [20, 11, 10, 3, 2, 1]
