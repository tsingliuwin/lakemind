#!/bin/bash
# Launcher for the Arrow sidecar (bash so add-opens array expands correctly).
# Usage: ./run.sh [project.table] [stdout_sink]
set -e
cd "$(dirname "$0")"
CP=$(find ~/.dbx/maven -name '*.jar' | tr '\n' ':')
set -a; source ../odps-spike.env; set +a
unset ODPS_TUNNEL_ENDPOINT ODPS_REGION   # let the SDK auto-resolve from ODPS_ENDPOINT

ADDOPTS=(
  --add-opens=java.base/java.io=ALL-UNNAMED
  --add-opens=java.base/java.lang=ALL-UNNAMED
  --add-opens=java.base/java.lang.reflect=ALL-UNNAMED
  --add-opens=java.base/java.net=ALL-UNNAMED
  --add-opens=java.base/java.nio=ALL-UNNAMED
  --add-opens=java.base/java.nio.charset=ALL-UNNAMED
  --add-opens=java.base/java.util=ALL-UNNAMED
  --add-opens=java.base/java.util.concurrent=ALL-UNNAMED
  --add-opens=java.base/jdk.internal.misc=ALL-UNNAMED
  --add-opens=java.base/sun.nio.ch=ALL-UNNAMED
  -XX:MaxDirectMemorySize=8G
)

TABLE="${1:-yantubi.dim_users_sc_track}"
OUT="${2:-/dev/null}"

java "${ADDOPTS[@]}" -cp "$CP:." ArrowSidecar "$TABLE" > "$OUT" 2>arrow.stderr
rc=$?
echo "exit: $rc"
echo "=== stderr tail ==="
tail -15 arrow.stderr
