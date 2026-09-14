# Evidence: what a container start costs, 2026-09-15

The transcripts and derived numbers behind
[`../../2026-09-15.what-a-container-start-costs.completed.md`](../../2026-09-15.what-a-container-start-costs.completed.md).
Twenty interleaved pairs of `POST /api/sessions`, one posture apart —
`host` (`luu.toml`, no `[worker]`, the default host worker) against
`container` (`luu.container.toml`, `[worker] runtime = "docker"`) — against a
real `luu serve`, a real Docker Desktop, and the real image, on this machine
(machine 1 of `ROADMAP/2026-09-09/machines.md`: M1 Pro, 16 GB, macOS).

```sh
docker build -t luu-worker:dev -f Containerfile .
cargo build --release --bin luu

cat > config.toml <<'TOML'
[posture.host]
policy = "luu.toml"

[posture.container]
policy = "luu.container.toml"
TOML

LUU_HOME="$(pwd)" target/release/luu serve --backend mock --no-store \
  --bind 127.0.0.1:7879 &

N=20 OUT=. ./run.sh   # this directory's script
```

`run.sh` times `POST /api/sessions {"posture": "…"}` with `date`-precision
wall clock (`time.time()*1000` in Python, called once per request rather than
trusting curl's own `%{time_total}` to agree with it), one **host** call then
one **container** call per iteration — interleaved rather than blocked, so a
warm-up effect or thermal drift lands on both arms equally instead of
favouring whichever ran first. `--no-store` keeps SQLite off the constant both
arms pay, since the flag is only about the worker. One untimed warm-up pair
runs before the twenty that are.

**Confirmed with `docker events` (not saved — a side check, not part of the
run) that every `container` call is a real `create`/`attach`/`start` of a new
container, and the one it replaced a real `die`/`destroy`, never a cached
reuse:** `create_session`'s `previous.shutdown().await` ends the old agency
before the handler returns, which is what the `container` arm's number
includes and the `host` arm's does not (a host worker's shutdown is
in-process and near-free).

`host.csv` and `container.csv` are the twenty raw millisecond readings each,
one per line under an `ms` header. `stats.json` is `mean`, `median`, `stdev`,
`min`, `max` and the raw list for both arms, plus `delta_mean_ms` and
`delta_median_ms` — computed with `python3 -c` directly against the two CSVs,
reproducible by rerunning the command the record's own body shows.
