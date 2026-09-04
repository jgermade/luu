# luu Architectural Audit for the Final Containerized Version

**Date:** 2026-09-01\
**Audited repository:** `jgermade/luu`\
**Objective:** assess whether the current architecture can evolve into a
final version in which **all agent command execution takes place inside
a container**, with no direct access from the agent to the host.

> **Important:** this is not a production security audit of the current
> state. The project is explicitly under development, and the current
> server may execute processes without the planned container protection.
> The purpose of this document is to identify architectural decisions
> worth preserving, changing, or preparing before the final hardening
> phase.

------------------------------------------------------------------------

## 1. Executive summary

The current architecture is **compatible with a strongly isolated final
version**, and using the container as an additional layer on top of
Landlock/seccomp is a sound decision.

The design already has several particularly useful properties:

-   the model does not execute commands directly;
-   tools are structured calls interpreted by Rust;
-   commands have an allowlist;
-   paths are checked in code;
-   subprocesses can be subject to kernel restrictions;
-   tasks have a narrower policy than the global policy;
-   human approval happens before a task is executed in `serve`;
-   the design already anticipates the container isolating **tool
    execution**, while the core and model client remain outside.

The main conclusion is:

> **I do not recommend redesigning the agent to introduce containers. I
> recommend introducing an explicit `ExecutionBackend`/`Worker`
> abstraction, so that the current sandbox can become a policy enforced
> inside a containerized worker.**

The target architecture should be:

``` text
                         HOST
                           │
            ┌──────────────┴──────────────┐
            │                              │
        Luu core                       Model backend
        (host)                         (Ollama/llama.cpp)
            │
            │ task + approved policy
            ▼
     ┌───────────────────────┐
     │   Execution Worker    │
     │      CONTAINER        │
     │                       │
     │  tools + commands     │
     │       │               │
     │  Landlock + seccomp   │
     │       │               │
     │   workspace only      │
     └───────────┬───────────┘
                 │
          structured results
                 │
                 ▼
              Luu core
```

The primary final security boundary should not be a list of dangerous
commands. It should be:

> **the worker has no technical path to reach the host outside
> explicitly mounted resources.**

------------------------------------------------------------------------

# 2. Current state vs. final design

The current design document already establishes three levels:

1.  in-process checks;
2.  Landlock + seccomp for subprocesses;
3.  a container above level 2.

It also specifies that the container should isolate tool execution while
the context manager and model client remain outside. This fits a
local-first agent architecture very well.

## Assessment

  ---------------------------------------------------------------------------
  Component         Now               Final target          Assessment
  ----------------- ----------------- --------------------- -----------------
  Agent loop        Host              Host                  🟢

  Context manager   Host              Host                  🟢

  Model client      Host              Host                  🟢

  Tool definitions  Core              Core + worker         🟢

  File tools        In-process        Worker                🟡

  Command execution Local process     Container worker      🟡

  Landlock          Subprocess        Inside worker         🟢

  Seccomp           Subprocess        Inside worker         🟢

  Workspace         Direct host       Explicit mount        🟡
                    access                                  

  Network           Policy            Worker network        🟡
                                      namespace/control     

  WebSocket         Local             Local/authenticated   🟡

  Persistence       Future            Host-side             🟢

  Secrets           Not defined yet   Never mounted by      🔴 Design pending
                                      default               
  ---------------------------------------------------------------------------

------------------------------------------------------------------------

# 3. Most important architectural decision

## The container should be an execution backend, not a wrapper around the entire agent

The current design says that the container "wraps tool execution, not
the whole core".

**I would keep that decision exactly as it is.**

I do not recommend:

``` text
container
 └── entire luu
      ├── model
      ├── context
      ├── UI
      └── tools
```

because it complicates:

-   GPU;
-   Ollama access;
-   persistence;
-   UI;
-   sessions;
-   observability;
-   performance;
-   host communication.

I prefer:

``` text
Host
 └── luu
      ├── planning
      ├── context
      ├── approval
      ├── model
      └── execution backend
             │
             ▼
        container worker
```

This allows the agent to remain independent of the isolation mechanism.

------------------------------------------------------------------------

# 4. Introduce an execution abstraction

In my opinion, this is the highest-value architectural change to make
before final hardening.

Currently the code has a fairly direct relationship:

``` text
Tool
  ↓
Sandbox
  ↓
Rust syscall / subprocess
```

The final version should evolve toward:

``` text
Tool
  ↓
ExecutionBackend
  ↓
┌─────────────────────────────┐
│ LocalWorker                 │
│ ContainerWorker             │
│ Future VMWorker              │
└─────────────────────────────┘
```

Conceptually:

``` rust
trait ExecutionBackend {
    fn read_file(...);
    fn write_file(...);
    fn edit_file(...);
    fn list_dir(...);
    fn run_command(...);
}
```

And:

``` rust
struct ContainerWorker {
    runtime: ContainerRuntime,
    container_id: ...,
    policy: SandboxPolicy,
}
```

The agent should not need to know whether a command was ultimately
executed through:

-   `Command`;
-   Podman;
-   Docker;
-   a VM;
-   another OCI runtime.

This dramatically reduces the cost of implementing final isolation.

------------------------------------------------------------------------

# 5. The permission model must remain independent from the container

A good decision in the current design is:

``` text
luu.toml
   ↓
global policy
   ↓
approved task
   ↓
narrowed sandbox
```

This should be preserved.

The container **must not become the place where permissions are
decided**.

The correct separation is:

``` text
POLICY
  ↓
what it may do

CONTAINER
  ↓
where it may do it

KERNEL SANDBOX
  ↓
which syscalls/filesystem operations can actually be used
```

For example:

``` text
task:
  files = ["src/"]
  writes = ["src/main.rs"]
  commands = ["cargo"]
  network = false
```

becomes:

``` text
Container:
  /workspace/src        mounted
  /workspace/...        not mounted
  network               disabled
  command               cargo
```

and inside the container:

``` text
Landlock:
  /workspace/src → RW
  rest → denied

Seccomp:
  restricted
```

This provides defense in depth.

------------------------------------------------------------------------

# 6. The workspace should be the physical boundary

A final-version design rule should be:

> **Do not mount the entire host project if the task does not need the
> entire project.**

There are two levels:

### Ideal

Mount only the required paths.

``` text
host:
  /project/src
  /project/tests
  /project/Cargo.toml

container:
  /workspace/src
  /workspace/tests
  /workspace/Cargo.toml
```

### Pragmatic

Mount the entire repository:

``` text
/project → /workspace
```

but make sure that:

-   `.env` does not reach the container;
-   credentials do not reach it;
-   `.ssh` does not reach it;
-   `~/.aws` does not reach it;
-   `~/.config` does not reach it;
-   tokens do not reach it;
-   host sockets do not reach it.

The ideal option is to create a temporary **workspace staging area** and
mount that staging area.

------------------------------------------------------------------------

# 7. Do not use `.gitignore` as a security boundary

A future implementation might be tempted to do:

``` text
copy repo
+ respect .gitignore
→ container
```

That is useful functionally, but must not be considered security.

A project may contain:

``` text
.env
credentials.json
secrets/
config/
private-key
```

without being covered by `.gitignore`.

Recommendation:

``` text
host repository
       ↓
workspace materializer
       ↓
explicit inclusion/exclusion policy
       ↓
container
```

The policy could eventually support:

``` toml
[container.workspace]
mode = "staging"

include = [
    "src/**",
    "tests/**",
    "Cargo.toml",
    "Cargo.lock",
]

exclude = [
    ".env*",
    ".git/**",
    "**/credentials*",
    "**/*secret*",
]
```

It does not need to be implemented now, but the conceptual model should
exist.

------------------------------------------------------------------------

# 8. `/etc` is no longer a critical finding in the containerized model

The current sandbox's `SYSTEM_ROOTS` includes:

``` text
/usr
/bin
/sbin
/lib
/lib64
/etc
/opt
```

The code correctly distinguishes these implicit roots from the
permissions given to in-process tools.

In the final containerized context, this is much less concerning
because:

``` text
/etc
```

will be the container's `/etc`, not the host's.

Therefore:

> **I do not consider `/etc` something that should simply be removed for
> host-security reasons.**

The important requirement is to ensure that the container does not
receive:

``` text
/etc/host-mounted-secret
```

or:

``` text
/etc
```

from the host through a bind mount.

------------------------------------------------------------------------

# 9. The command policy must survive the container boundary

The current allowlist:

``` toml
commands = ["cargo", "git"]
```

is a good layer.

In the final version I recommend three dimensions:

``` text
1. command allowed by policy
2. executable available in the image
3. command allowed by the task
```

For example:

``` text
global:
  cargo
  rustc
  git

task:
  cargo
  git
```

The runtime should verify:

``` text
requested command
      ↓
global allowlist
      ↓
task allowlist
      ↓
image manifest
      ↓
execute
```

This avoids ambiguity such as:

``` text
policy says cargo
but image does not contain cargo
```

The current design already identifies this possibility and proposes that
the image contain the authorized programs.

------------------------------------------------------------------------

# 10. Do not turn `commands` into a host-security allowlist

Once inside the container, there is no need to try to block:

``` text
rm
curl
python
bash
```

because they are "dangerous" by themselves.

If the policy allows:

``` text
bash
```

the container should be the security boundary.

This is important.

An approach based on:

``` text
"block rm"
"block curl"
"block chmod"
```

is fragile.

The correct approach is:

``` text
agent
  ↓
container
  ↓
host isolation
```

not:

``` text
agent
  ↓
infinite list of dangerous commands
  ↓
host
```

------------------------------------------------------------------------

# 11. Seccomp should remain enabled inside the container

The current design proposes keeping Landlock + seccomp even with a
container.

I agree.

The final chain should be:

``` text
Host
  │
  └── container runtime
        │
        └── Worker
              │
              ├── Landlock
              ├── seccomp
              └── command
```

This protects against:

-   worker bugs;
-   build scripts;
-   child processes;
-   tools launching other tools;
-   unexpected compiler behavior;
-   future expansion of the command allowlist.

Do not assume:

> "Docker already protects everything."

The container is a strong boundary, but additional layers ensure that a
failure in one layer does not compromise the whole model.

------------------------------------------------------------------------

# 12. The container runtime is part of the TCB

The final version should explicitly document what is considered the
Trusted Computing Base.

For example:

``` text
TCB:
  luu host
  container runtime
  host kernel
  image
  worker
```

But:

``` text
NOT TCB:
  model
  generated code
  build scripts
  repository contents
  npm/cargo scripts
  shell scripts
```

This matters because the model can produce arbitrary code.

The design should assume:

> **Agent-generated content is hostile.**

Even when the model is completely benign, a repository can contain:

``` text
build.rs
Makefile
package.json scripts
Cargo build scripts
setup.py
shell scripts
git hooks
```

All of these must be treated as untrusted code.

------------------------------------------------------------------------

# 13. Recommended container hardening

For the final implementation, establish a baseline such as:

``` text
rootless runtime
non-root user
cap-drop=ALL
no-new-privileges
read-only root filesystem
tmpfs /tmp
network=none by default
no Docker socket
no Podman socket
no host PID namespace
no host network
no host IPC
no devices unless explicitly required
```

And review:

``` text
seccomp profile
AppArmor/SELinux
user namespaces
cgroups
CPU quota
memory limit
process/PID limit
disk quota
timeout
```

Especially:

### Never

``` text
-v /var/run/docker.sock:/var/run/docker.sock
```

or equivalent.

Giving the agent access to the runtime socket effectively turns it into
an administrator of the host.

------------------------------------------------------------------------

# 14. The worker should have resource limits

The sandbox must not only be a filesystem boundary.

An agent can produce:

``` text
fork bomb
memory bomb
disk bomb
CPU loop
infinite compiler
gigantic output
```

Therefore:

``` text
Task
  ↓
Container
  ├── CPU
  ├── RAM
  ├── PIDs
  ├── disk
  ├── timeout
  └── output
```

The existing 8 KiB tool-output limit is a good first layer, but it does
not replace resource limits.

------------------------------------------------------------------------

# 15. Separate `network` permission from container networking

The current policy has:

``` toml
network = false
```

and seccomp controls certain socket operations.

For the final version I recommend explicit semantics:

``` text
task network = false
    ↓
container network namespace without external network
```

and:

``` text
task network = true
    ↓
network backend
    ↓
possibly restricted egress
```

Do not rely exclusively on seccomp to represent network isolation.

Seccomp can block certain ways of creating sockets, but it is not
equivalent to a network namespace.

------------------------------------------------------------------------

# 16. Future `network = true` deserves its own policy

This will probably be one of the most important pending design points.

It should not simply mean:

``` text
network = true
```

because that means:

``` text
curl https://anything
```

and the agent could exfiltrate:

``` text
source code
environment
accidentally mounted tokens
private repository data
```

A future policy should support something like:

``` text
network:
  mode = "none"

network:
  mode = "allowlist"
  hosts = [
      "crates.io",
      "github.com",
  ]

network:
  mode = "full"
```

And "full" should be deliberately explicit.

------------------------------------------------------------------------

# 17. The approval model is well designed

The current design has a valuable property:

``` text
prompt
 ↓
plan
 ↓
human approval
 ↓
task sandbox
 ↓
execution
```

The approved plan becomes the narrowed policy for that task.

This should remain when the container is introduced.

Do not implement:

``` text
approve task
 ↓
start generic privileged container
```

Instead:

``` text
approve task
 ↓
resolve policy
 ↓
build WorkerSpec
 ↓
create/reuse container
 ↓
apply task restrictions
 ↓
execute
```

------------------------------------------------------------------------

# 18. Pending issue: `writes` vs. `run_command`

The design already raises the question of whether `writes` should also
constrain `run_command`.

For the final version, the answer should be:

## Yes.

If a task says:

``` text
writes = ["src/main.rs"]
commands = ["cargo"]
```

then `cargo` should not be able to write arbitrarily to:

``` text
target/
Cargo.lock
/tmp
```

unless those locations are required and declared.

There is a difference between:

``` text
tool-level permission
```

and:

``` text
process-level permission
```

The process should receive the same filesystem policy as the task.

The current implementation already moves in this direction by applying
the narrowed policy to the child. This should be preserved.

------------------------------------------------------------------------

# 19. The container should receive the narrowed policy, not the global policy

This should become an architectural test:

``` text
SandboxPolicy(global)
        ↓
Plan::narrow()
        ↓
TaskSandbox
        ↓
ContainerSpec
```

Never:

``` text
SandboxPolicy(global)
        ↓
Container
        ↓
Task restrictions only in Rust
```

Otherwise a child process could bypass the agent's logical task
restriction.

The restriction must reach the kernel/container.

------------------------------------------------------------------------

# 20. Container reuse: yes, but carefully

The design proposes one container per session rather than one per
command.

I agree for performance reasons.

But this means:

``` text
task 1
  ↓
filesystem state
  ↓
task 2
```

The container is stateful.

Therefore, the semantics of a completed task need to be explicit.

### Option A --- intentional persistence

``` text
same container
same workspace
next task sees previous changes
```

This is the most natural option for a coding agent.

### Option B --- isolation per task

``` text
container per task
```

More secure, more expensive.

### Recommendation

Keep:

``` text
one container per session
```

but apply **task policy** dynamically and provide an explicit:

``` text
reset / destroy / recreate
```

operation when a session is considered compromised.

------------------------------------------------------------------------

# 21. The worker should be restartable

The core should assume the worker can die:

``` text
OOM
timeout
panic
container killed
runtime failure
```

Therefore:

``` text
Agent core
   │
   ├── create worker
   ├── execute
   ├── monitor
   ├── detect death
   └── recreate
```

The worker should not contain essential conversation state.

Important state should remain in the host/core.

This fits the current `Session`, `Context`, and `Task` design.

------------------------------------------------------------------------

# 22. Host ↔ worker protocol

I recommend not allowing the container to connect arbitrarily to the
host.

Instead of:

``` text
container → host network → API
```

prefer:

``` text
host
 │
 │ owns connection
 ▼
worker stdin/stdout
```

or a carefully managed Unix socket.

Ideally:

``` text
Host creates worker
Host owns transport
Worker never initiates arbitrary host connections
```

This reduces the exfiltration surface and makes the data flow much
easier to reason about.

------------------------------------------------------------------------

# 23. Never expose the runtime socket to the worker

This should have an explicit regression test.

Forbidden inside the worker:

``` text
DOCKER_HOST
CONTAINER_HOST
/var/run/docker.sock
/var/run/podman/podman.sock
```

Also review:

``` text
DOCKER_TLS_VERIFY
DOCKER_CERT_PATH
KUBECONFIG
AWS_CONTAINER_CREDENTIALS_FULL_URI
```

and equivalents.

------------------------------------------------------------------------

# 24. Secrets: likely the biggest future risk after the container boundary

The final version should assume:

> **The model can read anything inside the container.**

Therefore, do not introduce by default:

``` text
GITHUB_TOKEN
AWS_ACCESS_KEY_ID
OPENAI_API_KEY
SSH_AUTH_SOCK
```

unless there is an explicit need.

If a task needs authentication, prefer:

``` text
host-side credential broker
```

over:

``` text
secret mounted into container
```

For example:

``` text
agent
  ↓
"git push"
  ↓
host broker
  ↓
credential
```

rather than:

``` text
container
  ↓
GITHUB_TOKEN=...
```

------------------------------------------------------------------------

# 25. Git deserves a specific policy

`git` looks harmless, but it can:

``` text
git push
git fetch
git clone
git config
git credential
```

If network and credentials are available, `git` can become a direct
exfiltration path.

Future policy should distinguish:

``` text
git read operations
git write operations
```

Potentially even:

``` text
commands = ["git-read"]
```

with dedicated tools for remote write operations.

This does not need to be solved now, but avoid designing around the
assumption that:

``` text
git = filesystem tool
```

------------------------------------------------------------------------

# 26. The local server must also respect the boundary

The design currently uses:

``` text
127.0.0.1:7878
```

with no auth for localhost and bearer-token auth when exposed on another
address.

This should become an enforced guarantee, not just documentation.

Rule:

``` text
bind = loopback
    → no auth

bind != loopback
    → refuse unless auth is configured
```

Do not merely emit a warning.

An agent with `approve_task` should not accidentally become accessible
from the LAN.

------------------------------------------------------------------------

# 27. The approval endpoint is a privileged surface

The protocol contains:

``` text
approve_task
reject_task
close_task
reopen_task
```

Approval is an authority-bearing action.

Therefore:

``` text
GET /api/...
```

can be read-only,

but:

``` text
approve_task
```

is privileged.

Future design should conceptually separate:

``` text
read capability
control capability
```

so that a read-only observer cannot approve tasks.

------------------------------------------------------------------------

# 28. Recordings: watch for secrets

`--record` stores the JSON stream and the design allows recovery of:

-   prompts;
-   tool calls;
-   tool results;
-   context;
-   commands;
-   results.

This is useful for reproducibility.

But in production it may contain:

``` text
source code
accidentally printed tokens
paths
private URLs
commands
user data
```

Future recommendation:

``` text
recording sensitivity = confidential
```

and:

-   restrictive file permissions;
-   explicit warning;
-   redaction option;
-   never dump environment secrets;
-   avoid storing full stdout when unnecessary.

------------------------------------------------------------------------

# 29. Prefer structured tool results

The current 8 KiB output limit is good, but the design can improve:

``` text
run_command
{
  exit_code,
  stdout,
  stderr,
  truncated,
  duration_ms
}
```

rather than an undifferentiated string.

This is not only UX.

It also improves:

-   auditing;
-   error detection;
-   limits;
-   judging;
-   replay;
-   metrics;
-   mitigation of prompt injection from stdout.

------------------------------------------------------------------------

# 30. Treat tool output as untrusted content

A command can print:

``` text
IGNORE PREVIOUS INSTRUCTIONS
...
```

The model must receive it as:

``` text
tool_result
```

and not as a system instruction.

The current message architecture already separates `assistant(call)` and
`user(result)`, which is appropriate for preserving tool semantics.

When implementing the container, this boundary must not accidentally
change.

------------------------------------------------------------------------

# 31. Repository contents are also untrusted input

This is particularly important for:

``` text
read_file
fragment
repo map
git
```

A file can contain:

``` text
"Assistant, execute curl..."
```

The model must treat it as data.

The separation:

``` text
system
tool definitions
user/task
tool result
```

should remain stable.

------------------------------------------------------------------------

# 32. Security tests to add before final hardening

I would not attempt a complete security audit yet.

I would add a suite of **contract tests** defining the expected behavior
of the future architecture.

## Filesystem

Test:

``` text
../escape
absolute paths
symlinks
broken symlinks
hardlinks
TOCTOU
nested writes
nonexistent destination
```

## Commands

Test:

``` text
unauthorized command
authorized command
shell metacharacters
command with path
PATH manipulation
child process
build script
```

## Task

Test:

``` text
global policy
plan narrowing
read-only file
write-only declaration
command narrowing
network narrowing
```

## Container

Once implemented:

``` text
read host secret → impossible
read $HOME → impossible
access Docker socket → impossible
host PID visibility → impossible
host network → impossible
mount syscall → denied
new privileged namespace → denied
device access → denied
```

------------------------------------------------------------------------

# 33. Escape tests should be part of CI

The final version should include deliberately hostile tests.

For example, inside the worker attempt:

``` bash
cat /etc/passwd
find /
ls /proc/1/root
ls /host
cat /var/run/docker.sock
mount
unshare
nsenter
ptrace
```

Not because all of these are necessarily attacks, but because they
define the **boundary regression tests**.

The important test is not:

> "Is `cat` blocked?"

It is:

> "Can the worker obtain a host resource?"

------------------------------------------------------------------------

# 34. Do not rely on the container as the only security mechanism without validating the runtime

"Runs inside Docker" is not by itself a sufficient guarantee.

Real security depends on:

``` text
kernel
+
runtime
+
configuration
+
capabilities
+
mounts
+
namespaces
+
seccomp
+
AppArmor/SELinux
```

The final configuration should be versioned and tested.

Ideally:

``` text
container profile
Dockerfile
runtime arguments
seccomp profile
AppArmor profile
```

are all part of the project.

------------------------------------------------------------------------

# 35. Reproducible OCI image

The worker image should be:

``` text
versioned
reproducible
minimal but functional
```

and should contain exactly the tools allowed by policy.

For example:

``` text
luu-worker:rust-1
  cargo
  rustc
  git
  rg
  ls
```

rather than:

``` text
ubuntu:latest
```

with hundreds of utilities that have not been considered.

There is no need to make the image "distroless"; the design correctly
recognizes that the agent needs a toolchain.

The key is that it is **controlled and versioned**.

------------------------------------------------------------------------

# 36. Do not automatically update the image during a session

Avoid:

``` text
apt update
apt upgrade
```

as part of the agent's automatic behavior.

The image should be a declared dependency:

``` text
worker image version
```

and updates should be a host-side decision.

This improves:

-   reproducibility;
-   debugging;
-   security;
-   recordings;
-   comparison between sessions.

------------------------------------------------------------------------

# 37. Docker/Podman compatibility

The decision to generate an OCI image and allow the user to choose the
runtime is sound.

Keep:

``` text
ContainerRuntime
```

as an interface.

For example:

``` text
DockerRuntime
PodmanRuntime
```

but avoid constructing shell commands by concatenating strings.

The conceptual equivalent should be:

``` rust
Command::new(runtime)
    .args([...])
```

with structured arguments.

------------------------------------------------------------------------

# 38. The runtime must remain host-only

An important principle:

``` text
Host:
    container runtime

Container:
    worker
```

The worker should never have access to the runtime.

This prevents a vulnerability inside the worker from immediately
becoming:

``` text
container escape → runtime API → host root
```

------------------------------------------------------------------------

# 39. Future persistence

When SQLite is added, keep:

``` text
Host:
  session DB
  event log
  task state
```

and:

``` text
Container:
  workspace state
```

separate.

The container should not be the agent's database.

This fits the current principle that a session can be reconstructed from
events.

------------------------------------------------------------------------

# 40. Recommended final architecture

My concrete proposal is:

``` text
┌──────────────────────────────────────────────────────┐
│ HOST                                                 │
│                                                      │
│  ┌──────────────┐        ┌────────────────────────┐  │
│  │ Luu Core     │        │ Model Backend          │  │
│  │              │───────▶│ Ollama / llama.cpp     │  │
│  │ Context      │        └────────────────────────┘  │
│  │ Tasks        │                                    │
│  │ Approval     │                                    │
│  │ Policy       │                                    │
│  │ Protocol     │                                    │
│  └──────┬───────┘                                    │
│         │                                             │
│         │ WorkerSpec                                 │
│         ▼                                             │
│  ┌───────────────────────────────────────────────┐    │
│  │ Container Runtime                             │    │
│  │                                               │    │
│  │  rootless / non-root                          │    │
│  │  cap-drop=ALL                                 │    │
│  │  no-new-privileges                            │    │
│  │  read-only rootfs                             │    │
│  │  network none                                 │    │
│  │                                               │    │
│  │   ┌───────────────────────────────────────┐   │    │
│  │   │ Luu Worker                            │   │    │
│  │   │                                       │   │    │
│  │   │ Tools                                 │   │    │
│  │   │   ↓                                   │   │    │
│  │   │ Task Sandbox                          │   │    │
│  │   │   ↓                                   │   │    │
│  │   │ Landlock + seccomp                    │   │    │
│  │   │   ↓                                   │   │    │
│  │   │ commands                              │   │    │
│  │   └───────────────────────────────────────┘   │    │
│  └───────────────────────────────────────────────┘    │
│                                                      │
└──────────────────────────────────────────────────────┘
```

------------------------------------------------------------------------

# 41. Recommended implementation order

I do not recommend stopping development now to implement all hardening.

I would follow this sequence:

## Phase A --- now

### 1. Keep the current model

Finish:

-   agent loop;
-   tasks;
-   protocol;
-   tools;
-   context management;
-   model backends.

### 2. Introduce `ExecutionBackend`

Initially the implementation can simply be:

``` text
LocalExecutionBackend
```

This prepares the important change without adding Docker yet.

### 3. Make `TaskSandbox` an independent specification

Conceptually:

``` text
TaskSandboxSpec
  paths
  writes
  commands
  network
  enforcement
```

------------------------------------------------------------------------

# 42. Phase B --- container MVP

Implement:

``` text
ContainerExecutionBackend
```

with:

``` text
OCI image
rootless
non-root
cap-drop=ALL
no-new-privileges
network=none
workspace mount
timeout
memory limit
CPU limit
PID limit
```

And keep Landlock/seccomp.

Do not attempt yet:

-   network proxy;
-   secrets broker;
-   multi-host;
-   VM;
-   Kubernetes.

First demonstrate:

``` text
agent → task → container → command → result
```

reliably.

------------------------------------------------------------------------

# 43. Phase C --- hardening

Then:

### Filesystem

-   staging;
-   explicit mounts;
-   openat2;
-   symlink/hardlink tests.

### Network

-   network namespace;
-   default deny;
-   egress allowlist.

### Runtime

-   rootless;
-   capabilities;
-   seccomp;
-   AppArmor/SELinux;
-   cgroups;
-   devices.

### Secrets

-   no environment secrets;
-   no host credentials;
-   credential broker if needed.

### Protocol

-   auth for non-loopback;
-   read/control separation;
-   approval authorization.

------------------------------------------------------------------------

# 44. Phase D --- adversarial testing

Create a suite whose goal is to break:

``` text
agent
 ↓
tool
 ↓
worker
 ↓
host
```

Not just unit tests.

Cases:

``` text
prompt injection
malicious repository
malicious build.rs
malicious Cargo build script
malicious npm script
symlink attack
hardlink attack
fork bomb
memory exhaustion
disk exhaustion
network exfiltration
container runtime access
/proc escape
/dev access
namespace manipulation
credential discovery
```

------------------------------------------------------------------------

# 45. Final priorities

## P0 --- required before production

-   [ ] All execution occurs in a container.
-   [ ] The container cannot access the runtime socket.
-   [ ] Worker runs as non-root.
-   [ ] `cap-drop=ALL`.
-   [ ] `no-new-privileges`.
-   [ ] Network disabled by default.
-   [ ] Only explicit resources are mounted.
-   [ ] No host credentials by default.
-   [ ] CPU/RAM/PID/disk/time limits.
-   [ ] Landlock/seccomp remain active inside the worker.
-   [ ] Escape tests exist.
-   [ ] Non-loopback server bindings require authentication.
-   [ ] Task policy actually reaches the worker.

## P1 --- strongly recommended

-   [ ] `ExecutionBackend`.
-   [ ] Workspace staging.
-   [ ] openat2.
-   [ ] Egress allowlist.
-   [ ] Versioned OCI image.
-   [ ] Versioned runtime profile.
-   [ ] Parser/path-resolver fuzzing.
-   [ ] Secure/redacted recordings.
-   [ ] Structured tool results.

## P2 --- future evolution

-   [ ] Credential broker.
-   [ ] VM/microVM backend.
-   [ ] Remote workers.
-   [ ] Multi-session.
-   [ ] Network proxy.
-   [ ] Automated judge.

------------------------------------------------------------------------

# 46. Verdict

### Architecture: 🟢 APPROVED WITH RECOMMENDED CHANGES

The decision that:

> **the agent core remains outside and tool execution moves to a
> containerized worker**

is, in my view, the right direction.

The current architecture already has the necessary conceptual pieces:

``` text
Task
Policy
Sandbox
Tools
Execution
Model
Protocol
```

The main remaining work is not to change the agent, but to **make the
boundary between "deciding to execute" and "where execution happens"
explicit**.

Therefore, the most important recommendation is:

> **Introduce `ExecutionBackend` before implementing the container.**

The evolution then becomes:

``` text
v0 development
    ↓
LocalExecutionBackend
    ↓
functional validation
    ↓
ContainerExecutionBackend
    ↓
hardening
    ↓
production
```

without having to rewrite `agent-core`.

------------------------------------------------------------------------

# 47. Design contract checklist

Before considering container mode complete:

``` text
[ ] The model never executes directly.
[ ] Every ToolCall passes through validation.
[ ] Every task produces a narrowed policy.
[ ] The global policy is only the upper bound.
[ ] The worker receives the task policy.
[ ] The worker never receives host credentials by default.
[ ] The worker never receives the runtime socket.
[ ] The worker never shares the host PID namespace.
[ ] The worker never uses host networking by default.
[ ] The mounted workspace is explicit.
[ ] The root filesystem is immutable.
[ ] The process is non-root.
[ ] Capabilities are removed.
[ ] no-new-privileges is enabled.
[ ] Landlock/seccomp remain active.
[ ] CPU/RAM/PID/disk/time are limited.
[ ] The worker can be destroyed and recreated.
[ ] The session does not depend on worker-internal state.
[ ] A process escape does not imply host access.
[ ] Tests explicitly attempt to escape.
```

------------------------------------------------------------------------

## Sources reviewed

-   Repository: `https://github.com/jgermade/luu`
-   Design: `luu-design.md`
-   Sandbox: `crates/agent-core/src/sandbox/mod.rs`
-   Server: `crates/luu/src/serve.rs`

The current repository explicitly describes the container as level 3 of
isolation and specifies that it should be applied **on top of
Landlock/seccomp**, isolating tool execution while the core and model
client remain on the host. It also documents `network=none`, limited
bind mounts, and a Linux worker inside an OCI image.
