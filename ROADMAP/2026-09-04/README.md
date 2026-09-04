# Roadmap — revision 2026-09-04

**What this is:** the order of work as it stands on this date, and what blocks
what. Not a decision and not a description of the tree — for *what is true today*
read [`luu-design.md`](../../luu-design.md), and for *why* read the dated file
in [`RECORD/`](../../RECORD/) each item links to.

Supersedes [`ROADMAP/2026-09-01/`](../2026-09-01/) wholesale. Over three days,
almost the entire sequence of that revision has landed, and the vocabulary of the
project was clarified: **tasks** are model plan checklists, while the bounded
operational container is a **job** (Protocol v5, Record format 7).

## What landed since 2026-09-01

| | |
| --- | --- |
| **OpenAI-compatible backend** | Wire test passing, supports local `llama-server`, vLLM, and remote hosted endpoints — [`an-openai-compatible-backend`](../../RECORD/2026-09-01.an-openai-compatible-backend.completed.md) |
| **Level 3 container observed live** | `luu-worker:dev` built and run in Docker Desktop VM on Apple Silicon with Landlock ABI v8, seccomp, and rlimits enforcing — [`the-container-observed`](../../RECORD/2026-09-03.the-container-observed.completed.md) |
| **Closing on an exit code** | A plan carries `closes_on`; the task folds itself when the command exits 0, reporting closing authority on the wire — [`closing-on-an-exit-code`](../../RECORD/2026-09-02.closing-on-an-exit-code.completed.md) |
| **Sessions in SQLite & Resume** | Sessions persist to SQLite whole; `SessionStore::resume` restores write-side context, turns, and fold summaries — [`sessions-in-sqlite`](../../RECORD/2026-09-02.sessions-in-sqlite.completed.md), [`session-resume`](../../RECORD/2026-09-04.session-resume.completed.md) |
| **Protocol over stdio** | `luu stdio` speaks line-oriented NDJSON over stdin/stdout, unblocking subprocess-based editors without open ports — [`protocol-over-stdio`](../../RECORD/2026-09-04.protocol-over-stdio.completed.md) |
| **Network narrowing per plan** | A plan carries optional `network: bool`, verified by worker Landlock and socket checks — [`network-per-plan`](../../RECORD/2026-09-04.network-per-plan.completed.md) |
| **The Gate Probe completed** | 15-prompt corpus run against `qwen2.5-coder:7b` in container via Playwright; 100% declaration rate, 53% amend rate — [`the-gate-probe`](../../RECORD/2026-08-31.the-gate-probe.completed.md) |
| **Relevance selection probe** | Tested PageRank on a neutral 38-file corpus; rejected (12.5% accuracy vs 100% path order baseline, history evictions). Path order remains baseline — [`the-map-order-probe`](../../RECORD/2026-09-03.the-map-order-probe.completed.md) |
| **From Tasks to Jobs** | Resolved vocabulary clash: `Job` is the bounded execution container; `tasks` is the model's checklist in `plan.tasks` — [`from-tasks-to-jobs`](../../RECORD/2026-09-04.from-tasks-to-jobs.completed.md) |
| **Narrowing phase 2: Egress through the host** | Host CONNECT proxy filters outbound traffic by destination hostname/wildcard when `network: true` — [`egress-through-the-host`](../../RECORD/2026-09-04.egress-through-the-host.completed.md) |
| **The handshake and signed approvals** | Federation stage 1, items 1–2: the client says what it speaks and is refused out loud on a mismatch; `approve_job` carries an Ed25519 signature over the grant, and `job_approved` says who approved — [`signed-approvals`](../../RECORD/2026-09-04.signed-approvals.completed.md) |

---

## The order

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 1 | ~**VSCode extension (Surface #3)** — TypeScript extension spawning `luu stdio`, rendering chat, job gate approval/amendment, and tool stream~ | nothing | [`vscode-extension`](../../RECORD/2026-09-04.vscode-extension.completed.md) |
| 2 | ~**Narrowing phase 2: Egress through the host** — proxy/filter outbound traffic when `network: true`, restricting destinations~ | nothing | [`egress-through-the-host`](../../RECORD/2026-09-04.egress-through-the-host.completed.md) |
| 3 | ~**Multi-session in `serve` & UI** — session switching, creation, and resume of stored sessions in the web UI~ | nothing | [`multi-session-in-serve`](../../RECORD/2026-09-04.multi-session-in-serve.completed.md) |
| 4 | ~**Relevance selection: In-degree and non-greedy fill** — alternative ranking avoiding PageRank's entry-point penalty and oversized file traps~ | nothing | [`in-degree-and-fill`](../../RECORD/2026-09-04.in-degree-and-fill.completed.md) |
| 5 | **Fleet measurement across target machines** — benchmark matrix against local hardware platforms | 1, 3 | [`machines.md`](machines.md) |
| 6 | ~**Federation stage 1, items 1–2** — a versioned handshake, and approvals signed with a key no relay holds~ | nothing | [`signed-approvals`](../../RECORD/2026-09-04.signed-approvals.completed.md) |
| 7 | **Federation stage 1, items 3–4** — transfer ships the record stream rather than a snapshot, and an imported job that is not `Closed` returns to the destination's gate | 6 | [`federation.md`](../2026-08-31/federation.md) |
| 8 | **The transfer probe** — two machines on a LAN, a session moved, an open job re-approved on arrival; the decision on whether the portal earns a place waits on it | 7 | [`federation.md`](../2026-08-31/federation.md) |

```mermaid
gantt
    title The order as of 2026-09-04
    dateFormat YYYY-MM-DD
    axisFormat %b %d
    section Landed today
    Multi-session UI in serve              :done, multi, 2026-09-04, 1d
    Egress through the host (narrowing)    :done, egress, 2026-09-04, 1d
    VSCode extension (stdio)               :done, vsc, 2026-09-04, 1d
    Relevance selection (in-degree/fill)   :done, rel, 2026-09-04, 1d
    Handshake and signed approvals         :done, sig, 2026-09-04, 1d
    section Unblocked today
    Fleet measurements across machines     :crit, bench, 2026-09-04, 10d
    section Waiting on those
    Transfer: the record stream and import :fed, after sig, 11d
    The transfer probe                     :probe, after fed, 7d
```

---

## What actually blocks what

- **The VSCode extension is completely unblocked.** With `luu stdio` tested and
  Protocol v4 in place, the extension has a robust, port-free stdio transport to
  spawn and drive.
- **Egress through the host is unblocked.** `network` per plan landed; the next
  step is routing container traffic through a host-side filtering proxy so
  arbitrary LAN and WAN requests cannot exfiltrate host state.
- **Multi-session in `serve` is ready for the UI.** The SQLite storage layer
  and `context.resume` engine are fully functional; all that remains is UI
  affordances to list, select, and resume previous sessions.
- **Federation's safety-ordered half is done, and the rest of stage 1 is now
  unblocked.** The two items whose ordering is a safety property — a wire that
  says its version, and approvals signed with a key no relay holds — landed
  together, so transfer can be built without the destination having to trust
  what reached its socket. What it still cannot do is move a session: that is
  items 3 and 4, and the transfer probe cannot run until they exist.
- **Relevance selection has its falsifiable test.** `the-map-order-probe` built a
  38-question neutral corpus. Any new ranking algorithm (such as weighted
  in-degree) or non-greedy fill rule must beat path order on that exact corpus.
