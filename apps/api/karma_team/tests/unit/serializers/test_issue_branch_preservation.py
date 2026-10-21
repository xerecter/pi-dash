"""Unrelated work-item writes must not restore a stale work branch."""

from types import SimpleNamespace
from unittest.mock import patch

import pytest
from django.utils import timezone

from karma_team.api.serializers.issue import IssueSerializer as APIIssueSerializer
from karma_team.api.serializers.intake import IntakeIssueUpdateSerializer as APIIntakeIssueSerializer
from karma_team.app.serializers.issue import IssueCreateSerializer
from karma_team.app.serializers.intake import IntakeIssueSerializer
from karma_team.app.views.issue.archive import IssueArchiveViewSet
from karma_team.db.models import Intake, IntakeIssue, Issue, State
from karma_team.space.serializer.issue import IssueCreateSerializer as SpaceIssueSerializer


pytestmark = [pytest.mark.unit, pytest.mark.django_db]


@pytest.fixture
def issue(project):
    state = State.objects.create(project=project, name="Backlog", group="backlog", default=True)
    return Issue.objects.create(project=project, name="Original title", state=state)


@pytest.fixture(params=[IssueCreateSerializer, APIIssueSerializer, SpaceIssueSerializer], ids=["app", "api", "space"])
def serializer_class(request):
    return request.param


def update_issue(serializer_class, issue, data):
    serializer = serializer_class(
        issue, data=data, partial=True,
        context={"project_id": str(issue.project_id), "workspace_id": str(issue.workspace_id)},
    )
    serializer.is_valid(raise_exception=True)
    return serializer.save()


@pytest.mark.parametrize("data", [
    {"name": "Edited title"},
    {"description_html": "<p>Edited description</p>"},
    {"priority": "high"},
    {"target_date": "2030-01-01"},
    {"sort_order": 1234.0},
    {"complexity_score": 3},
])
def test_unrelated_patch_preserves_concurrently_saved_branch(issue, serializer_class, data):
    # The PATCH loaded an empty branch before another request saved the branch.
    Issue.objects.filter(pk=issue.pk).update(git_work_branch="feat/pinned")
    saved = update_issue(serializer_class, issue, data)
    issue.refresh_from_db()
    assert issue.git_work_branch == "feat/pinned"
    assert saved.git_work_branch == "feat/pinned"
    assert str(getattr(issue, next(iter(data)))) == str(next(iter(data.values())))


def test_related_only_patch_preserves_branch(issue, serializer_class):
    Issue.objects.filter(pk=issue.pk).update(git_work_branch="feat/pinned")
    field = "label_ids" if serializer_class is IssueCreateSerializer else "labels"
    update_issue(serializer_class, issue, {field: []})
    issue.refresh_from_db()
    assert issue.git_work_branch == "feat/pinned"


def test_explicit_branch_clear_is_still_allowed(issue, serializer_class):
    Issue.objects.filter(pk=issue.pk).update(git_work_branch="feat/pinned")
    update_issue(serializer_class, issue, {"git_work_branch": ""})
    issue.refresh_from_db()
    assert issue.git_work_branch == ""


def test_branch_patch_does_not_restore_other_stale_fields(issue, serializer_class):
    Issue.objects.filter(pk=issue.pk).update(name="Concurrent title", priority="high")
    update_issue(serializer_class, issue, {"git_work_branch": "feat/pinned"})
    issue.refresh_from_db()
    assert (issue.name, issue.priority, issue.git_work_branch) == ("Concurrent title", "high", "feat/pinned")


def test_state_transition_uses_current_branch_and_preserves_dispatch_metadata(issue, serializer_class):
    target = State.objects.create(project=issue.project, name="In Progress", group="started")
    Issue.objects.filter(pk=issue.pk).update(git_work_branch="feat/pinned")
    issue._orchestration_dispatch_immediate = False
    moved_by_run = object()
    issue._orchestration_moved_by_run = moved_by_run
    state_field = "state_id" if serializer_class is IssueCreateSerializer else "state"
    with patch("karma_team.orchestration.signals.handle_issue_state_transition") as transition:
        update_issue(serializer_class, issue, {state_field: str(target.pk)})
    transition.assert_called_once()
    assert transition.call_args.kwargs["issue"].git_work_branch == "feat/pinned"
    assert transition.call_args.kwargs["dispatch_immediate"] is False
    assert transition.call_args.kwargs["moved_by_run"] is moved_by_run
    issue.refresh_from_db()
    assert issue.state_id == target.pk
    assert issue.git_work_branch == "feat/pinned"


@pytest.mark.parametrize("action", ["archive", "unarchive"])
def test_archive_actions_preserve_concurrently_saved_branch(issue, create_user, action):
    completed = State.objects.create(project=issue.project, name="Done", group="completed")
    issue.state = completed
    issue.archived_at = timezone.now().date() if action == "unarchive" else None
    issue.save()
    Issue.objects.filter(pk=issue.pk).update(git_work_branch="feat/pinned")
    request = SimpleNamespace(user=create_user)
    with (
        patch("karma_team.app.views.issue.archive.Issue.issue_objects.get", return_value=issue),
        patch("karma_team.app.views.issue.archive.Issue.objects.get", return_value=issue),
        patch("karma_team.app.views.issue.archive.issue_activity.delay"),
        patch("karma_team.app.views.issue.archive.base_host", return_value="http://localhost"),
    ):
        response = getattr(IssueArchiveViewSet, action).__wrapped__(
            IssueArchiveViewSet(), request, issue.workspace.slug, str(issue.project_id), pk=str(issue.pk),
        )
    assert response.status_code in {200, 204}
    issue.refresh_from_db()
    assert issue.git_work_branch == "feat/pinned"
    assert (issue.archived_at is not None) == (action == "archive")


@pytest.mark.parametrize("intake_serializer", [IntakeIssueSerializer, APIIntakeIssueSerializer])
def test_accept_intake_preserves_concurrently_saved_branch(issue, intake_serializer):
    default_state = issue.state
    triage = State.objects.create(project=issue.project, name="Triage", group="triage", is_triage=True)
    issue.state = triage
    issue.save()
    intake = Intake.objects.create(project=issue.project, name="Intake")
    entry = IntakeIssue.objects.create(project=issue.project, intake=intake, issue=issue)
    Issue.objects.filter(pk=issue.pk).update(git_work_branch="feat/pinned")
    serializer = intake_serializer(entry, data={"status": 1}, partial=True)
    serializer.is_valid(raise_exception=True)
    serializer.save()
    issue.refresh_from_db()
    assert issue.state_id == default_state.pk
    assert issue.git_work_branch == "feat/pinned"


def test_soft_delete_keeps_branch_for_restoration(issue):
    Issue.objects.filter(pk=issue.pk).update(git_work_branch="feat/pinned")
    issue.delete()
    deleted = Issue.all_objects.get(pk=issue.pk)
    assert deleted.deleted_at is not None
    assert deleted.git_work_branch == "feat/pinned"
