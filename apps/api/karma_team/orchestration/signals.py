# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Issue state-transition hook.

We intercept Issue saves with a pre_save snapshot of the prior ``state_id`` and
compare in post_save. Using signals keeps the trigger out of every writer path
(REST views, admin, importers) while still routing every transition through
``orchestration.service``.

Comments are intentionally inert — there is no ``post_save(IssueComment)``
receiver here. The agent is woken by the per-issue ticker
(``karma_team.bgtasks.agent_ticker``) or by the explicit Comment & Run
button, which calls into ``orchestration.service.handle_issue_comment``
directly. See ``.ai_design/issue_ticking_system/design.md`` §9.
"""

from __future__ import annotations

import logging

from django.db import transaction
from django.db.models.signals import post_save, pre_save
from django.dispatch import receiver

from karma_team.db.models.issue import Issue
from karma_team.db.models.state import State, StateGroup
from karma_team.orchestration.blocking import RELEASING_GROUPS, is_blocked, issues_unblocked_by
from karma_team.orchestration.service import (
    _active_run_for,
    _dispatch_entry_run,
    handle_issue_state_transition,
)

logger = logging.getLogger(__name__)

_PREVIOUS_STATE = "_orchestration_prev_state_id"
_PREVIOUS_DELETED_AT = "_orchestration_prev_deleted_at"

#: Per-instance opt-out flag. A view that wants to drive a state
#: transition without the signal-side immediate dispatch (e.g. Comment &
#: Run on a Paused issue) sets ``issue._orchestration_dispatch_immediate
#: = False`` before calling ``Issue.save()``.
_DISPATCH_IMMEDIATE_ATTR = "_orchestration_dispatch_immediate"

#: Per-instance carrier for "this state change was made from inside an
#: agent run". The work-item PATCH view sets
#: ``issue._orchestration_moved_by_run = <AgentRun>`` (verified from the
#: ``X-karma-team-Run-Id`` header) before ``Issue.save()``; the handler uses it
#: to tell an agent's move from a human's — account identity cannot, because
#: the agent's CLI token resolves to the runner owner's account. See
#: ``.ai_design/ticking_relevance/design.md`` §5.6.
MOVED_BY_RUN_ATTR = "_orchestration_moved_by_run"

#: Process-local counter of swallowed orchestration errors. We catch and log
#: every exception in `fire_state_transition` so a broken trigger can't crash
#: the user's Issue save — but silent failures are worse than loud 500s, so we
#: also bump this counter. Ops dashboards and tests can assert on it.
orchestration_error_count = 0


@receiver(pre_save, sender=Issue, dispatch_uid="orchestration.issue_presave")
def capture_prior_state(sender, instance: Issue, **kwargs) -> None:
    if not instance.pk:
        setattr(instance, _PREVIOUS_STATE, None)
        setattr(instance, _PREVIOUS_DELETED_AT, None)
        return
    try:
        prior = Issue.all_objects.only("state_id", "deleted_at").get(pk=instance.pk)
    except Issue.DoesNotExist:
        setattr(instance, _PREVIOUS_STATE, None)
        setattr(instance, _PREVIOUS_DELETED_AT, None)
        return
    setattr(instance, _PREVIOUS_STATE, prior.state_id)
    setattr(instance, _PREVIOUS_DELETED_AT, prior.deleted_at)


@receiver(post_save, sender=Issue, dispatch_uid="orchestration.issue_postsave")
def fire_state_transition(sender, instance: Issue, created: bool, **kwargs) -> None:
    prev_state_id = getattr(instance, _PREVIOUS_STATE, None)
    current_state_id = instance.state_id
    previous_deleted_at = getattr(instance, _PREVIOUS_DELETED_AT, None)
    became_soft_deleted = not created and previous_deleted_at is None and instance.deleted_at is not None
    state_changed = prev_state_id != current_state_id
    if not state_changed and not became_soft_deleted:
        return

    to_state = instance.state if current_state_id else None
    if state_changed:
        from_state = _lookup_state(prev_state_id)
        dispatch_immediate = bool(getattr(instance, _DISPATCH_IMMEDIATE_ATTR, True))
        moved_by_run = getattr(instance, MOVED_BY_RUN_ATTR, None)

        try:
            handle_issue_state_transition(
                issue=instance,
                from_state=from_state,
                to_state=to_state,
                actor=None,
                dispatch_immediate=dispatch_immediate,
                moved_by_run=moved_by_run,
            )
        except Exception:  # noqa: BLE001 — never let orchestration crash issue save
            _record_orchestration_error(
                "handle_issue_state_transition failed for "
                f"issue={instance.pk} from_state={prev_state_id} to_state={current_state_id}"
            )

    if became_soft_deleted or (to_state is not None and to_state.group in RELEASING_GROUPS):
        schedule_unblocked_issue_wake(instance, using=kwargs.get("using"))


def schedule_unblocked_issue_wake(
    blocker: Issue,
    *,
    actor=None,
    blocked_issue_ids=None,
    using=None,
) -> None:
    """After commit, start eligible In Progress issues released by ``blocker``."""

    blocker_id = blocker.pk
    explicit_issue_ids = tuple(blocked_issue_ids) if blocked_issue_ids is not None else None

    def wake_after_commit() -> None:
        try:
            blocker_manager = Issue.all_objects
            if using is not None:
                blocker_manager = blocker_manager.using(using)
            committed_blocker = blocker_manager.select_related("state").get(pk=blocker_id)
            if explicit_issue_ids is None:
                blocked_issues = list(issues_unblocked_by(committed_blocker))
            else:
                issue_manager = Issue.objects
                if using is not None:
                    issue_manager = issue_manager.using(using)
                blocked_issues = list(
                    issue_manager.filter(pk__in=explicit_issue_ids).select_related("state")
                )
        except Exception:  # noqa: BLE001 — dependency wake must not break the writer
            _record_orchestration_error(f"dependency lookup failed for blocker={blocker_id}")
            return

        for blocked_issue in blocked_issues:
            try:
                _wake_unblocked_issue(blocked_issue, actor=actor)
            except Exception:  # noqa: BLE001 — keep waking the remaining dependants
                _record_orchestration_error(
                    f"dependency wake failed for blocker={blocker_id} issue={blocked_issue.pk}"
                )

    transaction.on_commit(wake_after_commit, using=using)


def _wake_unblocked_issue(issue: Issue, *, actor=None) -> None:
    from karma_team.orchestration import scheduling

    if issue.state is None or issue.state.group != StateGroup.STARTED.value:
        return
    if is_blocked(issue) or _active_run_for(issue) is not None:
        return

    decision = scheduling.reconcile(
        issue,
        scheduling.TickerEvent.entered_bucket(want_run=True, actor=actor),
    )
    if decision.dispatch_now:
        _dispatch_entry_run(issue, issue.state, actor)


def _record_orchestration_error(message: str) -> None:
    global orchestration_error_count

    orchestration_error_count += 1
    logger.exception(
        "orchestration.error: %s (total_errors=%d)",
        message,
        orchestration_error_count,
    )


def _lookup_state(state_id) -> State | None:
    if state_id is None:
        return None
    try:
        return State.all_state_objects.get(pk=state_id)
    except State.DoesNotExist:
        return None
