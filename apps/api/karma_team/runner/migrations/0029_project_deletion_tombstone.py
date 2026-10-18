# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only

import django.utils.timezone
from django.db import migrations, models


def backfill_project_deletion_tombstones(apps, schema_editor):
    Project = apps.get_model("db", "Project")
    ProjectDeletionTombstone = apps.get_model(
        "runner", "ProjectDeletionTombstone"
    )
    tombstones = (
        ProjectDeletionTombstone(
            project_id=project_id,
            workspace_id=workspace_id,
            deleted_at=deleted_at,
        )
        for project_id, workspace_id, deleted_at in Project._base_manager.filter(
            deleted_at__isnull=False
        )
        .order_by("id")
        .values_list("id", "workspace_id", "deleted_at")
        .iterator(chunk_size=1000)
    )
    ProjectDeletionTombstone.objects.bulk_create(
        tombstones,
        batch_size=1000,
        ignore_conflicts=True,
    )


class Migration(migrations.Migration):
    dependencies = [
        ("db", "0165_generic_git_repository_binding"),
        ("runner", "0028_runner_creation_request_id"),
    ]

    operations = [
        migrations.CreateModel(
            name="ProjectDeletionTombstone",
            fields=[
                (
                    "project_id",
                    models.UUIDField(editable=False, primary_key=True, serialize=False),
                ),
                (
                    "workspace_id",
                    models.UUIDField(db_index=True, editable=False),
                ),
                (
                    "deleted_at",
                    models.DateTimeField(
                        default=django.utils.timezone.now,
                        editable=False,
                    ),
                ),
            ],
            options={
                "db_table": "runner_project_deletion_tombstone",
                "ordering": ("project_id",),
            },
        ),
        migrations.RunPython(
            backfill_project_deletion_tombstones,
            reverse_code=migrations.RunPython.noop,
        ),
    ]
