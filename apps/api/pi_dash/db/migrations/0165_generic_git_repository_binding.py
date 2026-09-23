# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only

import django.db.models.deletion
from django.db import migrations, models


class Migration(migrations.Migration):
    dependencies = [
        ("db", "0164_unify_ticking_cadence_3h"),
    ]

    operations = [
        migrations.AlterField(
            model_name="gitrepository",
            name="provider",
            field=models.CharField(
                choices=[
                    ("github", "GitHub"),
                    ("gitlab", "GitLab"),
                    ("generic", "Generic Git"),
                ],
                max_length=32,
            ),
        ),
        migrations.AlterField(
            model_name="gitrepositorybinding",
            name="provider_account",
            field=models.ForeignKey(
                blank=True,
                null=True,
                on_delete=django.db.models.deletion.SET_NULL,
                related_name="repository_bindings",
                to="db.gitprovideraccount",
            ),
        ),
    ]
