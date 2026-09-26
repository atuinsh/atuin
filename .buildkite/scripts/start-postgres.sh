#!/usr/bin/env bash
# Starts Postgres for the integration tests on localhost:5432 (the
# ATUIN_DB_URI the step sets) and waits until it accepts connections.
set -euo pipefail

echo "--- :postgres: Starting Postgres"
docker run -d --name postgres \
  -e POSTGRES_USER=atuin -e POSTGRES_PASSWORD=pass -e POSTGRES_DB=atuin \
  -p 5432:5432 \
  postgres:18-alpine >/dev/null

for _ in $(seq 1 30); do
  if docker exec postgres pg_isready -U atuin >/dev/null 2>&1; then
    echo "Postgres is ready"
    exit 0
  fi
  sleep 1
done
docker logs postgres
echo "+++ :rotating_light: Postgres did not become ready"
exit 1
