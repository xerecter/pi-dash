# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Runner enrollment + legacy refresh endpoints.

The active onboarding flow is ``karma-team auth login`` followed by
``karma-team runner add``, which calls ``RunnerCreateEndpoint`` with a
dev-machine MachineToken. ``RunnerEnrollEndpoint`` remains available only
to redeem already-minted legacy one-time enrollment tokens. Legacy
refreshes hit ``POST /api/v1/runner/runners/<rid>/refresh/`` and rotate
the refresh token in lock-step on the server.
"""

from __future__ import annotations

import logging
import re
import uuid as _uuid
from typing import Optional

from django.db import IntegrityError, transaction
from django.utils import timezone
from rest_framework import status
from rest_framework.permissions import AllowAny, IsAuthenticated
from rest_framework.response import Response
from rest_framework.views import APIView

from karma_team.api.middleware.api_authentication import APIKeyAuthentication
from karma_team.authentication.services.cli_tokens import deactivate_api_token
from karma_team.authentication.session import BaseSessionAuthentication
from karma_team.db.models.project import Project
from karma_team.runner.authentication import (
    MachineTokenAuthentication,
    RunnerAccessTokenAuthentication,
    RunnerRefreshTokenAuthentication,
)
from karma_team.runner.models import (
    DevMachine,
    MachineToken,
    Pod,
    Runner,
    RunnerBindingSource,
    RunnerForceRefresh,
    RunnerProvisioning,
    RunnerStatus,
)
from karma_team.runner.serializers import RunnerEnrollRequestSerializer
from karma_team.runner.services import tokens
from karma_team.runner.services.permissions import is_workspace_member
from karma_team.runner.services.project_visibility import (
    projects_visible_to_machine_token,
)
from karma_team.runner.services.pubsub import close_runner_session, send_to_runner
from karma_team.runner.services.runner_delete import delete_runner as delete_runner_svc

logger = logging.getLogger(__name__)


class DevMachineOwnershipError(Exception):
    pass


def _touch_dev_machine(machine: DevMachine, *, host_label: str) -> DevMachine:
    host_label = (host_label or "").strip()[:255]
    now = timezone.now()
    update_fields = ["last_seen_at", "updated_at"]
    machine.last_seen_at = now
    if host_label and machine.host_label != host_label:
        machine.host_label = host_label
        update_fields.append("host_label")
    if host_label and not machine.label:
        machine.label = host_label[:128]
        update_fields.append("label")
    machine.save(update_fields=update_fields)
    return machine


def _get_or_create_dev_machine(
    *,
    user,
    dev_machine_id: Optional[_uuid.UUID],
    host_label: str,
) -> Optional[DevMachine]:
    host_label = (host_label or "").strip()[:255]
    now = timezone.now()
    if dev_machine_id is not None:
        locked = DevMachine.objects.select_for_update().filter(pk=dev_machine_id).first()
        if locked is not None:
            if locked.owner_id != user.id:
                raise DevMachineOwnershipError
            return _touch_dev_machine(locked, host_label=host_label)
        try:
            with transaction.atomic():
                return DevMachine.objects.create(
                    id=dev_machine_id,
                    owner=user,
                    host_label=host_label,
                    label=host_label[:128],
                    last_seen_at=now,
                )
        except IntegrityError:
            locked = DevMachine.objects.select_for_update().filter(pk=dev_machine_id).first()
            if locked is None or locked.owner_id != user.id:
                raise DevMachineOwnershipError
            return _touch_dev_machine(locked, host_label=host_label)

    if not host_label:
        return None
    locked = (
        DevMachine.objects.select_for_update()
        .filter(owner=user, host_label=host_label, revoked_at__isnull=True)
        .order_by("created_at")
        .first()
    )
    if locked is not None:
        return _touch_dev_machine(locked, host_label=host_label)
    try:
        with transaction.atomic():
            return DevMachine.objects.create(
                owner=user,
                host_label=host_label,
                label=host_label[:128],
                last_seen_at=now,
            )
    except IntegrityError:
        # Legacy callers without a stable id can still race on old database
        # states that have the prior owner/host constraint. Reuse the winner.
        return (
            DevMachine.objects.select_for_update()
            .filter(owner=user, host_label=host_label, revoked_at__isnull=True)
            .order_by("created_at")
            .first()
        )


def _maybe_mint_machine_token(
    *,
    user,
    workspace,
    dev_machine: Optional[DevMachine],
    host_label: str,
) -> Optional[tokens.MintedToken]:
    """Bootstrap a MachineToken if the user has none for this host.

    ``design.md`` §5.1: bootstrap runs inside the enrollment transaction
    so two concurrent enrollments cannot both mint a token. The unique
    constraint backs us up; the lock prevents the steady-state race.
    """
    filters = {
        "user": user,
        "workspace": workspace,
        "revoked_at__isnull": True,
    }
    if dev_machine is not None:
        filters["dev_machine"] = dev_machine
    else:
        filters["host_label"] = host_label
        filters["dev_machine__isnull"] = True
    locked = MachineToken.objects.select_for_update().filter(**filters).first()
    if locked is not None:
        return None
    minted = tokens.mint_machine_token()
    try:
        with transaction.atomic():
            MachineToken.objects.create(
                user=user,
                dev_machine=dev_machine,
                workspace=workspace,
                host_label=host_label,
                token_hash=minted.hashed,
                token_fingerprint=minted.fingerprint,
                label=f"machine: {host_label[:96]}",
                is_service=True,
            )
    except IntegrityError:
        return None
    return minted


def _rotate_machine_token(
    *,
    user,
    workspace,
    dev_machine: Optional[DevMachine],
    host_label: str,
) -> tokens.MintedToken:
    filters = {
        "workspace": workspace,
        "revoked_at__isnull": True,
    }
    if dev_machine is not None:
        filters["dev_machine"] = dev_machine
    else:
        filters["user"] = user
        filters["host_label"] = host_label
        filters["dev_machine__isnull"] = True
    MachineToken.objects.select_for_update().filter(**filters).update(revoked_at=timezone.now())
    minted = tokens.mint_machine_token()
    MachineToken.objects.create(
        user=user,
        dev_machine=dev_machine,
        workspace=workspace,
        host_label=host_label,
        token_hash=minted.hashed,
        token_fingerprint=minted.fingerprint,
        label=f"machine: {host_label[:96]}",
        is_service=True,
    )
    return minted


class RunnerInviteEndpoint(APIView):
    """Deprecated token mint endpoint.

    The cloud UI no longer creates legacy token commands. Keep the
    route only so old clients receive a deliberate response instead of
    accidentally minting new one-time enrollment tokens.
    """

    authentication_classes = [BaseSessionAuthentication]
    permission_classes = [IsAuthenticated]

    def post(self, request):
        return Response(
            {
                "error": "legacy_enrollment_disabled",
                "error_description": "Use `karma-team auth login` and `karma-team runner add` to register runners.",
            },
            status=status.HTTP_410_GONE,
        )


def _managed_cap_error(*, user, pod, workspace_id):
    """Refuse a second bundled runner for the same user on the same project.

    The desktop enrolls one runner per project it opens; more than that would
    be a bug in the client (or a stale row it failed to reuse), and silently
    accepting them would leave the pod with several machines all claiming the
    same pinned work. Returns a ``Response`` to return, or ``None`` to proceed.
    """
    from django.conf import settings

    existing = (
        Runner.objects.filter(
            owner=user,
            pod__project_id=pod.project_id,
            pod__deleted_at__isnull=True,
            workspace_id=workspace_id,
            provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
            revoked_at__isnull=True,
        )
        .exclude(status=RunnerStatus.REVOKED)
        .count()
    )
    if existing >= settings.MANAGED_RUNNER_MAX_PER_USER_PROJECT:
        return Response(
            {
                "error": "managed_runner_limit",
                "error_description": "This project already has a karma-team Agent registered for you on a machine.",
            },
            status=status.HTTP_409_CONFLICT,
        )
    return None


class RunnerEnrollEndpoint(APIView):
    """``POST /api/v1/runner/runners/enroll/`` — public.

    Exchanges a one-time enrollment token for the runner's long-lived
    refresh token and a short-lived access token. Bootstraps a
    ``MachineToken`` for the workspace/dev-machine if none
    exists; otherwise the response omits ``machine_token``.
    """

    authentication_classes: list = []
    permission_classes = [AllowAny]
    # Inherit DEFAULT_THROTTLE_CLASSES (AnonRateThrottle, 30/minute per
    # IP) — design.md §9.1 requires enroll to be tightly auth-throttled
    # against bearer-token brute-force. The DRF default is the lightest
    # protection that won't break legitimate one-off enrollments.

    def post(self, request):
        serializer = RunnerEnrollRequestSerializer(data=request.data)
        serializer.is_valid(raise_exception=True)
        data = serializer.validated_data

        token_hash = tokens.hash_token(data["enrollment_token"])
        dev_machine_id = data.get("dev_machine_id")
        host_label = (data.get("host_label") or "")[:255]
        body_name = (data.get("name") or "").strip()[:128]

        with transaction.atomic():
            try:
                runner = (
                    Runner.objects.select_for_update()
                    .select_related("workspace", "pod__project")
                    .get(enrollment_token_hash=token_hash)
                )
            except Runner.DoesNotExist:
                return Response(
                    {"error": "invalid_or_expired_enrollment_token"},
                    status=status.HTTP_401_UNAUTHORIZED,
                )
            if runner.revoked_at is not None:
                return Response(
                    {"error": "runner_revoked"},
                    status=status.HTTP_409_CONFLICT,
                )
            if runner.enrolled_at is not None:
                return Response(
                    {"error": "enrollment_token_already_used"},
                    status=status.HTTP_409_CONFLICT,
                )

            refresh = tokens.mint_refresh_token()
            access = tokens.mint_access_token(
                runner_id=str(runner.id),
                user_id=str(runner.owner_id),
                workspace_id=str(runner.workspace_id),
                rtg=1,
            )
            try:
                dev_machine = _get_or_create_dev_machine(
                    user=runner.owner,
                    dev_machine_id=dev_machine_id,
                    host_label=host_label,
                )
            except DevMachineOwnershipError:
                return Response(
                    {"error": "dev_machine_not_found"},
                    status=status.HTTP_404_NOT_FOUND,
                )
            update_fields = [
                "dev_machine",
                "host_label",
                "enrolled_at",
                "enrollment_token_hash",
                "enrollment_token_fingerprint",
                "refresh_token_hash",
                "refresh_token_fingerprint",
                "refresh_token_generation",
                "previous_refresh_token_hash",
            ]
            runner.dev_machine = dev_machine
            runner.host_label = host_label or runner.host_label
            runner.enrolled_at = timezone.now()
            runner.enrollment_token_hash = ""
            runner.enrollment_token_fingerprint = ""
            runner.refresh_token_hash = refresh.hashed
            runner.refresh_token_fingerprint = refresh.fingerprint
            runner.refresh_token_generation = 1
            runner.previous_refresh_token_hash = ""
            if body_name:
                runner.name = body_name
                update_fields.append("name")
            runner.save(update_fields=update_fields)

            # MachineToken bootstrap inside the same transaction.
            machine_minted: Optional[tokens.MintedToken] = None
            if host_label:
                machine_minted = _maybe_mint_machine_token(
                    user=runner.owner,
                    workspace=runner.workspace,
                    dev_machine=dev_machine,
                    host_label=host_label,
                )

        project_identifier = runner.pod.project.identifier if runner.pod and runner.pod.project_id else ""
        body = {
            "runner_id": str(runner.id),
            "runner_name": runner.name,
            "refresh_token": refresh.raw,
            "access_token": access.raw,
            "access_token_expires_at": access.expires_at.isoformat(),
            "refresh_token_generation": runner.refresh_token_generation,
            "workspace_slug": runner.workspace.slug,
            "pod_slug": runner.pod.name if runner.pod_id else "",
            "project_identifier": project_identifier,
            "long_poll_interval_secs": 25,
            "protocol_version": 4,
            "machine_token_minted": machine_minted is not None,
        }
        if machine_minted is not None:
            body["machine_token"] = machine_minted.raw
        return Response(body, status=status.HTTP_201_CREATED)


class RunnerRefreshEndpoint(APIView):
    """``POST /api/v1/runner/runners/<runner_id>/refresh/``.

    Verifies the bearer **refresh** token against the runner's
    ``refresh_token_hash`` (current) and ``previous_refresh_token_hash``
    (replay window), rotates atomically under ``select_for_update``,
    and returns a fresh refresh+access token pair. Live workspace
    membership re-check happens here per ``design.md`` §5.3 step 4.
    """

    authentication_classes = [RunnerRefreshTokenAuthentication]
    permission_classes: list = []
    throttle_classes: list = []

    def post(self, request, runner_id):
        raw = getattr(request, "auth_refresh_token", None)
        if not raw:
            return Response(
                {"error": "missing_refresh_token"},
                status=status.HTTP_401_UNAUTHORIZED,
            )
        presented_hash = tokens.hash_token(raw)

        with transaction.atomic():
            runner = Runner.objects.select_for_update().select_related("workspace").filter(id=runner_id).first()
            if runner is None:
                return Response(
                    {"error": "invalid_refresh_token"},
                    status=status.HTTP_401_UNAUTHORIZED,
                )
            if runner.revoked_at is not None:
                return Response(
                    {"error": "runner_revoked"},
                    status=status.HTTP_401_UNAUTHORIZED,
                )
            if (
                runner.dev_machine_id is not None
                and DevMachine.objects.filter(
                    pk=runner.dev_machine_id,
                    revoked_at__isnull=False,
                ).exists()
            ):
                return Response(
                    {"error": "dev_machine_revoked"},
                    status=status.HTTP_401_UNAUTHORIZED,
                )

            if presented_hash == runner.refresh_token_hash:
                pass  # Current generation; happy path.
            elif runner.previous_refresh_token_hash and presented_hash == runner.previous_refresh_token_hash:
                runner.revoke(reason="refresh_token_replayed")
                return Response(
                    {"error": "refresh_token_replayed"},
                    status=status.HTTP_401_UNAUTHORIZED,
                )
            else:
                return Response(
                    {"error": "invalid_refresh_token"},
                    status=status.HTTP_401_UNAUTHORIZED,
                )

            if not is_workspace_member(runner.owner, runner.workspace_id):
                runner.revoke(reason="membership_revoked")
                return Response(
                    {"error": "membership_revoked"},
                    status=status.HTTP_401_UNAUTHORIZED,
                )

            new_refresh = tokens.mint_refresh_token()
            runner.previous_refresh_token_hash = runner.refresh_token_hash
            runner.refresh_token_hash = new_refresh.hashed
            runner.refresh_token_fingerprint = new_refresh.fingerprint
            runner.refresh_token_generation = runner.refresh_token_generation + 1
            runner.save(
                update_fields=[
                    "previous_refresh_token_hash",
                    "refresh_token_hash",
                    "refresh_token_fingerprint",
                    "refresh_token_generation",
                ]
            )
            new_access = tokens.mint_access_token(
                runner_id=str(runner.id),
                user_id=str(runner.owner_id),
                workspace_id=str(runner.workspace_id),
                rtg=runner.refresh_token_generation,
            )
            RunnerForceRefresh.objects.filter(runner=runner).delete()

        return Response(
            {
                "refresh_token": new_refresh.raw,
                "access_token": new_access.raw,
                "access_token_expires_at": new_access.expires_at.isoformat(),
                "refresh_token_generation": runner.refresh_token_generation,
            }
        )


class RunnerReviveEndpoint(APIView):
    """Deprecated token-based revive endpoint.

    Existing enrollment tokens can still be redeemed by
    ``RunnerEnrollEndpoint``, but the cloud no longer mints fresh
    legacy token commands. Operators should delete stale/revoked
    runners and add a new runner from the target authenticated machine.
    """

    authentication_classes = [BaseSessionAuthentication]
    permission_classes = [IsAuthenticated]

    def post(self, request, runner_id):
        return Response(
            {
                "error": "legacy_enrollment_disabled",
                "error_description": (
                    "Delete this runner and run `karma-team runner add` from the authenticated target machine."
                ),
            },
            status=status.HTTP_410_GONE,
        )


class RunnerSelfRevokeEndpoint(APIView):
    """``DELETE /api/v1/runner/runners/<rid>/`` — runner self-deletion.

    The web UI's `RunnerDetailEndpoint.delete` covers operator-driven
    teardown via session auth; this is the symmetric machine-token path
    so the daemon can `karma-team runner remove <name>` cleanly without
    requiring the user to click through the cloud UI. Deletion is
    state-idempotent: once the row is gone, a repeated DELETE returns 404 and
    the caller may treat that as the desired already-absent state.
    """

    authentication_classes = [MachineTokenAuthentication, RunnerAccessTokenAuthentication]
    permission_classes: list = []
    throttle_classes: list = []

    def delete(self, request, runner_id):
        machine_token = getattr(request, "auth_machine_token", None)
        if machine_token is not None:
            runner = Runner.objects.filter(pk=runner_id).first()
            if runner is None:
                return Response({"error": "not_found"}, status=status.HTTP_404_NOT_FOUND)
            if (
                machine_token.dev_machine_id is None
                or runner.dev_machine_id != machine_token.dev_machine_id
                or runner.owner_id != machine_token.user_id
                or runner.workspace_id != machine_token.workspace_id
            ):
                return Response(
                    {"error": "not_found"},
                    status=status.HTTP_404_NOT_FOUND,
                )
            delete_runner_svc(runner, purge_local=True)
            return Response(status=status.HTTP_204_NO_CONTENT)

        auth_runner = getattr(request, "auth_runner", None)
        if auth_runner is None or str(auth_runner.id) != str(runner_id):
            return Response(
                {"error": "runner_id_mismatch"},
                status=status.HTTP_403_FORBIDDEN,
            )
        runner_pk = auth_runner.pk
        # `revoke()` is no-op safe if already revoked; we still close the
        # session and drop the row so a stale daemon can't re-attach.
        auth_runner.revoke(reason="self_revoked")
        send_to_runner(
            runner_pk,
            {"type": "revoke", "reason": "self_revoked"},
        )
        close_runner_session(runner_pk)
        Runner.objects.filter(pk=runner_pk).delete()
        return Response(status=status.HTTP_204_NO_CONTENT)


# Runner names must round-trip safely through systemd unit names,
# filesystem paths, and TOML keys — so reject anything that would
# blow up downstream. Matches the runner CLI's `runner_name::validate`
# in spirit; the regex stays simple on purpose.
_RUNNER_NAME_RE = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_.\-]{0,127}$")
_MAX_AUTO_NAME_RETRIES = 5


class RunnerCreateEndpoint(APIView):
    """``POST /api/v1/runner/runners/`` — CLI-initiated runner creation.

    The active replacement for the legacy invite/enroll token pair.
    Callers normally have a dev-machine MachineToken (issued by
    ``karma-team auth login``). One round-trip mints the Runner row under that
    dev machine and marks it enrolled. APIToken auth remains accepted as a
    transition path and rotates/returns a MachineToken.

    Auth: ``X-Api-Key`` (MachineToken or APIToken). The token's user must
    be a member of the target workspace and the workspace must contain the
    named project. We deliberately do NOT require admin/maintainer here for
    parity with the web "Add Runner" button — any workspace member who can
    see the project can register a runner against it.

    Workspace resolution: when ``workspace_slug`` is omitted, and the
    caller is a member of exactly one workspace, we infer it. With zero
    memberships we return 400; with multiple we require the caller to
    name one. This matches the single-workspace-per-host onboarding
    model the CLI is built around.
    """

    authentication_classes = [APIKeyAuthentication]
    permission_classes = [IsAuthenticated]
    throttle_classes: list = []

    def post(self, request):
        from karma_team.db.models.project import Project
        from karma_team.db.models.workspace import Workspace, WorkspaceMember

        auth_machine_token = getattr(request, "auth_machine_token", None)
        workspace_slug = (request.data.get("workspace_slug") or "").strip()
        project_identifier = (request.data.get("project") or "").strip()
        dev_machine_id_raw = (request.data.get("dev_machine_id") or "").strip()
        host_label = (request.data.get("host_label") or "").strip()[:255]
        body_name = (request.data.get("name") or "").strip()[:128]
        pod_name = (request.data.get("pod") or "").strip()
        creation_request_id_raw = str(request.data.get("request_id") or "").strip()
        creation_request_id: Optional[_uuid.UUID] = None
        if creation_request_id_raw:
            try:
                creation_request_id = _uuid.UUID(creation_request_id_raw)
            except (TypeError, ValueError, AttributeError):
                return Response(
                    {"error": "invalid_request_id"},
                    status=status.HTTP_400_BAD_REQUEST,
                )

        if not project_identifier:
            return Response(
                {"error": "project is required"},
                status=status.HTTP_400_BAD_REQUEST,
            )
        dev_machine_id: Optional[_uuid.UUID] = None
        if dev_machine_id_raw:
            try:
                dev_machine_id = _uuid.UUID(dev_machine_id_raw)
            except (TypeError, ValueError, AttributeError):
                return Response(
                    {"error": "invalid_dev_machine_id"},
                    status=status.HTTP_400_BAD_REQUEST,
                )
        if auth_machine_token is not None:
            if workspace_slug and workspace_slug != auth_machine_token.workspace.slug:
                return Response(
                    {"error": "workspace_not_found"},
                    status=status.HTTP_404_NOT_FOUND,
                )
            workspace_slug = auth_machine_token.workspace.slug
            if auth_machine_token.dev_machine_id is not None:
                if dev_machine_id is not None and dev_machine_id != auth_machine_token.dev_machine_id:
                    return Response(
                        {"error": "dev_machine_token_mismatch"},
                        status=status.HTTP_403_FORBIDDEN,
                    )
                dev_machine_id = auth_machine_token.dev_machine_id
            if not host_label:
                host_label = auth_machine_token.host_label
        if body_name and not _RUNNER_NAME_RE.match(body_name):
            return Response(
                {
                    "error": "invalid_runner_name",
                    "error_description": (
                        "name must start with a letter, digit, or underscore "
                        "and contain only letters, digits, underscore, dot, or dash"
                    ),
                },
                status=status.HTTP_400_BAD_REQUEST,
            )

        # Resolve workspace. Explicit slug wins; otherwise infer from
        # the caller's single workspace membership (most karma-team CLI
        # users only belong to one). Multi-workspace users must be
        # explicit so we don't pick the wrong one.
        if workspace_slug:
            workspace = Workspace.objects.filter(slug=workspace_slug).first()
            if workspace is None or not is_workspace_member(request.user, workspace.id):
                # Same 404 in both cases — don't leak existence of
                # workspaces the caller can't see.
                return Response(
                    {"error": "workspace_not_found"},
                    status=status.HTTP_404_NOT_FOUND,
                )
        else:
            memberships = list(
                WorkspaceMember.objects.filter(member=request.user, is_active=True)
                .select_related("workspace")
                .order_by("created_at")[:2]
            )
            if not memberships:
                return Response(
                    {
                        "error": "no_workspace_membership",
                        "error_description": "Caller is not a member of any workspace.",
                    },
                    status=status.HTTP_400_BAD_REQUEST,
                )
            if len(memberships) > 1:
                return Response(
                    {
                        "error": "workspace_slug_required",
                        "error_description": (
                            "Caller belongs to multiple workspaces — pass workspace_slug to pick one."
                        ),
                    },
                    status=status.HTTP_400_BAD_REQUEST,
                )
            workspace = memberships[0].workspace

        # Mint the runner row, retrying on auto-name collisions when no
        # name was supplied. Two `karma-team runner add` calls running
        # concurrently can both compute the same ``runner_NNN`` default;
        # retrying server-side hides that race from the user. If the
        # caller passed an explicit name and we 409, surface it.
        attempts = 0
        last_exc: Optional[IntegrityError] = None
        runner: Optional[Runner] = None
        project: Optional[Project] = None
        pod: Optional[Pod] = None
        machine_minted: Optional[tokens.MintedToken] = None
        response_status = status.HTTP_201_CREATED
        dev_machine: Optional[DevMachine] = None
        while attempts < _MAX_AUTO_NAME_RETRIES:
            attempts += 1
            try:
                with transaction.atomic():
                    dev_machine = _get_or_create_dev_machine(
                        user=request.user,
                        dev_machine_id=dev_machine_id,
                        host_label=host_label,
                    )
                    if creation_request_id is not None and dev_machine is None:
                        return Response(
                            {"error": "request_id_requires_dev_machine"},
                            status=status.HTTP_400_BAD_REQUEST,
                        )
                    if creation_request_id is not None:
                        runner = (
                            # Only lock the durable idempotency winner.  The
                            # select_related joins are response hydration, not
                            # part of the lock contract.  Locking their rows as
                            # well would create a mixed DevMachine ->
                            # Runner/Project lock set that can deadlock with
                            # project deletion's Project -> Runner order.
                            Runner.objects.select_for_update(of=("self",))
                            .select_related("pod__project", "workspace")
                            .filter(
                                dev_machine=dev_machine,
                                creation_request_id=creation_request_id,
                                owner_id=request.user.id,
                                workspace_id=workspace.id,
                            )
                            .first()
                        )
                        if runner is not None:
                            project = runner.pod.project
                            pod = runner.pod
                            workspace = runner.workspace
                            response_status = status.HTTP_200_OK
                            break
                        if Runner.objects.filter(
                            dev_machine=dev_machine,
                            creation_request_id=creation_request_id,
                        ).exists():
                            return Response(
                                {"error": "idempotency_scope_conflict"},
                                status=status.HTTP_409_CONFLICT,
                            )
                    # Lock in the same order as automatic binding:
                    # DevMachine -> Project -> Pod -> Runner. Resolving the
                    # active project and pod inside this transaction prevents
                    # a project delete from committing between validation and
                    # the runner insert.
                    project = (
                        Project.objects.select_for_update()
                        .filter(
                            workspace_id=workspace.id,
                            identifier=project_identifier,
                            deleted_at__isnull=True,
                        )
                        .first()
                    )
                    if project is None:
                        return Response(
                            {"error": "project_not_found"},
                            status=status.HTTP_404_NOT_FOUND,
                        )

                    pod = None
                    if pod_name:
                        pod = (
                            Pod.objects.select_for_update()
                            .filter(
                                project_id=project.id,
                                name=pod_name,
                                deleted_at__isnull=True,
                            )
                            .first()
                        )
                    if pod is None:
                        pod = (
                            Pod.objects.select_for_update()
                            .filter(
                                project_id=project.id,
                                is_default=True,
                                deleted_at__isnull=True,
                            )
                            .first()
                        )
                    if pod is None:
                        return Response(
                            {"error": "project_has_no_default_pod"},
                            status=status.HTTP_409_CONFLICT,
                        )

                    name = body_name or _next_auto_runner_name(pod)
                    # Provisioning is inherited from the machine, never read
                    # from the body: a client must not be able to claim it is
                    # karma-team-managed and thereby take pinned desktop work.
                    provisioning = getattr(dev_machine, "provisioning", RunnerProvisioning.MANUAL)
                    if provisioning == RunnerProvisioning.DESKTOP_BUNDLED:
                        # Sign-out removes the local config, but the server row
                        # deliberately survives. Re-enrollment must recover it
                        # before applying the per-user/project creation cap.
                        runner = (
                            Runner.objects.select_for_update()
                            .filter(
                                owner=request.user,
                                workspace_id=workspace.id,
                                dev_machine=dev_machine,
                                pod=pod,
                                provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
                                revoked_at__isnull=True,
                            )
                            .first()
                        )
                        if runner is not None:
                            if creation_request_id is not None and runner.creation_request_id is None:
                                runner.creation_request_id = creation_request_id
                                runner.save(update_fields=["creation_request_id", "updated_at"])
                            response_status = status.HTTP_200_OK
                            break
                        cap_error = _managed_cap_error(user=request.user, pod=pod, workspace_id=workspace.id)
                        if cap_error is not None:
                            return cap_error
                    runner = Runner.objects.create(
                        owner=request.user,
                        workspace_id=workspace.id,
                        dev_machine=dev_machine,
                        pod=pod,
                        name=name,
                        host_label=host_label,
                        provisioning=provisioning,
                        creation_request_id=creation_request_id,
                        enrolled_at=timezone.now(),
                    )
                    if auth_machine_token is None and host_label:
                        machine_minted = _rotate_machine_token(
                            user=runner.owner,
                            workspace=runner.workspace,
                            dev_machine=dev_machine,
                            host_label=host_label,
                        )
                        deactivate_api_token(request.auth, only_cli_device_tokens=True)
                break
            except DevMachineOwnershipError:
                return Response(
                    {"error": "dev_machine_not_found"},
                    status=status.HTTP_404_NOT_FOUND,
                )
            except IntegrityError as exc:
                last_exc = exc
                if creation_request_id is not None and dev_machine is not None:
                    # The conditional unique constraint is the final arbiter
                    # if a concurrent process wins despite the machine lock
                    # (for example around legacy machine creation). Always
                    # prefer that durable winner over classifying a name race.
                    runner = (
                        Runner.objects.select_related("pod__project", "workspace")
                        .filter(
                            dev_machine_id=dev_machine.id,
                            creation_request_id=creation_request_id,
                            owner_id=request.user.id,
                            workspace_id=workspace.id,
                        )
                        .first()
                    )
                    if runner is not None:
                        project = runner.pod.project
                        pod = runner.pod
                        workspace = runner.workspace
                        response_status = status.HTTP_200_OK
                        break
                    if Runner.objects.filter(
                        dev_machine_id=dev_machine.id,
                        creation_request_id=creation_request_id,
                    ).exists():
                        return Response(
                            {"error": "idempotency_scope_conflict"},
                            status=status.HTTP_409_CONFLICT,
                        )
                if body_name:
                    # Caller named it explicitly — don't retry.
                    return Response(
                        {"error": "runner_name_taken"},
                        status=status.HTTP_409_CONFLICT,
                    )
                # Auto-name path: retry with a freshly recomputed name.
                continue
        if runner is None or project is None or pod is None:
            logger.warning(
                "RunnerCreateEndpoint: gave up after %s auto-name attempts: %s",
                _MAX_AUTO_NAME_RETRIES,
                last_exc,
            )
            return Response(
                {"error": "could_not_allocate_runner_name"},
                status=status.HTTP_409_CONFLICT,
            )

        body = {
            "runner_id": str(runner.id),
            "runner_name": runner.name,
            "refresh_token": "",
            "access_token": "",
            "access_token_expires_at": timezone.now().isoformat(),
            "refresh_token_generation": runner.refresh_token_generation,
            "workspace_slug": workspace.slug,
            "pod_slug": pod.name,
            "project_identifier": project.identifier,
            "long_poll_interval_secs": 25,
            "protocol_version": 4,
            "machine_token_minted": machine_minted is not None,
        }
        if machine_minted is not None:
            body["machine_token"] = machine_minted.raw
        return Response(body, status=response_status)


def _auto_binding_name(project_identifier: str, machine_id: _uuid.UUID, project_id: _uuid.UUID) -> str:
    """Keep generated names distinct across workspaces on the same daemon."""

    project_part = re.sub(r"[^A-Za-z0-9_-]+", "-", project_identifier).strip("-") or "project"
    machine_part = _uuid.UUID(str(machine_id)).hex[:8]
    return f"auto_{project_part[:50]}_{machine_part}_{project_id.hex[:8]}"


def _machine_for_binding_request(request, dev_machine_id) -> Optional[DevMachine]:
    token = getattr(request, "auth_machine_token", None)
    if token is None or token.dev_machine_id is None:
        return None
    if str(token.dev_machine_id) != str(dev_machine_id):
        return None
    machine = token.dev_machine
    if machine is None or machine.owner_id != token.user_id or machine.revoked_at is not None:
        return None
    return machine


def _binding_response(runner: Runner, *, created: bool) -> dict:
    project = runner.pod.project
    return {
        "runner_id": str(runner.id),
        "runner_name": runner.name,
        "project_id": str(project.id),
        "project_identifier": project.identifier,
        "pod_id": str(runner.pod_id),
        "pod_slug": runner.pod.name,
        "workspace_slug": runner.workspace.slug,
        "binding_source": runner.binding_source,
        "refresh_token": "",
        "access_token": "",
        "machine_token_minted": False,
        "protocol_version": 4,
        "created": created,
    }


class MachineProjectBindingEnsureEndpoint(APIView):
    """Idempotently ensure one default-pod runner for a machine/project."""

    authentication_classes = [MachineTokenAuthentication]
    permission_classes: list = []
    throttle_classes: list = []

    def post(self, request, dev_machine_id):
        machine = _machine_for_binding_request(request, dev_machine_id)
        if machine is None:
            return Response(
                {"error": "dev_machine_mismatch"},
                status=status.HTTP_403_FORBIDDEN,
            )
        token = request.auth_machine_token
        try:
            project_id = _uuid.UUID(str(request.data.get("project_id")))
        except (TypeError, ValueError, AttributeError):
            return Response(
                {"error": "project_id_required"},
                status=status.HTTP_400_BAD_REQUEST,
            )

        with transaction.atomic():
            locked_machine = (
                DevMachine.objects.select_for_update()
                .filter(pk=machine.id, owner_id=token.user_id, revoked_at__isnull=True)
                .first()
            )
            if locked_machine is None:
                return Response(
                    {"error": "dev_machine_mismatch"},
                    status=status.HTTP_403_FORBIDDEN,
                )
            project = (
                projects_visible_to_machine_token(
                    Project.objects.select_for_update(of=("self",)),
                    token,
                )
                .filter(
                    pk=project_id,
                )
                .first()
            )
            if project is None:
                return Response(
                    {"error": "project_not_found"},
                    status=status.HTTP_404_NOT_FOUND,
                )
            pod = (
                Pod.objects.select_for_update()
                .filter(
                    project_id=project.id,
                    is_default=True,
                    deleted_at__isnull=True,
                )
                .first()
            )
            if pod is None:
                return Response(
                    {"error": "project_has_no_default_pod"},
                    status=status.HTTP_409_CONFLICT,
                )

            existing = (
                Runner.objects.select_related("pod__project", "workspace")
                .filter(
                    dev_machine=locked_machine,
                    workspace_id=token.workspace_id,
                    pod__project_id=project.id,
                    revoked_at__isnull=True,
                    pod__deleted_at__isnull=True,
                    pod__project__deleted_at__isnull=True,
                )
                .order_by("created_at", "id")
                .first()
            )
            if existing is not None:
                return Response(_binding_response(existing, created=False), status=status.HTTP_200_OK)

            provisioning = locked_machine.provisioning
            if provisioning == RunnerProvisioning.DESKTOP_BUNDLED:
                cap_error = _managed_cap_error(
                    user=token.user,
                    pod=pod,
                    workspace_id=token.workspace_id,
                )
                if cap_error is not None:
                    return cap_error

            name = _auto_binding_name(project.identifier, locked_machine.id, project.id)
            # Runner names are unique within a pod. Machine and project
            # suffixes also distinguish local runners across workspaces.
            # Retain a bounded fallback for a pre-existing manual runner
            # with the same generated name.
            for attempt in range(100):
                candidate = name if attempt == 0 else f"{name[:115]}_{attempt + 1}"
                try:
                    # Keep a failed uniqueness attempt inside a savepoint so
                    # the surrounding machine-lock transaction remains usable
                    # when we re-read a concurrent winner.
                    with transaction.atomic():
                        runner = Runner.objects.create(
                            owner_id=token.user_id,
                            workspace_id=token.workspace_id,
                            dev_machine=locked_machine,
                            pod=pod,
                            name=candidate,
                            host_label=locked_machine.host_label,
                            provisioning=provisioning,
                            binding_source=RunnerBindingSource.AUTO_PROJECT_SYNC,
                            enrolled_at=timezone.now(),
                        )
                    runner = Runner.objects.select_related("pod__project", "workspace").get(pk=runner.pk)
                    return Response(_binding_response(runner, created=True), status=status.HTTP_201_CREATED)
                except IntegrityError:
                    winner = (
                        Runner.objects.select_related("pod__project", "workspace")
                        .filter(
                            dev_machine=locked_machine,
                            workspace_id=token.workspace_id,
                            pod__project_id=project.id,
                            revoked_at__isnull=True,
                            pod__deleted_at__isnull=True,
                            pod__project__deleted_at__isnull=True,
                        )
                        .order_by("created_at", "id")
                        .first()
                    )
                    if winner is not None:
                        return Response(_binding_response(winner, created=False), status=status.HTTP_200_OK)
            return Response(
                {"error": "could_not_allocate_runner_name"},
                status=status.HTTP_409_CONFLICT,
            )


def _next_auto_runner_name(pod: Pod) -> str:
    """Generate ``runner_NNN`` numbered higher than any existing runner
    in the same pod. Used by ``RunnerCreateEndpoint`` when the caller
    didn't supply ``name``. Race-safe via the retry loop in the caller.
    """
    count = Runner.objects.filter(pod=pod).count()
    return f"runner_{count + 1:03d}"


class MachineTokenTicketEndpoint(APIView):
    """``POST /api/v1/workspaces/<ws>/machine-tokens/tickets/``.

    Web UI mints a one-time ticket the CLI can redeem for a
    MachineToken via ``karma-team auth login <ticket>``. Backed by Redis
    with ``EX 60`` (``design.md`` §5.6, ``tasks.md`` §1.5).
    """

    authentication_classes = [BaseSessionAuthentication]
    permission_classes = [IsAuthenticated]

    def post(self, request, workspace_id):
        try:
            ws_uuid = _uuid.UUID(str(workspace_id))
        except (ValueError, AttributeError):
            return Response(
                {"error": "invalid_workspace_id"},
                status=status.HTTP_400_BAD_REQUEST,
            )
        if not is_workspace_member(request.user, ws_uuid):
            return Response(
                {"error": "workspace not found"},
                status=status.HTTP_404_NOT_FOUND,
            )
        host_label = (request.data.get("host_label") or "").strip()[:255]
        if not host_label:
            return Response(
                {"error": "host_label is required"},
                status=status.HTTP_400_BAD_REQUEST,
            )
        ticket = _uuid.uuid4().hex
        from karma_team.settings.redis import redis_instance

        client = redis_instance()
        if client is not None:
            import json as _json

            client.set(
                f"machine_token_ticket:{ticket}",
                _json.dumps(
                    {
                        "user_id": str(request.user.id),
                        "workspace_id": str(ws_uuid),
                        "host_label": host_label,
                    }
                ),
                ex=60,
            )
        return Response(
            {"ticket": ticket, "expires_in_secs": 60},
            status=status.HTTP_201_CREATED,
        )


class MachineTokenRedeemEndpoint(APIView):
    """``POST /api/v1/runner/machine-tokens/`` — public; redeem a ticket."""

    authentication_classes: list = []
    permission_classes = [AllowAny]
    # Inherit DEFAULT_THROTTLE_CLASSES (AnonRateThrottle) so a leaked
    # ticket can't be brute-redeemed at line speed.

    def post(self, request):
        ticket = (request.data.get("ticket") or "").strip()
        if not ticket:
            return Response(
                {"error": "ticket is required"},
                status=status.HTTP_400_BAD_REQUEST,
            )
        from karma_team.settings.redis import redis_instance

        client = redis_instance()
        if client is None:
            return Response(
                {"error": "redis_unavailable"},
                status=status.HTTP_503_SERVICE_UNAVAILABLE,
            )
        key = f"machine_token_ticket:{ticket}"
        # GETDEL: atomic redeem (consume on read).
        try:
            blob = client.execute_command("GETDEL", key)
        except Exception:
            blob = client.get(key)
            if blob is not None:
                client.delete(key)
        if not blob:
            return Response(
                {"error": "invalid_or_expired_ticket"},
                status=status.HTTP_401_UNAUTHORIZED,
            )
        if isinstance(blob, bytes):
            blob = blob.decode()
        import json as _json

        try:
            payload = _json.loads(blob)
        except (TypeError, ValueError):
            return Response(
                {"error": "invalid_ticket_payload"},
                status=status.HTTP_400_BAD_REQUEST,
            )
        from karma_team.db.models import User
        from karma_team.db.models.workspace import Workspace

        try:
            user = User.objects.get(id=payload["user_id"])
            workspace = Workspace.objects.get(id=payload["workspace_id"])
        except (User.DoesNotExist, Workspace.DoesNotExist, KeyError):
            return Response(
                {"error": "stale_ticket"},
                status=status.HTTP_410_GONE,
            )
        host_label = (payload.get("host_label") or "")[:255]

        with transaction.atomic():
            minted = _maybe_mint_machine_token(
                user=user,
                workspace=workspace,
                dev_machine=None,
                host_label=host_label,
            )
        if minted is None:
            return Response(
                {"error": "machine_token_already_active"},
                status=status.HTTP_409_CONFLICT,
            )
        return Response(
            {
                "machine_token": minted.raw,
                "host_label": host_label,
                "workspace_slug": workspace.slug,
            },
            status=status.HTTP_201_CREATED,
        )
