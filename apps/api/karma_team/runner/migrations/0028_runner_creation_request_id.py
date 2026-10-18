# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only

from django.db import migrations, models
from django.db.models import Q


class Migration(migrations.Migration):
    dependencies = [
        ("runner", "0027_auto_project_binding"),
    ]

    operations = [
        migrations.AddField(
            model_name="runner",
            name="creation_request_id",
            field=models.UUIDField(blank=True, null=True),
        ),
        migrations.AddConstraint(
            model_name="runner",
            constraint=models.UniqueConstraint(
                condition=Q(
                    dev_machine__isnull=False,
                    creation_request_id__isnull=False,
                ),
                fields=("dev_machine", "creation_request_id"),
                name="runner_unique_creation_request_per_machine",
            ),
        ),
    ]
