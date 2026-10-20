#!/bin/bash
set -e

python manage.py wait_for_db
# Wait for migrations
python manage.py wait_for_migrations
# Run the processes
# Bound the prefork pool because each child can retain its own database
# connection. The default is min(nproc, 8), while an explicit deployment value
# remains authoritative.
concurrency="${CELERY_WORKER_CONCURRENCY:-}"
if [ -z "$concurrency" ]; then
  concurrency=$(nproc 2>/dev/null || echo 8)
  if [ "$concurrency" -gt 8 ]; then
    concurrency=8
  fi
fi

# Let Celery receive container shutdown signals directly.
exec celery -A karma_team worker -l info --concurrency "$concurrency"
