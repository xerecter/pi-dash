# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only

from dataclasses import replace
from types import SimpleNamespace
from uuid import uuid4

import pytest
from django.core.cache import cache
from pydantic_ai import ModelRetry

from karma_team.assistant.models import AssistantMessage, AssistantThread, AssistantTurn, MessageKind
from karma_team.assistant.runtime.agent import assistant
from karma_team.assistant.tools import _scoping, issues, labels, projects, relations
from karma_team.db.models import Issue, IssueAssignee, IssueLabel, IssueRelation, Label, ProjectMember
from karma_team.tests.contract.assistant.conftest import fake_ctx, make_deps

pytestmark = pytest.mark.django_db


@pytest.fixture
def contexts(world):
    result = {}
    for name, role in (("admin", 20), ("member", 15), ("guest", 5), ("outsider", 15), ("other_user", 20)):
        user = getattr(world, name)
        thread = AssistantThread.objects.create(workspace=world.ws, user=user)
        turn = AssistantTurn.objects.create(thread=thread)
        result[name] = fake_ctx(make_deps(user, world.ws, role, thread_id=thread.id, turn_id=turn.id))
    return SimpleNamespace(**result)


@pytest.fixture
def project_labels(world):
    return [Label.objects.create(project=world.proj_a, workspace=world.ws, name=name) for name in ("Bug", "Feature")]


@pytest.fixture(autouse=True)
def prevent_external_side_effects(mocker):
    mocker.patch("karma_team.db.mixins.soft_delete_related_objects.delay")
    mocker.patch("karma_team.assistant.runtime.events.publish_event")
    mocker.patch("karma_team.runner.services.matcher.drain_pod_by_id")


def test_metadata_tools_registered_with_agent():
    names = set(assistant._function_toolset.tools)
    assert {
        "create_issue",
        "update_issue",
        "delete_issue",
        "get_issue",
        "update_issue_assignees",
        "update_issue_labels",
        "list_project_members",
        "list_labels",
        "get_label",
        "create_label",
        "update_label",
        "delete_label",
        "list_issue_relations",
        "create_issue_relation",
        "update_issue_relation",
        "delete_issue_relation",
    } <= names
    schema = assistant._function_toolset.tools["update_issue"].function_schema.json_schema
    assert {"git_work_branch", "assignee_ids", "label_ids"} <= set(schema["properties"])


@pytest.mark.django_db(transaction=True)
def test_agent_executes_registered_tools_end_to_end(world, contexts, project_labels):
    from pydantic_ai.messages import ModelResponse, TextPart, ToolCallPart, ToolReturnPart
    from pydantic_ai.models.function import FunctionModel

    issue_id = str(world.issue_a.id)
    calls = iter(
        [
            ("get_issue", {"issue_id": issue_id}),
            ("update_issue", {"issue_id": issue_id, "git_work_branch": "feature/agent-test"}),
            (
                "update_issue_assignees",
                {"issue_id": issue_id, "assignee_ids": [str(world.member.id)], "operation": "add"},
            ),
            (
                "update_issue_labels",
                {"issue_id": issue_id, "label_ids": [str(project_labels[0].id)], "operation": "add"},
            ),
            (
                "create_issue_relation",
                {"issue_id": issue_id, "related_issue_id": str(world.guest_issue.id), "relation_type": "blocked_by"},
            ),
        ]
    )

    def model_function(messages, info):
        call = next(calls, None)
        if call is None:
            return ModelResponse(parts=[TextPart("Updated the work item.")])
        name, args = call
        assert name in {tool.name for tool in info.function_tools}
        return ModelResponse(parts=[ToolCallPart(name, args)])

    result = assistant.run_sync("Update this work item", deps=contexts.member.deps, model=FunctionModel(model_function))
    assert result.output == "Updated the work item."
    returns = [part for message in result.all_messages() for part in message.parts if isinstance(part, ToolReturnPart)]
    assert len(returns) == 5
    detail = issues.get_issue(contexts.member, issue_id)
    assert detail["git_work_branch"] == "feature/agent-test"
    assert detail["assignee_ids"] == [str(world.member.id)]
    assert detail["label_ids"] == [str(project_labels[0].id)]
    assert detail["relations"][0]["relation_type"] == "blocked_by"


def test_create_and_read_all_metadata(world, contexts, project_labels):
    result = issues.create_issue(
        contexts.member,
        str(world.proj_a.id),
        "Assigned branch",
        git_work_branch="feature/ai-work",
        assignee_ids=[str(world.member.id)],
        label_ids=[str(project_labels[0].id)],
    )
    detail = issues.get_issue(contexts.member, result["id"])
    assert detail["git_work_branch"] == "feature/ai-work"
    assert detail["assignee_ids"] == [str(world.member.id)]
    assert detail["label_ids"] == [str(project_labels[0].id)]
    assert detail["relations"] == []
    assert detail["labels"][0]["name"] == "<untrusted>Bug</untrusted>"
    assert IssueAssignee.objects.get(issue_id=result["id"]).created_by_id == world.member.id
    assert IssueLabel.objects.get(issue_id=result["id"]).created_by_id == world.member.id
    found = issues.search_issues(contexts.member, "", project_id=str(world.proj_a.id))["results"]
    assert next(item for item in found if item["id"] == result["id"])["git_work_branch"] == "feature/ai-work"


def test_branch_update_preserve_and_clear(world, contexts):
    issue_id = str(world.issue_a.id)
    issues.update_issue(contexts.member, issue_id, git_work_branch="feature/one")
    issues.update_issue(contexts.member, issue_id, priority="high")
    assert issues.get_issue(contexts.member, issue_id)["git_work_branch"] == "feature/one"
    issues.update_issue(contexts.member, issue_id, git_work_branch=None, name="Renamed")
    assert issues.get_issue(contexts.member, issue_id)["git_work_branch"] == "feature/one"
    result = issues.update_issue(contexts.member, issue_id, git_work_branch="")
    assert result["git_work_branch"] == ""
    assert result["changed"] == ["git_work_branch"]


@pytest.mark.parametrize("branch", ["has a space", "branch;command", "a" * 129, "branch\n"])
def test_invalid_branch_rolls_back_all_fields(world, contexts, branch):
    with pytest.raises(ModelRetry):
        issues.update_issue(contexts.member, str(world.issue_a.id), name="Do not save", git_work_branch=branch)
    world.issue_a.refresh_from_db()
    assert world.issue_a.name == "Fix login"


def test_metadata_is_ready_before_create_dispatch(world, contexts, project_labels, mocker):
    def transition(issue, _from, _to, **kwargs):
        assert issue.git_work_branch == "feature/ready"
        assert list(issue.issue_assignee.values_list("assignee_id", flat=True)) == [world.member.id]
        assert list(issue.label_issue.values_list("label_id", flat=True)) == [project_labels[0].id]
        return SimpleNamespace(created_run=None)

    dispatch = mocker.patch("karma_team.orchestration.service.handle_issue_state_transition", side_effect=transition)
    issues.create_issue(
        contexts.member,
        str(world.proj_a.id),
        "Ready",
        git_work_branch="feature/ready",
        assignee_ids=[str(world.member.id)],
        label_ids=[str(project_labels[0].id)],
    )
    dispatch.assert_called_once()


def test_dispatch_failure_does_not_discard_created_issue(world, contexts, mocker):
    mocker.patch(
        "karma_team.orchestration.service.handle_issue_state_transition", side_effect=RuntimeError("unavailable")
    )
    result = issues.create_issue(
        contexts.member, str(world.proj_a.id), "Keep the work item", git_work_branch="feature/keep"
    )
    assert result["created"] is True
    assert "dispatch_error" in result
    assert Issue.objects.get(pk=result["id"]).git_work_branch == "feature/keep"


def test_update_metadata_preserves_other_links_and_audit(world, contexts, project_labels):
    issue_id = str(world.issue_a.id)
    issues.update_issue(
        contexts.member,
        issue_id,
        git_work_branch="feature/keep",
        assignee_ids=[str(world.member.id)],
        label_ids=[str(project_labels[0].id)],
    )
    link_id = IssueAssignee.objects.get(issue=world.issue_a).id
    issues.update_issue(contexts.member, issue_id, name="New name")
    assert IssueAssignee.objects.get(issue=world.issue_a).id == link_id
    result = issues.update_issue(contexts.member, issue_id, assignee_ids=[], label_ids=[])
    assert result["assignee_ids"] == result["label_ids"] == []
    assert result["git_work_branch"] == "feature/keep"
    world.issue_a.refresh_from_db()
    assert world.issue_a.updated_by_id == world.member.id
    assert (
        AssistantMessage.objects.filter(thread_id=contexts.member.deps.thread_id, kind=MessageKind.TOOL_RESULT).count()
        == 3
    )


def test_assignee_add_remove_set_and_deduplicate(world, contexts):
    ctx, issue_id = contexts.member, str(world.issue_a.id)
    member_id, admin_id = str(world.member.id), str(world.admin.id)
    issues.update_issue_assignees(ctx, issue_id, [member_id, member_id], "add")
    result = issues.update_issue_assignees(ctx, issue_id, [admin_id], "add")
    assert set(result["assignee_ids"]) == {member_id, admin_id}
    result = issues.update_issue_assignees(ctx, issue_id, [member_id], "remove")
    assert result["assignee_ids"] == [admin_id]
    assert issues.update_issue_assignees(ctx, issue_id, [], "set")["assignees"] == []
    assert IssueAssignee.objects.filter(issue=world.issue_a).count() == 0
    issues.update_issue_assignees(ctx, issue_id, [member_id], "add")
    assert IssueAssignee.objects.filter(issue=world.issue_a).count() == 1


def test_removed_assignment_disappears_from_my_issues(world, contexts):
    issue_id, member_id = str(world.issue_a.id), str(world.member.id)
    issues.update_issue_assignees(contexts.member, issue_id, [member_id], "add")
    assert issue_id in {item["id"] for item in issues.list_my_issues(contexts.member, "assigned")["results"]}
    issues.update_issue_assignees(contexts.member, issue_id, [member_id], "remove")
    for scope in ("assigned", "all"):
        assert issue_id not in {item["id"] for item in issues.list_my_issues(contexts.member, scope)["results"]}


@pytest.mark.parametrize("person", ["guest", "outsider", "other_user"])
def test_assignees_must_be_eligible_project_members(world, contexts, person):
    with pytest.raises(ModelRetry):
        issues.update_issue(contexts.member, str(world.issue_a.id), assignee_ids=[str(getattr(world, person).id)])
    assert not IssueAssignee.objects.filter(issue=world.issue_a).exists()


def test_inactive_member_not_assignable_but_can_be_removed(world, contexts):
    issue_id, user_id = str(world.issue_a.id), str(world.member.id)
    issues.update_issue_assignees(contexts.admin, issue_id, [user_id], "add")
    ProjectMember.objects.filter(project=world.proj_a, member=world.member).update(is_active=False)
    with pytest.raises(ModelRetry):
        issues.update_issue_assignees(contexts.admin, issue_id, [user_id], "add")
    assert issues.update_issue_assignees(contexts.admin, issue_id, [user_id], "remove")["assignee_ids"] == []


def test_project_members_expose_assignment_eligibility(world, contexts):
    members = {item["user_id"]: item for item in projects.list_project_members(contexts.member, str(world.proj_a.id))}
    assert members[str(world.member.id)]["can_assign"] is True
    assert members[str(world.member.id)]["email"] == world.member.email
    assert members[str(world.guest.id)]["can_assign"] is False


def test_label_attachment_add_remove_set(world, contexts, project_labels):
    issue_id = str(world.issue_a.id)
    first, second = (str(label.id) for label in project_labels)
    issues.update_issue_labels(contexts.member, issue_id, [first, first], "add")
    result = issues.update_issue_labels(contexts.member, issue_id, [second], "add")
    assert set(result["label_ids"]) == {first, second}
    assert IssueLabel.objects.filter(issue=world.issue_a).count() == 2
    assert issues.update_issue_labels(contexts.member, issue_id, [first], "remove")["label_ids"] == [second]
    assert issues.update_issue_labels(contexts.member, issue_id, [], "set")["label_ids"] == []
    assert Label.objects.filter(id__in=[first, second]).count() == 2


@pytest.mark.parametrize("foreign", ["same_workspace", "other_workspace"])
def test_label_attachment_rejects_foreign_project_and_workspace(world, contexts, foreign):
    label = Label.objects.create(
        name="Foreign",
        project=world.proj_b if foreign == "same_workspace" else None,
        workspace=world.ws if foreign == "same_workspace" else world.other_ws,
    )
    with pytest.raises(ModelRetry):
        issues.update_issue(contexts.member, str(world.issue_a.id), name="No change", label_ids=[str(label.id)])
    world.issue_a.refresh_from_db()
    assert world.issue_a.name == "Fix login"


@pytest.mark.parametrize("field", ["assignee_ids", "label_ids"])
def test_malformed_metadata_ids_return_tool_retry(world, contexts, field):
    with pytest.raises(ModelRetry):
        issues.update_issue(contexts.member, str(world.issue_a.id), **{field: ["not-a-uuid"]})


@pytest.mark.parametrize("person", ["guest", "outsider", "other_user"])
def test_metadata_write_permissions(world, contexts, person):
    ctx = getattr(contexts, person)
    for call in (
        lambda: issues.update_issue(ctx, str(world.issue_a.id), git_work_branch="feature/no"),
        lambda: issues.update_issue_assignees(ctx, str(world.issue_a.id), []),
        lambda: issues.update_issue_labels(ctx, str(world.issue_a.id), []),
    ):
        with pytest.raises(ModelRetry):
            call()


def test_project_label_crud_and_cache_invalidation(world, contexts):
    project_id = str(world.proj_a.id)
    key = f"/api/workspaces/{world.ws.slug}/labels/"
    cache.set(key, {"stale": True})
    created = labels.create_label(contexts.admin, project_id, "Bug", "#ff0000", "Defect")
    assert cache.get(key) is None
    label_id = created["id"]
    assert labels.get_label(contexts.member, project_id, label_id)["color"] == "#ff0000"
    updated = labels.update_label(contexts.admin, project_id, label_id, name="Defect", color="#00ff00")
    assert updated["description"] == "<untrusted>Defect</untrusted>"
    assert updated["name"] == "<untrusted>Defect</untrusted>"
    issues.update_issue_labels(contexts.member, str(world.issue_a.id), [label_id])
    assert labels.delete_label(contexts.admin, project_id, label_id)["deleted"] is True
    assert not Label.objects.filter(id=label_id).exists()
    assert Label.all_objects.get(id=label_id).deleted_at is not None
    assert issues.get_issue(contexts.member, str(world.issue_a.id))["labels"] == []
    assert projects.list_labels(contexts.member, project_id) == []


@pytest.mark.parametrize("person", ["member", "guest", "outsider", "other_user"])
def test_project_label_definitions_require_admin(world, contexts, project_labels, person):
    ctx, project_id = getattr(contexts, person), str(world.proj_a.id)
    for call in (
        lambda: labels.create_label(ctx, project_id, "Nope"),
        lambda: labels.update_label(ctx, project_id, str(project_labels[0].id), name="Nope"),
        lambda: labels.delete_label(ctx, project_id, str(project_labels[0].id)),
    ):
        with pytest.raises(ModelRetry):
            call()


def test_duplicate_label_names_are_case_insensitive(world, contexts, project_labels):
    with pytest.raises(ModelRetry):
        labels.create_label(contexts.admin, str(world.proj_a.id), "bug")
    with pytest.raises(ModelRetry):
        labels.update_label(contexts.admin, str(world.proj_a.id), str(project_labels[1].id), name="BUG")


def test_label_read_is_scoped(world, contexts, project_labels):
    for ctx, project_id in ((contexts.member, str(world.proj_b.id)), (contexts.other_user, str(world.proj_a.id))):
        with pytest.raises(_scoping.ToolNotFound):
            labels.get_label(ctx, project_id, str(project_labels[0].id))
    with pytest.raises(ModelRetry):
        labels.get_label(contexts.admin, "bad-project", str(project_labels[0].id))


@pytest.mark.parametrize(
    "kind,inverse",
    [
        ("blocked_by", "blocking"),
        ("blocking", "blocked_by"),
        ("relates_to", "relates_to"),
        ("duplicate", "duplicate"),
        ("start_before", "start_after"),
        ("start_after", "start_before"),
        ("finish_before", "finish_after"),
        ("finish_after", "finish_before"),
        ("implemented_by", "implements"),
        ("implements", "implemented_by"),
    ],
)
def test_relation_creation_and_inverse_query(world, contexts, kind, inverse):
    first, second = str(world.issue_a.id), str(world.guest_issue.id)
    result = relations.create_issue_relation(contexts.member, first, second, kind)
    assert result["relation_type"] == kind
    assert relations.list_issue_relations(contexts.member, second)[0]["relation_type"] == inverse
    assert relations.create_issue_relation(contexts.member, second, first, inverse)["created"] is False
    assert IssueRelation.objects.count() == 1
    assert issues.get_issue(contexts.member, first)["relations"][0]["id"] == result["id"]


def test_relation_update_and_delete_wake_blocked_issue(world, contexts, mocker):
    wake = mocker.patch("karma_team.assistant.tools.relations.schedule_unblocked_issue_wake")
    first, second = str(world.issue_a.id), str(world.guest_issue.id)
    created = relations.create_issue_relation(contexts.member, first, second, "blocked_by")
    updated = relations.update_issue_relation(contexts.member, first, created["id"], "relates_to")
    assert updated["relation_type"] == "relates_to"
    wake.assert_called_once_with(world.guest_issue, actor=world.member, blocked_issue_ids=[world.issue_a.id])
    relations.update_issue_relation(contexts.member, first, created["id"], "blocking")
    stored = IssueRelation.objects.get(id=created["id"])
    assert stored.issue_id == world.guest_issue.id
    assert stored.related_issue_id == world.issue_a.id
    wake.reset_mock()
    relations.delete_issue_relation(contexts.member, second, created["id"])
    wake.assert_called_once_with(world.issue_a, actor=world.member, blocked_issue_ids=[world.guest_issue.id])
    assert not IssueRelation.objects.filter(id=created["id"]).exists()
    assert Issue.objects.filter(id__in=[first, second]).count() == 2


def test_dependency_wake_occurs_after_commit(world, contexts, django_capture_on_commit_callbacks, mocker):
    wake = mocker.patch("karma_team.orchestration.signals._wake_unblocked_issue")
    first, second = str(world.issue_a.id), str(world.guest_issue.id)
    created = relations.create_issue_relation(contexts.member, first, second, "blocked_by")
    with django_capture_on_commit_callbacks(execute=True):
        relations.delete_issue_relation(contexts.member, first, created["id"])
        wake.assert_not_called()
    wake.assert_called_once()
    assert wake.call_args.args[0].pk == world.issue_a.pk


def test_relation_queries_do_not_leak_inaccessible_issues(world, contexts):
    relation = relations.create_issue_relation(
        contexts.admin, str(world.issue_a.id), str(world.issue_b.id), "relates_to"
    )
    assert relations.list_issue_relations(contexts.member, str(world.issue_a.id)) == []
    for call in (
        lambda: relations.create_issue_relation(
            contexts.member, str(world.issue_a.id), str(world.issue_b.id), "blocked_by"
        ),
        lambda: relations.update_issue_relation(contexts.member, str(world.issue_a.id), relation["id"], "duplicate"),
        lambda: relations.delete_issue_relation(contexts.member, str(world.issue_a.id), relation["id"]),
    ):
        with pytest.raises(_scoping.ToolNotFound):
            call()


def test_relation_rejects_self_invalid_type_and_conflict(world, contexts):
    first, second = str(world.issue_a.id), str(world.guest_issue.id)
    with pytest.raises(ModelRetry):
        relations.create_issue_relation(contexts.member, first, first, "relates_to")
    with pytest.raises(ModelRetry):
        relations.create_issue_relation(contexts.member, first, second, "unknown")
    relations.create_issue_relation(contexts.member, first, second, "relates_to")
    with pytest.raises(ModelRetry):
        relations.create_issue_relation(contexts.member, first, second, "blocked_by")


@pytest.mark.parametrize("person", ["guest", "outsider", "other_user"])
def test_relation_write_permissions(world, contexts, person):
    first, second = str(world.issue_a.id), str(world.guest_issue.id)
    relation = relations.create_issue_relation(contexts.admin, first, second, "relates_to")
    ctx = getattr(contexts, person)
    for call in (
        lambda: relations.create_issue_relation(ctx, first, second, "duplicate"),
        lambda: relations.update_issue_relation(ctx, first, relation["id"], "duplicate"),
        lambda: relations.delete_issue_relation(ctx, first, relation["id"]),
    ):
        with pytest.raises(ModelRetry):
            call()


def test_missing_relation_is_model_retry(world, contexts):
    for relation_id in (str(uuid4()), "not-a-uuid"):
        with pytest.raises(ModelRetry):
            relations.delete_issue_relation(contexts.member, str(world.issue_a.id), relation_id)


def test_issue_delete_creator_admin_permissions_and_soft_delete(world, contexts):
    with pytest.raises(_scoping.ToolPermissionError):
        issues.delete_issue(contexts.member, str(world.issue_a.id))
    own = issues.create_issue(contexts.member, str(world.proj_a.id), "Disposable")
    assert issues.delete_issue(contexts.member, own["id"])["deleted"] is True
    assert not Issue.objects.filter(id=own["id"]).exists()
    assert Issue.all_objects.get(id=own["id"]).deleted_at is not None
    assert issues.delete_issue(contexts.admin, str(world.guest_issue.id))["deleted"] is True


@pytest.mark.parametrize("model", ["GitIssueSync", "GithubIssueSync"])
def test_synced_issue_cannot_be_deleted(world, contexts, mocker, model):
    mocker.patch(f"karma_team.db.models.{model}.objects.filter").return_value.exists.return_value = True
    with pytest.raises(ModelRetry, match="synced"):
        issues.delete_issue(contexts.admin, str(world.issue_a.id))
    assert Issue.objects.filter(pk=world.issue_a.id).exists()


def test_unattended_mode_cannot_delete(world, contexts, project_labels):
    ctx = fake_ctx(replace(contexts.admin.deps, mode="loop"))
    for call in (
        lambda: issues.delete_issue(ctx, str(world.issue_a.id)),
        lambda: labels.delete_label(ctx, str(world.proj_a.id), str(project_labels[0].id)),
        lambda: relations.delete_issue_relation(ctx, str(world.issue_a.id), str(uuid4())),
    ):
        with pytest.raises(_scoping.ToolPermissionError):
            call()
