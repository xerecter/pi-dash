#!/bin/bash
set -e

python manage.py wait_for_db
python manage.py wait_for_migrations

exec watchmedo auto-restart \
  --directory=./karma_team \
  --pattern="*.py" \
  --recursive \
  -- celery -A karma_team beat -l info
