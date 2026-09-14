#!/usr/bin/env bash
# Times POST /api/sessions against `luu serve` under two postures — host and
# container — interleaved so system drift lands on both arms equally rather
# than favouring whichever ran first. See RECORD/2026-09-08.a-session-picks-its-executor.completed.md
# §Still open and ROADMAP/2026-09-09/README.md item 7.
set -euo pipefail

base="http://127.0.0.1:7879"
n="${N:-10}"
out="${OUT:-.}"

now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }

create() {
  local posture="$1"
  local t0 t1
  t0=$(now_ms)
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$base/api/sessions" \
    -H 'content-type: application/json' \
    -d "{\"posture\":\"$posture\"}")
  t1=$(now_ms)
  if [ "$code" != "200" ] && [ "$code" != "201" ]; then
    echo "posture=$posture http=$code (expected 200/201)" >&2
    exit 1
  fi
  echo $((t1 - t0))
}

echo "warming up both postures once each (not measured — first pull/handshake of the run)"
create host >/dev/null
create container >/dev/null

echo "ms" > "$out/host.csv"
echo "ms" > "$out/container.csv"

for i in $(seq 1 "$n"); do
  h=$(create host)
  c=$(create container)
  echo "$h" >> "$out/host.csv"
  echo "$c" >> "$out/container.csv"
  echo "run $i: host=${h}ms container=${c}ms"
done
