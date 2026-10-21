# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Dependency gates for issue agent dispatch.

``IssueRelation(issue=B, related_issue=A, relation_type="blocked_by")``
means B must wait for A. A blocker releases its dependants once it reaches
test or a terminal state, or when it is soft-deleted.
"""

from django.db.models import Q, QuerySet

from karma_team.db.models.issue import Issue, IssueRelation, IssueRelationChoices
from karma_team.db.models.state import StateGroup


BLOCKED_BY_DEPENDENCY_REASON = "blocked-by-dependency"
RELEASING_GROUPS = frozenset(
    {
        StateGroup.TEST.value,
        StateGroup.COMPLETED.value,
        StateGroup.CANCELLED.value,
    }
)


def unresolved_blockers(issue: Issue) -> QuerySet[IssueRelation]:
    """Return active ``blocked_by`` relations whose blocker has not released."""

    return (
        IssueRelation.objects.filter(
            issue=issue,
            relation_type=IssueRelationChoices.BLOCKED_BY,
        )
        .select_related("related_issue__state")
        .exclude(
            Q(related_issue__deleted_at__isnull=False)
            | Q(related_issue__state__group__in=RELEASING_GROUPS)
        )
    )


def is_blocked(issue: Issue) -> bool:
    """Whether ``issue`` still has at least one unresolved blocker."""

    return unresolved_blockers(issue).exists()


def issues_unblocked_by(issue: Issue) -> QuerySet[Issue]:
    """Return live issues that declare ``issue`` as a blocker."""

    blocked_issue_ids = IssueRelation.objects.filter(
        related_issue=issue,
        relation_type=IssueRelationChoices.BLOCKED_BY,
    ).values_list("issue_id", flat=True)
    return Issue.objects.filter(pk__in=blocked_issue_ids).select_related("state").distinct()
