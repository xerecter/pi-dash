# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only

from django.db import migrations, models
from django.db.models import Q


class Migration(migrations.Migration):
    dependencies = [
        ("runner", "0026_merge_agent_metadata_and_phase_kind"),
    ]

    operations = [
        migrations.AddField(
            model_name="runner",
            name="binding_source",
            field=models.CharField(
                choices=[
                    ("manual", "Bound by the user"),
                    ("auto_project_sync", "Bound by project sync"),
                ],
                db_index=True,
                default="manual",
                max_length=24,
            ),
        ),
        migrations.AddConstraint(
            model_name="runner",
            constraint=models.UniqueConstraint(
                condition=Q(
                    binding_source="auto_project_sync",
                    revoked_at__isnull=True,
                    dev_machine__isnull=False,
                ),
                fields=("dev_machine", "pod", "binding_source"),
                name="runner_one_active_auto_binding_per_machine_pod",
            ),
        ),
    ]
