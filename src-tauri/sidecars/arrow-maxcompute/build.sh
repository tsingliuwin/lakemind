#!/bin/bash
# Build the thin arrow-maxcompute-sidecar.jar (just ArrowSidecar.class). The
# heavy odps-sdk-core (shaded, contains com.aliyun.odps.* + shaded Arrow) is
# NOT bundled — it's resolved at runtime via the driver classpath from
# `external::paths::SidecarPaths::driver_jars`.
#
# Prerequisite: the odps-sdk-core shaded jar must be present in ~/.dbx/maven
# (fetched once by the dbx maven-resolver for com.aliyun.odps:odps-jdbc).
set -e
cd "$(dirname "$0")"

SDK=$(find ~/.dbx/maven -name 'odps-sdk-core-*-shaded.jar' 2>/dev/null | head -1)
if [ -z "$SDK" ]; then
  echo "!! odps-sdk-core shaded jar not found in ~/.dbx/maven." >&2
  echo "   Fetch it first via the dbx maven-resolver, e.g.:" >&2
  echo "   ~/.dbx/maven/.../dbx-maven-resolver com.aliyun.odps:odps-jdbc:3.9.3" >&2
  echo "   (or run spike/arrow-sidecar/run.sh once, which resolves it)" >&2
  exit 1
fi

# PartitionSpec lives in odps-sdk-commons (a transitive dep of odps-jdbc).
COMMONS=$(find ~/.dbx/maven -name 'odps-sdk-commons-*-public.jar' 2>/dev/null | head -1)
CP="$SDK"
if [ -n "$COMMONS" ]; then
  CP="$SDK:$COMMONS"
  echo "[build] SDK classpath: $CP"
else
  echo "[build] SDK classpath: $CP (commons jar not found — partition support disabled)"
fi

javac -cp "$CP" ArrowSidecar.java
jar cf arrow-maxcompute-sidecar.jar ArrowSidecar.class
rm -f ArrowSidecar.class
echo "[build] built: $(pwd)/arrow-maxcompute-sidecar.jar"
