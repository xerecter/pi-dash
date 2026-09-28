# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Development settings"""

import os

from .common import *  # noqa
from karma_team.config import get_config

DEBUG = True

# The local stack is served over HTTP. Django's default COOP header is only
# effective on a secure origin and causes browsers to log a misleading warning
# during the form redirect, so leave it unset for local development.
SECURE_CROSS_ORIGIN_OPENER_POLICY = None

# Debug Toolbar settings
INSTALLED_APPS += ("debug_toolbar",)  # noqa
MIDDLEWARE += ("debug_toolbar.middleware.DebugToolbarMiddleware",)  # noqa

DEBUG_TOOLBAR_PATCH_SETTINGS = False

# Only show emails in console don't send it to smtp
EMAIL_BACKEND = get_config("EMAIL_BACKEND", "django.core.mail.backends.console.EmailBackend")

CACHES = {
    "default": {
        "BACKEND": "django_redis.cache.RedisCache",
        "LOCATION": REDIS_URL,  # noqa
        "OPTIONS": {"CLIENT_CLASS": "django_redis.client.DefaultClient"},
    }
}

INTERNAL_IPS = ("127.0.0.1",)

MEDIA_URL = "/uploads/"
MEDIA_ROOT = os.path.join(BASE_DIR, "uploads")  # noqa

LOG_DIR = os.path.join(BASE_DIR, "logs")  # noqa

if not os.path.exists(LOG_DIR):
    os.makedirs(LOG_DIR)

LOGGING = {
    "version": 1,
    "disable_existing_loggers": True,
    "formatters": {
        "verbose": {
            "format": "{levelname} {asctime} {module} {process:d} {thread:d} {message}",
            "style": "{",
        },
        "json": {
            "()": "pythonjsonlogger.jsonlogger.JsonFormatter",
            "fmt": "%(levelname)s %(asctime)s %(module)s %(name)s %(message)s",
        },
    },
    "handlers": {
        "console": {
            "level": "DEBUG",
            "class": "logging.StreamHandler",
            "formatter": "json",
        }
    },
    "loggers": {
        "karma_team.api.request": {
            "level": "INFO",
            "handlers": ["console"],
            "propagate": False,
        },
        "karma_team.api": {"level": "INFO", "handlers": ["console"], "propagate": False},
        "karma_team.worker": {"level": "INFO", "handlers": ["console"], "propagate": False},
        "karma_team.exception": {
            "level": "ERROR",
            "handlers": ["console"],
            "propagate": False,
        },
        "karma_team.external": {
            "level": "INFO",
            "handlers": ["console"],
            "propagate": False,
        },
        "karma_team.mongo": {
            "level": "INFO",
            "handlers": ["console"],
            "propagate": False,
        },
        "karma_team.authentication": {
            "level": "INFO",
            "handlers": ["console"],
            "propagate": False,
        },
        "karma_team.migrations": {
            "level": "INFO",
            "handlers": ["console"],
            "propagate": False,
        },
    },
}
