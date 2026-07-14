#!/bin/bash
# Concurrency test: K parallel Arrow download sessions on DISJOINT row windows.
#   Usage: ./concurrency.sh <K> <window_rows> [table]
# Measures aggregate rows/sec and speedup vs single-session (~17382).
set -u
cd "$(dirname "$0")"
CP=$(find ~/.dbx/maven -name '*.jar' | tr '\n' ':')
set -a; source ../odps-spike.env; set +a
unset ODPS_TUNNEL_ENDPOINT ODPS_REGION
TABLE="${3:-yantubi.dim_users_sc_track}"
K="${1:-5}"
W="${2:-2000000}"

ADDOPTS=(--add-opens=java.base/java.io=ALL-UNNAMED --add-opens=java.base/java.lang=ALL-UNNAMED --add-opens=java.base/java.lang.reflect=ALL-UNNAMED --add-opens=java.base/java.net=ALL-UNNAMED --add-opens=java.base/java.nio=ALL-UNNAMED --add-opens=java.base/java.nio.charset=ALL-UNNAMED --add-opens=java.base/java.util=ALL-UNNAMED --add-opens=java.base/java.util.concurrent=ALL-UNNAMED --add-opens=java.base/jdk.internal.misc=ALL-UNNAMED --add-opens=java.base/sun.nio.ch=ALL-UNNAMED -XX:MaxDirectMemorySize=4G)

echo "[conc] K=$K  window=$W  table=$TABLE"
wall_start=$(python3 -c 'import time;print(time.time())')
pids=()
for ((i=0; i<K; i++)); do
  start=$(( i * W ))
  java "${ADDOPTS[@]}" -cp "$CP:." ArrowSidecar "$TABLE" "$start" "$W" > /dev/null 2>conc.$i.stderr &
  pids+=($!)
  echo "  worker $i: start=$start count=$W pid=${pids[$i]}"
done
for p in "${pids[@]}"; do wait "$p"; done
wall_end=$(python3 -c 'import time;print(time.time())')

python3 - "$wall_start" "$wall_end" "$K" <<'PY'
import sys, re, glob
ws, we, K = float(sys.argv[1]), float(sys.argv[2]), int(sys.argv[3])
wall = we - ws
total = 0; max_dl = 0.0
print("\n[conc] per-worker:")
for i in range(K):
    fn = f"conc.{i}.stderr"
    txt = open(fn).read() if __import__('os').path.exists(fn) else ""
    m = re.search(r'\[arrow\] DONE.*rows=(\d+).*elapsed=(\d+)ms.*->\s+([\d.]+)\s+rows/sec', txt)
    if m:
        rows, ms, rps = int(m.group(1)), int(m.group(2))/1000.0, float(m.group(3))
        total += rows; max_dl = max(max_dl, ms)
        print(f"  worker {i}: rows={rows} dl={ms:.2f}s ({rps:.0f} rows/s)")
    else:
        last = [l for l in txt.splitlines() if l.strip()][-3:]
        print(f"  worker {i}: NO DONE line. tail: {last}")
agg_wall = total / wall if wall>0 else 0
agg_dl   = total / max_dl if max_dl>0 else 0
print(f"\n[conc] RESULT: workers={K} total_rows={total} wall={wall:.2f}s")
print(f"[conc]   aggregate (by wall)  = {agg_wall:.0f} rows/sec")
print(f"[conc]   aggregate (by max dl)= {agg_dl:.0f} rows/sec   (excludes JVM startup / session-create, parallelized)")
print(f"[conc]   speedup vs single(17382): {agg_wall/17382:.2f}x (wall)  {agg_dl/17382:.2f}x (max-dl)")
PY