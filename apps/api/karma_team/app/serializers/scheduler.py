# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Serializers for the project-scheduler API surface.

See ``.ai_design/project_scheduler/design.md`` §7 and
``.ai_design/project_scheduler_calendar/decisions.md`` §1-2 for the
iCal-shaped recurrence model that replaced cron in migration 0140.
"""

from __future__ import annotations

import re
from datetime import datetime
from typing import Iterable
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from rest_framework import serializers

from karma_team.app.serializers.base import BaseSerializer
from karma_team.bgtasks._rrule import RRuleValidationError, validate_rrule_string
from karma_team.db.models.scheduler import Scheduler, SchedulerBinding
from karma_team.runner.models import Pod
from karma_team.runner.services.permissions import (
    can_view_scheduler_run,
    scheduler_run_visible_to_user_q,
)


EXTRA_CONTEXT_MAX_LENGTH = 16 * 1024

# Cap the JSON-list extras stored on a binding — RDATE/EXDATE are
# expanded on every fire and every occurrences-endpoint request. A
# pathological 10k-entry list would slow each tick.
RDATE_EXDATE_MAX_LENGTH = 256

_HEX_COLOR_RE = re.compile(r"^#[0-9a-fA-F]{6}$")


def _validate_color(value: str) -> str:
    value = (value or "").strip().lower()
    if not _HEX_COLOR_RE.match(value):
        raise serializers.ValidationError("color must be a 7-character hex string like '#3b82f6'")
    return value


def _validate_iso_datetime_list(values: Iterable, field: str) -> list[str]:
    """Validate an rdates/exdates JSON list: each item must be an ISO datetime string."""
    if values is None:
        return []
    if not isinstance(values, list):
        raise serializers.ValidationError({field: "must be a JSON array of ISO 8601 datetime strings"})
    if len(values) > RDATE_EXDATE_MAX_LENGTH:
        raise serializers.ValidationError({field: f"must contain at most {RDATE_EXDATE_MAX_LENGTH} entries"})
    out: list[str] = []
    for i, raw in enumerate(values):
        if not isinstance(raw, str):
            raise serializers.ValidationError({field: f"item {i} must be a string, got {type(raw).__name__}"})
        try:
            parsed = datetime.fromisoformat(raw.replace("Z", "+00:00"))
        except ValueError as e:
            raise serializers.ValidationError({field: f"item {i} is not a valid ISO 8601 datetime: {e}"})
        out.append(parsed.isoformat())
    return out


class SchedulerSerializer(BaseSerializer):
    active_binding_count = serializers.SerializerMethodField()

    class Meta:
        model = Scheduler
        fields = [
            "id",
            "workspace",
            "slug",
            "name",
            "description",
            "prompt",
            "color",
            "source",
            "is_enabled",
            "active_binding_count",
            "created_at",
            "updated_at",
        ]
        read_only_fields = [
            "id",
            "workspace",
            "source",
            "active_binding_count",
            "created_at",
            "updated_at",
        ]

    def get_active_binding_count(self, obj: Scheduler) -> int:
        # When called over a queryset where the field is annotated, use that;
        # otherwise fall back to a count query. The list view annotates.
        annotated = getattr(obj, "_active_binding_count", None)
        if annotated is not None:
            return annotated
        return obj.bindings.filter(deleted_at__isnull=True).count()

    def validate_color(self, value: str) -> str:
        return _validate_color(value)

    def validate_slug(self, value: str) -> str:
        """Return duplicate active slugs as a field error.

        Django REST Framework does not automatically validate the model's
        conditional unique constraint.  Checking it here gives the create UI
        a recoverable ``{"slug": [...]}`` response instead of surfacing the
        database ``IntegrityError`` as a generic request failure.  The default
        manager excludes soft-deleted schedulers, matching the constraint.
        """
        workspace = self.instance.workspace if self.instance is not None else self.context.get("workspace")
        if workspace is not None:
            existing = Scheduler.objects.filter(workspace=workspace, slug=value)
            if self.instance is not None:
                existing = existing.exclude(pk=self.instance.pk)
            if existing.exists():
                raise serializers.ValidationError("This slug is already in use in this workspace.")
        return value


class SchedulerBindingSerializer(BaseSerializer):
    scheduler_slug = serializers.CharField(source="scheduler.slug", read_only=True)
    scheduler_name = serializers.CharField(source="scheduler.name", read_only=True)
    scheduler_color = serializers.CharField(source="scheduler.color", read_only=True)
    last_run = serializers.SerializerMethodField()
    last_run_status = serializers.SerializerMethodField()
    last_run_ended_at = serializers.SerializerMethodField()
    last_error = serializers.SerializerMethodField()
    # Optional pod override. Scoped to active pods; the cross-project check
    # lives in `validate()` (the project is known there via instance/context).
    # NULL = use the project's default pod at fire time.
    pod = serializers.PrimaryKeyRelatedField(
        queryset=Pod.objects.filter(deleted_at__isnull=True),
        required=False,
        allow_null=True,
    )
    # Joined pod name so the calendar / list can render the chosen pod without
    # a second fetch. Null when the binding uses the project default.
    pod_name = serializers.CharField(source="pod.name", read_only=True, default=None)

    class Meta:
        model = SchedulerBinding
        fields = [
            "id",
            "scheduler",
            "scheduler_slug",
            "scheduler_name",
            "scheduler_color",
            "project",
            "workspace",
            "dtstart",
            "tzid",
            "rrule",
            "rdates",
            "exdates",
            "extra_context",
            "enabled",
            "outcome_mode",
            "pod",
            "pod_name",
            "next_run_at",
            "last_run",
            "last_run_status",
            "last_run_ended_at",
            "last_error",
            "actor",
            "created_at",
            "updated_at",
        ]
        read_only_fields = [
            "id",
            "workspace",
            "pod_name",
            "next_run_at",
            "last_run",
            "last_run_status",
            "last_run_ended_at",
            "last_error",
            "actor",
            "created_at",
            "updated_at",
        ]

    def _last_run_is_visible(self, obj: SchedulerBinding) -> bool:
        cache = getattr(self, "_last_run_visibility_cache", None)
        if cache is None:
            cache = {}
            self._last_run_visibility_cache = cache
        if obj.pk not in cache:
            request = self.context.get("request")
            user = getattr(request, "user", None) if request is not None else None
            cache[obj.pk] = obj.last_run_id is None or can_view_scheduler_run(user, obj.last_run)
        return cache[obj.pk]

    def get_last_run(self, obj: SchedulerBinding):
        if not obj.last_run_id or not self._last_run_is_visible(obj):
            return None
        return str(obj.last_run_id)

    def get_last_run_status(self, obj: SchedulerBinding):
        if not obj.last_run_id or not self._last_run_is_visible(obj):
            return None
        return obj.last_run.status

    def get_last_run_ended_at(self, obj: SchedulerBinding):
        if not obj.last_run_id or not self._last_run_is_visible(obj):
            return None
        return obj.last_run.ended_at

    def get_last_error(self, obj: SchedulerBinding) -> str:
        # A short-circuit error with no run (for example, no default pod) is a
        # project-level scheduler error. If a last run exists, however, the
        # error is run-derived and must follow that run's private-runner gate.
        if obj.last_run_id and not self._last_run_is_visible(obj):
            return ""
        return obj.last_error

    def validate_rrule(self, value: str) -> str:
        value = (value or "").strip()
        # Empty rrule means single-shot at dtstart — fine. validate_rrule_string
        # is a no-op for empty input but the cross-field check below catches
        # the "no rrule + no dtstart" case.
        if value:
            # Strip a leading "RRULE:" if a client sent the full iCal-line
            # prefix. dateutil handles both forms but we canonicalize to the
            # bare "FREQ=..." form for storage.
            if value.upper().startswith("RRULE:"):
                value = value[len("RRULE:") :]
            try:
                # dtstart from attrs may not be set yet at field-validation
                # time; we pass None and the validator uses a placeholder.
                validate_rrule_string(value)
            except RRuleValidationError as e:
                raise serializers.ValidationError(str(e))
        return value

    def validate_rdates(self, value):
        return _validate_iso_datetime_list(value, "rdates")

    def validate_exdates(self, value):
        return _validate_iso_datetime_list(value, "exdates")

    def validate_tzid(self, value: str) -> str:
        value = (value or "UTC").strip()
        if value == "UTC":
            return value
        try:
            ZoneInfo(value)
        except ZoneInfoNotFoundError:
            raise serializers.ValidationError(f"tzid {value!r} is not a recognized IANA timezone")
        return value

    def validate_extra_context(self, value: str) -> str:
        if value and len(value) > EXTRA_CONTEXT_MAX_LENGTH:
            raise serializers.ValidationError(f"extra_context must be at most {EXTRA_CONTEXT_MAX_LENGTH} characters")
        return value

    def validate(self, attrs):
        # On update, lock `scheduler` and `project` — the design says swap
        # via uninstall + reinstall, not in-place repointing (would leave
        # workspace/project inconsistent with scheduler.workspace).
        if self.instance is not None:
            for locked in ("scheduler", "project"):
                if locked in attrs and attrs[locked] != getattr(self.instance, locked):
                    raise serializers.ValidationError({locked: f"{locked} cannot be changed; uninstall and re-install"})
        # Cross-field RRULE validation now that we have both dtstart and rrule.
        # Prefer attrs (incoming changes) over the instance (existing values).
        rrule_str = attrs.get(
            "rrule",
            getattr(self.instance, "rrule", "") if self.instance else "",
        )
        dtstart = attrs.get(
            "dtstart",
            getattr(self.instance, "dtstart", None) if self.instance else None,
        )
        if rrule_str:
            try:
                validate_rrule_string(rrule_str, dtstart=dtstart)
            except RRuleValidationError as e:
                raise serializers.ValidationError({"rrule": str(e)})
        # A chosen pod must belong to the binding's project. Resolve the
        # project from the *authoritative* source first: the existing instance
        # (update) or the view-supplied context (create — the view injects this
        # project at save()). Only fall back to the client-sent `project` when
        # neither is available. Trusting the request body here would let a
        # crafted payload pass a foreign-project pod (claim project=B, send a
        # B-pod) while save() still pins the binding to the URL's project.
        pod = attrs.get("pod")
        if pod is not None:
            project_id = (
                (self.instance.project_id if self.instance is not None else None)
                or getattr(self.context.get("project"), "id", None)
                or getattr(attrs.get("project"), "id", None)
            )
            if project_id is not None and pod.project_id != project_id:
                raise serializers.ValidationError(
                    {"pod": "pod must belong to the same project as this scheduler install"}
                )
        return super().validate(attrs)


class SchedulerBindingDetailSerializer(SchedulerBindingSerializer):
    """Expanded payload used by one scheduler installation's detail page."""

    resolved_prompt = serializers.SerializerMethodField()
    run_count = serializers.SerializerMethodField()
    scheduler_source = serializers.CharField(source="scheduler.source", read_only=True)
    scheduler_is_enabled = serializers.BooleanField(source="scheduler.is_enabled", read_only=True)

    class Meta(SchedulerBindingSerializer.Meta):
        fields = SchedulerBindingSerializer.Meta.fields + [
            "resolved_prompt",
            "run_count",
            "scheduler_source",
            "scheduler_is_enabled",
        ]
        read_only_fields = SchedulerBindingSerializer.Meta.read_only_fields + [
            "resolved_prompt",
            "run_count",
            "scheduler_source",
            "scheduler_is_enabled",
        ]

    def get_resolved_prompt(self, obj: SchedulerBinding) -> str:
        from karma_team.prompting.context import build_scheduler_task_body

        return build_scheduler_task_body(obj)

    def get_run_count(self, obj: SchedulerBinding) -> int:
        # The count must use the same private-Runner gate as the history list;
        # project membership alone must not reveal hidden run metadata.
        request = self.context.get("request")
        if request is None or not request.user.is_authenticated:
            return 0
        return obj.agent_runs.filter(scheduler_run_visible_to_user_q(request.user)).count()
