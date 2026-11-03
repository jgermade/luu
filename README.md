# luu

A local-first AI agent in Rust, built for the context window you actually have:
7B–32B models with 8K–32K of context, and a remote model only when you name one,
never as a fallback. What it is about is context management — what gets into the
prompt, and what earns its place there.

**[jgermade.github.io/luu](https://jgermade.github.io/luu/)** ·
[demo](https://jgermade.github.io/luu/demo/) ·
[design](luu-design.md) ·
[releases](https://github.com/jgermade/luu/releases)

## Install

```sh
curl -fsSL https://jgermade.github.io/luu/setup.sh | sh
```

Linux (x86_64, arm64; glibc) and macOS (Apple Silicon, Intel). One binary in
`~/.local/bin`, no root, no shell profile edited — when that directory is not on
your `PATH`, the script prints the line to add. The download is checked against
the release's sha256 and run once before it replaces anything.

```sh
# install, or update to the latest release (does nothing if it is already there)
curl -fsSL https://jgermade.github.io/luu/setup.sh | sh

# download and replace it, even the same version
curl -fsSL https://jgermade.github.io/luu/setup.sh | sh -s -- reinstall

# remove the binary; sessions, keys and config.toml stay where they are
curl -fsSL https://jgermade.github.io/luu/setup.sh | sh -s -- uninstall

# remove the binary and the state directory too (asks first)
curl -fsSL https://jgermade.github.io/luu/setup.sh | sh -s -- uninstall --purge

# a machine with no curl, such as a minimal Debian: the same script, with wget
wget -qO- https://jgermade.github.io/luu/setup.sh | sh
```

After an install it asks how luu runs here. **local** is all of the above: for
you, on `127.0.0.1`, no login. **online** is for people who log in — Linux with
systemd, and root through `sudo`. It does five things:

- puts the binary in `/usr/local/bin`;
- makes a system user `luu` whose home `/var/lib/luu` is the state directory;
- writes `/etc/luu/luu.toml`;
- runs `luu setup` for your domain and the first owner, and prints the code
  they log in with;
- starts `luu.service`.

`reinstall` restarts the service, and `uninstall --purge` removes all of it.
Without a terminal, say it:

```sh
curl -fsSL https://jgermade.github.io/luu/setup.sh | sh -s -- install --mode online \
  --origin https://luu.example.com --owner me@example.com
```

luu does not terminate TLS: put a proxy in front, `caddy reverse-proxy --from
luu.example.com --to 127.0.0.1:7878`. See
[`local-or-online`](RECORD/2026-11-03.local-or-online.WIP.md).

| Option | |
| --- | --- |
| `--mode MODE` | `local` (the default) or `online`. Or `LUU_MODE`. |
| `--origin URL`, `--owner EMAIL` | Online: what a browser types, and the first owner. |
| `--dir DIR` | Where the binary goes. Default `~/.local/bin` (`/usr/local/bin` online), or `LUU_INSTALL_DIR`. |
| `--version TAG` | A release such as `v0.1.1`. Default the latest, or `LUU_VERSION`. |
| `--purge` | With `uninstall`: also delete the state directory (`LUU_HOME`, `~/.luu`, `~/.config/luu`). |
| `-y`, `--yes` | Do not ask before deleting; `--purge` needs it when there is no terminal. |
| `LUU_DOWNLOAD_URL` | A directory with `luu-<target>.tar.gz` and `SHA256SUMS`, used instead of GitHub. |

```sh
# a pinned version, somewhere else
curl -fsSL https://jgermade.github.io/luu/setup.sh | sh -s -- install --version v0.1.1 --dir ~/bin

# read it first
curl -fsSL https://jgermade.github.io/luu/setup.sh -o setup.sh && less setup.sh && sh setup.sh
```

From source, on anything without a prebuilt binary:

```sh
cargo install --locked --git https://github.com/jgermade/luu luu
```

### In a container

`luu serve`, with its state and its work tree in volumes, published for amd64
and arm64 from each release (`deploy/Containerfile.release`):

```sh
docker run -d --name luu --restart unless-stopped \
  -v luu-state:/state -v luu-work:/work -p 7878:7878 ghcr.io/jgermade/luu
docker exec luu cat /state/token    # then http://localhost:7878/?token=…
```

[`deploy/compose.yaml`](deploy/compose.yaml) puts Caddy in front for TLS on a
name of your own. The worker image a session runs its commands in is published
beside it as `ghcr.io/jgermade/luu-worker`.

### On Kubernetes

One pod, its two volumes kept on uninstall, behind an Ingress of any class —
[`deploy/helm/luu`](deploy/helm/luu), published as an OCI chart:

```sh
helm install luu oci://ghcr.io/jgermade/charts/luu \
  --set ingress.enabled=true --set ingress.host=luu.example.com --set ingress.tls.enabled=true
kubectl exec deploy/luu -- cat /state/token
```

Sessions run in luu's own pod by default. With `--set runtime=kubernetes`
each session runs in a pod of its own that mounts only its folder; hold the
namespace to Pod Security `restricted` (see
[`a-pod-per-session`](RECORD/2026-11-03.a-pod-per-session.WIP.md)).

Each starts with one identity, the token. To make either one online, with
people who log in, run `luu setup` in it, then restart it:

```sh
docker exec -it luu luu setup && docker restart luu
kubectl exec -it deploy/luu -- luu setup && kubectl rollout restart deploy/luu
```

`luu setup` writes `[server]` for your domain, makes the first owner and
prints their code. More people are `luu user add <email>`, each with a code
from `luu user code <email>`, and each has their own sessions and folder. See
[`a-user-a-code-and-a-folder`](RECORD/2026-11-03.a-user-a-code-and-a-folder.WIP.md)
and [`three-ways-in`](RECORD/2026-10-30.three-ways-in.WIP.md).

## First run

```sh
luu chat "hola"                                   # one turn, mock backend, no model needed
luu chat "hola" --backend ollama \
  --model qwen2.5-coder:7b --context-limit 8192   # a local Ollama
luu chat "hola" --backend openai \
  --openai-url http://127.0.0.1:8080/v1           # llama-server, vLLM, LM Studio
luu serve                                         # the web UI on http://127.0.0.1:7878
luu tools                                         # the resolved sandbox and the exact prefix
```

The first run asks where to keep its state — `~/.luu` or `~/.config/luu` — and
`LUU_HOME` answers without being asked.

## What it does

- **Decide, then render.** The prompt is chosen against a token budget, bucket by
  bucket, and rendered from that choice, so every token sent is attributable.
  History is evicted in whole turns; a closed task folds into a summary.
- **A prefix the cache can keep.** System text and tool definitions stay
  byte-identical across calls, and how much of them survived is reported per turn.
- **Plans you approve.** In `luu serve` a prompt becomes a plan naming what it
  reads, writes and runs, held until you approve it — and the approved plan is
  the sandbox for its task.
- **The kernel holds the child.** The model never executes anything: commands run
  under Landlock and seccomp on Linux, rlimits everywhere, or in a container
  (docker, podman, nerdctl, Apple's `container`). Every verdict says what enforced it.
- **Any backend.** Ollama, anything OpenAI-compatible, anything
  Anthropic-compatible — and llama-server, mlx-serve or Ollama started for you.
- **Sessions that survive.** Stored in SQLite, recorded and replayed in the web UI.

## Working on luu

[`AGENTS.md`](AGENTS.md) is how this repository works — commands, conventions and
the design commitments that are easy to erode. [`luu-design.md`](luu-design.md)
is the design as it stands, [`RECORD/`](RECORD/) is how it got there, and
[`ROADMAP/`](ROADMAP/) is what is planned and not yet true.

```sh
make install   # dependencies: Rust, and the UI's optional ones
make test      # cargo test --workspace
make up        # the web UI on 127.0.0.1:7878
make lint      # what CI runs
```

The landing page and the installer live in [`pages/`](pages/), served as they
are at the root of GitHub Pages with the web UI's replay under `demo/`.
`scripts/setup-check.sh <archive>` walks the installer against a build.
