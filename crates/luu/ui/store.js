// The transport layer, kept out of the components on purpose.
//
// Two reasons, both deliberate:
//
// 1. jq79 has no teardown hook yet (RECORD/2026-08-26.web-debug-client.completed.md), so
//    nothing a component owns can be closed when it unmounts. The socket lives
//    here, at module scope, where its lifetime is the page's. That is fine for
//    one page and will not survive per-session components — which is why the
//    hook is on the upstream list rather than crossed off it.
//
// 2. This is the half that will be type-checked. Once the protocol enums are
//    exported with `ts-rs`, this file gets `// @ts-check` against the generated
//    .d.ts; the templates stay untyped. Nothing here should need a DOM node.

import { $reactive } from "./vendor/jq79.js"

// What this client speaks, sent on connect so a host that speaks something else
// refuses it out loud rather than by misreading the next message. Kept beside
// `agent_core::protocol::VERSION` and `agent_core::record::FORMAT`: they are
// one number each, and this file is the other half of the pair.
const PROTOCOL = 5
const FORMAT = 7

export const state = $reactive({
  status: "connecting",   // connecting | ready | running | closed | replay
  backend: "",
  model: "",
  protocol: 0,
  // What the host calls this session. An approval is signed against it, so a
  // signature made here does not replay against another host.
  session: null,
  turn: null,
  // `turn` is the session's number for the exchange, which is what an eviction
  // names; `evicted` is the turn whose selection dropped this one, or null.
  messages: [],           // { id, turn, role, text, task, reason, usage, evicted }
  budget: null,           // { limit, counter, buckets: [...], backendPrompt }
  prompt: "",             // the exact string sent to the model, last turn
  prefix: null,           // { shared_bytes, shared_tokens, prompt_tokens } — null on turn 1
  // This turn's tool calls, in order. A call is pushed before it is checked, so
  // one that is still running (or was denied) is visible as itself rather than
  // as nothing happening. Per turn, like the budget beside it.
  // `command` is present for run_command only: { exit_code, signal, stdout,
  // stderr, duration_ms }. Null everywhere else, because an in-process tool has
  // no exit code and a zero would be a lie about a fact that does not exist.
  tools: [],              // [{ step, name, arguments, verdict, error, output, truncated, duration_ms, command }]
  // The model calls after the first one of this turn — the tool round trips.
  // The budget describes the first call only, while the backend's usage is
  // summed over all of them, so the two are comparable only with these added in.
  extraCalls: [],         // [{ step, prompt_tokens, shared_bytes, shared_tokens }]
  // The session's jobs, in the order they were proposed. A proposed one is a
  // gate: nothing runs behind it until someone answers.
  jobs: [],               // [{ id, objective, plan, proposed, source, state, summary }]
  tasks: [],              // alias for jobs
  error: null,
  // The last thing the server declined to do, and why. Cleared when a turn
  // starts, because by then the answer is on screen.
  refused: null,          // { request, reason, detail }
  // The last cut the window made. Kept beside the budget rather than inside it:
  // the buckets say what the prompt is worth, this says what stopped being in
  // it. Null in a session that never filled its window.
  evicted: null,          // { turn, turns, tokens, counter, policy }
  // Whether this session is a recording rather than a server. The status word
  // is not the same question: it says "running" while a recorded turn plays,
  // and a composer that reads the status is enabled over a recording nobody can
  // send into.
  replay: false,
  fixtures: [],           // replay mode only: [{ name, file, about }]
  fixture: "",            // the one being replayed
  sessions: [],           // live server sessions list: [{ id, title, turns, ... }]
  currentSessionId: "live",
})

let socket = null
let traceSocket = null
let backoff = 250
let frame = null
let pending = ""
let nextId = 1

// Messages are replaced, never mutated in place.
//
// jq79 does not wake an `:each` binding when a property of an object inside a
// reactive array is assigned from outside the component — `items[0].text = x`
// renders nothing, `items[0] = {...}` renders. Minimal repro and the upstream
// note are in RECORD/2026-08-26.walking-skeleton.completed.md. Replacing costs one object
// per frame, and `:key` keeps the DOM.
function replaceLast(patch) {
  const index = state.messages.length - 1
  if (index < 0) return null
  const last = state.messages[index]
  if (!last) return null
  state.messages[index] = { ...last, ...patch }
  return last
}

function patchJob(id, patch) {
  state.jobs = state.jobs.map(job => job.id === id ? { ...job, ...patch } : job)
  state.tasks = state.jobs
}
const patchTask = patchJob

/// The transcript as the context sees it: a closed job is one entry, its turns
/// folded behind the summary the model will get from now on.
///
/// Collapsed by default and expandable, which is the pair that matters — the
/// default view is what the prompt now contains, and opening one shows what it
/// no longer does. A debug client that could only show one of those would be
/// hiding the thing this whole design is about.
export function foldTranscript(messages, jobs, expanded) {
  const list = jobs || []
  const closed = new Map(list.filter(j => j.state === "closed").map(j => [j.id, j]))
  const entries = []
  const seen = new Set()

  for (const message of messages) {
    const id = message.job ?? message.task
    const job = closed.get(id)
    if (!job) {
      entries.push({ kind: "message", id: `m${message.id}`, message })
      continue
    }
    if (!seen.has(job.id)) {
      seen.add(job.id)
      entries.push({ kind: "fold", id: `j${job.id}`, job, task: job })
    }
    if (expanded.includes(job.id)) {
      entries.push({ kind: "message", id: `m${message.id}`, message })
    }
  }
  return entries
}

function flush() {
  frame = null
  if (!pending) return
  const last = state.messages[state.messages.length - 1]
  if (last && last.role === "assistant") replaceLast({ text: last.text + pending })
  pending = ""
}

function appendToken(text) {
  pending += text
  if (frame === null) frame = requestAnimationFrame(flush)
}

// The bearer token, when the server was bound off loopback and asked for one.
// The page itself is served without it — a browser navigation cannot carry a
// header — so a person opens `…/?token=…` and the transport takes it from
// there: on the socket as a query parameter, which is the only thing the
// `WebSocket` constructor can do, and on the read side as the header it should
// be everywhere. Null on the loopback default and on a static host, where both
// halves are unchanged.
const token = new URLSearchParams(location.search).get("token")

function url(path) {
  const scheme = location.protocol === "https:" ? "wss:" : "ws:"
  const query = token ? `?token=${encodeURIComponent(token)}` : ""
  return `${scheme}//${location.host}${path}${query}`
}

/// The read side, with the token when there is one.
function apiHeaders() {
  return token ? { Authorization: `Bearer ${token}` } : {}
}

function onProtocol(message) {
  switch (message.type) {
    case "hello":
      state.backend = message.backend
      state.model = message.model
      state.protocol = message.protocol
      state.session = message.session ?? null
      state.turn = message.turn
      state.status = message.turn === null ? idle() : "running"
      if (!isReplay) {
        refreshLiveSession()
        refreshSessionsList()
      }
      break

    case "turn_started": {
      state.turn = message.turn
      state.status = "running"
      state.error = null
      // Whatever was refused, the answer to it is on screen now.
      state.refused = null
      // The job it belongs to travels with the turn, so the transcript can
      // group without replaying the lifecycle to work out what was open.
      const turnJob = message.job ?? message.task ?? null
      state.messages.push({ id: nextId++, turn: message.turn, role: "user", text: message.prompt, job: turnJob, task: turnJob, reason: null, usage: null, evicted: null })
      state.messages.push({ id: nextId++, turn: message.turn, role: "assistant", text: "", job: turnJob, task: turnJob, reason: null, usage: null, evicted: null })
      state.tools = []
      state.extraCalls = []
      break
    }

    // The lifecycle. Jobs are replaced rather than mutated, for the same
    // reason messages are: jq79 does not wake an `:each` on a property assigned
    // inside an array element.
    case "job_proposed":
    case "task_proposed": {
      const id = message.job ?? message.task
      const newJob = {
        id,
        objective: message.objective,
        plan: message.plan,
        // Kept beside the plan as approved: the difference between them is what
        // a person had to add, which is the cost of the gate.
        proposed: message.plan,
        // Whether the planning call wrote this plan or answered in prose. Null
        // in a recording made before the distinction existed, and then the
        // panel has only emptiness to go on.
        source: message.source ?? null,
        state: "proposed",
        summary: null,
        closedBy: null,
      }
      state.jobs = [...state.jobs, newJob]
      state.tasks = state.jobs
      break
    }

    case "job_approved":
    case "task_approved": {
      // The plan as approved, which is what the job's sandbox is built from —
      // the person at the gate may have added what the model forgot. An older
      // recording carries none, and then the proposal is the best answer there
      // is.
      const id = message.job ?? message.task
      patchJob(id, message.plan
        ? { state: "approved", plan: message.plan }
        : { state: "approved" })
      break
    }

    // The server declining to do something, which used to be an early return
    // and therefore indistinguishable from a message that never arrived.
    case "refused":
      state.refused = { request: message.request, reason: message.reason, detail: message.detail }
      break

    case "job_rejected":
    case "task_rejected": {
      const id = message.job ?? message.task
      patchJob(id, { state: "rejected" })
      break
    }

    case "job_closed":
    case "task_closed": {
      // Who folded it. Absent in a recording made before there was more than
      // one authority, and then it was a person: nothing else could close a
      // job when the file was written.
      const id = message.job ?? message.task
      patchJob(id, {
        state: "closed",
        summary: message.summary,
        closedBy: message.by ?? "user",
      })
      break
    }

    case "job_reopened":
    case "task_reopened": {
      // The summary goes with the fold: it is an account of work that is being
      // written again.
      const id = message.job ?? message.task
      patchJob(id, { state: "approved", summary: null, closedBy: null })
      break
    }

    // What left the window and stays out. The turns are kept and marked, never
    // removed: a transcript that agreed with the prompt could no longer show
    // the difference between them, which is the one thing this client is for.
    case "evicted": {
      const gone = new Set(message.turns)
      state.messages = state.messages.map(m => gone.has(m.turn) ? { ...m, evicted: message.turn } : m)
      state.evicted = {
        turn: message.turn,
        turns: message.turns,
        tokens: message.tokens,
        counter: message.counter,
        policy: message.policy,
      }
      break
    }

    case "token":
      appendToken(message.text)
      break

    case "ended":
      flush()
      replaceLast({ reason: message.reason, usage: message.usage })
      // The gap between this and what we counted is the chat template, applied
      // where we cannot see it. Reassigned rather than mutated: the panel reads
      // the object, and one write is one update.
      if (state.budget && message.usage) {
        state.budget = { ...state.budget, backendPrompt: message.usage.prompt_tokens }
      }
      state.turn = null
      state.status = idle()
      break

    // Pushed and then replaced rather than mutated: jq79 does not wake an
    // `:each` binding on a property assigned inside an array element.
    case "tool_call":
      state.tools = [...state.tools, {
        step: message.step,
        name: message.name,
        arguments: message.arguments,
        verdict: null,
        error: null,
        output: "",
        truncated: false,
        duration_ms: null,
        command: null,
      }]
      break

    case "tool_result":
      state.tools = state.tools.map(call => call.step === message.step
        ? {
            ...call,
            verdict: message.verdict,
            error: message.error,
            output: message.output,
            truncated: message.truncated,
            duration_ms: message.duration_ms,
            command: message.command ?? null,
          }
        : call)
      break

    case "failed":
      flush()
      state.error = message.message
      state.turn = null
      state.status = idle()
      break
  }
}

function onTrace(message) {
  if (message.type === "prompt") state.prompt = message.text
  // Absent on the first turn of a session: there is no previous prompt, so the
  // panel says so rather than drawing 0%.
  if (message.type === "prefix_reuse") {
    state.prefix = {
      shared_bytes: message.shared_bytes,
      shared_tokens: message.shared_tokens,
      prompt_tokens: message.prompt_tokens,
    }
  }
  if (message.type === "step_call") {
    state.extraCalls = [...state.extraCalls, {
      step: message.step,
      prompt_tokens: message.prompt_tokens,
      shared_bytes: message.shared_bytes,
      shared_tokens: message.shared_tokens,
    }]
  }
  if (message.type === "budget") {
    // Arrives before the call now, so a cancelled turn has one too. The
    // backend's own count lands later, on `ended`.
    state.budget = {
      limit: message.limit,
      counter: message.counter,
      buckets: message.buckets,
      backendPrompt: null,
    }
  }
}

let everConnected = false
// Set once the page has given up on a server and taken the recordings instead.
let fellBack = false

function open(path, onMessage, assign, greet = false) {
  const ws = new WebSocket(url(path))
  ws.onmessage = event => {
    try {
      onMessage(JSON.parse(event.data))
    } catch (error) {
      // One unreadable frame must not take the page down with it.
      console.error("unparseable frame", error)
    }
  }
  ws.onopen = () => {
    backoff = 250
    everConnected = true
    // Before anything else, because on a port that requires a bearer token the
    // host refuses everything until this arrives. Harmless on loopback, where
    // it is optional.
    if (greet) ws.send(JSON.stringify({ type: "hello", protocol: PROTOCOL, format: FORMAT }))
  }
  ws.onclose = async () => {
    assign(null)
    state.status = "closed"

    // A fallback already took the page into replay: there is no server to
    // reconnect to and no second replay to start.
    if (fellBack) return

    // No agent was ever there: this is a static deploy, not a server that
    // restarted. Offer the recorded sessions instead of retrying forever.
    //
    // Both sockets close at once on a static host, so the flag is set *before*
    // the await and not after it. Without that, each of them started its own
    // replay of the same file, and the loser left a turn it had already pushed
    // at the top of the transcript — a user message with an assistant reply
    // that never fills, on every visit to the deployed page.
    if (!everConnected) {
      fellBack = true
      if (await loadFixtures()) {
        socket = traceSocket = null
        return replay(state.fixtures[0].file)
      }
      // Nothing to fall back to after all: a live server that went away.
      fellBack = false
    }

    // The server restarting is the ordinary case during development.
    setTimeout(connect, backoff)
    backoff = Math.min(backoff * 2, 5000)
  }
  return ws
}

/// Replay: the same messages, read from a recorded file instead of a socket.
///
/// This is what makes the UI useful on a static host — GitHub Pages has no
/// agent behind it, and a recorded session is a truer fixture than a hand-made
/// one, because it is a real run of the real protocol.
async function replay(file) {
  // Claimed before the reset, so a replay this one supersedes stops writing
  // into the state we are about to fill: it re-reads this after every await.
  const token = ++replayToken
  reset()
  isReplay = true
  // On the state, not only in this module: the composer asks "is this a
  // recording", and the status word cannot answer it — it says "running" while
  // a recorded turn plays.
  state.replay = true
  state.status = "replay"
  state.fixture = file

  const response = await fetch(file)
  if (!response.ok) {
    state.error = `could not load ${file} (${response.status})`
    return
  }

  const lines = (await response.text()).split("\n").filter(Boolean).map(JSON.parse)

  let previous = 0
  for (const line of lines) {
    if (token !== replayToken) return          // a newer replay superseded this one

    if (line.channel === "header") {
      state.backend = line.backend
      state.model = line.model
      state.protocol = line.protocol
      continue
    }

    // Played back at the pace it was recorded, capped so a session with a long
    // pause in it does not become a long pause here.
    const wait = Math.min(line.at_ms - previous, 400)
    previous = line.at_ms
    if (wait > 0) await new Promise(r => setTimeout(r, wait))
    if (token !== replayToken) return

    if (line.channel === "protocol") onProtocol(line.message)
    if (line.channel === "trace") onTrace(line.message)
  }

  flush()
  state.status = "replay"
}

/// What "not running" means depends on whether there is an agent behind us.
function idle() {
  return isReplay ? "replay" : "ready"
}

function reset() {
  state.messages = []
  state.jobs = []
  state.tasks = []
  state.budget = null
  state.tools = []
  state.extraCalls = []
  state.prompt = ""
  state.prefix = null
  state.error = null
  state.refused = null
  state.evicted = null
  state.turn = null
  pending = ""
}

let replayToken = 0
let isReplay = false

export function playFixture(file) {
  if (file) replay(file)
}

/// Asks the read API which sessions exist.
///
/// The same request answers on the live server and on a static host: `luu
/// export` writes this file, and the server serves the same JSON at the same
/// path. Returning false means there is nothing to fall back to, so
/// reconnecting is still right.
async function loadFixtures() {
  try {
    const response = await fetch("./api/sessions.json", { headers: apiHeaders() })
    if (!response.ok) return false
    const sessions = await response.json()
    // Only sessions that ship a recording can be replayed; a live one cannot.
    const replayable = sessions.filter(session => session.record)
    if (!replayable.length) return false
    state.fixtures = replayable.map(session => ({
      name: session.title || session.id,
      file: session.record,
      turns: session.turns,
    }))
    return true
  } catch {
    return false
  }
}

export async function connect() {
  const asked = new URLSearchParams(location.search).get("replay")
  if (asked) {
    await loadFixtures()
    return replay(asked)
  }
  if (socket) return
  socket = open("/ws", onProtocol, s => { socket = s }, true)
  traceSocket = open("/ws/trace", onTrace, s => { traceSocket = s })
}

export function send(text) {
  if (!socket || socket.readyState !== WebSocket.OPEN || !text.trim()) return
  socket.send(JSON.stringify({ type: "prompt", text }))
  state.prompt = ""
  state.budget = null
  state.tools = []
  state.extraCalls = []
  state.prefix = null
}

export function cancel() {
  if (!socket || socket.readyState !== WebSocket.OPEN) return
  socket.send(JSON.stringify({ type: "cancel" }))
}

/// The other half of the gate. Approving runs the prompt the server has been
/// holding since the proposal; refusing drops it with the plan.
function act(type, id) {
  if (!socket || socket.readyState !== WebSocket.OPEN) return
  socket.send(JSON.stringify({ type, job: id, task: id }))
}

/// Approving carries what the person added to the plan, which is the half that
/// makes narrowing survivable: a job may touch what it was approved for, so a
/// plan that forgot a file is widened here rather than rejected and retyped.
/// `closesOn` is the one part of a plan the model is never asked for: a command
/// line whose exit code of 0 folds the job without anyone present. Empty
/// leaves the person as the only authority, which is what every job did before
/// the field existed.
export function approveJob(job, files = [], writes = [], commands = [], closesOn = "") {
  if (!socket || socket.readyState !== WebSocket.OPEN) return
  socket.send(JSON.stringify({
    type: "approve_job", job, task: job, files, writes, commands,
    closes_on: closesOn.trim() || null,
  }))
}
export const approveTask = approveJob
export const rejectJob = job => act("reject_job", job)
export const rejectTask = rejectJob
export const closeJob = job => act("close_job", job)
export const closeTask = closeJob
export const reopenJob = job => act("reopen_job", job)
export const reopenTask = reopenJob

export async function refreshLiveSession() {
  try {
    const res = await fetch("./api/sessions/live", { headers: apiHeaders() })
    if (!res.ok) return
    const view = await res.json()
    reset()
    state.backend = view.backend
    state.model = view.model
    state.jobs = (view.jobs || []).map(j => ({
      id: j.id,
      objective: j.objective,
      plan: j.plan,
      proposed: j.proposed,
      source: j.source,
      state: j.state,
      summary: j.summary,
      closedBy: j.closed_by || null,
    }))
    state.tasks = state.jobs

    const msgs = []
    let id = 1
    for (const t of (view.turns || [])) {
      if (t.prompt) {
        msgs.push({
          id: id++,
          turn: t.turn,
          role: "user",
          text: t.prompt,
          job: t.job,
          task: t.job,
          reason: null,
          usage: null,
          evicted: null,
        })
      }
      if (t.text || (t.tools && t.tools.length)) {
        msgs.push({
          id: id++,
          turn: t.turn,
          role: "assistant",
          text: t.text || "",
          job: t.job,
          task: t.job,
          reason: null,
          usage: t.usage || null,
          evicted: null,
        })
      }
    }
    state.messages = msgs
    nextId = id
  } catch {
    // Ignore fetch failure
  }
}

export async function refreshSessionsList() {
  try {
    const res = await fetch("./api/sessions", { headers: apiHeaders() })
    if (!res.ok) return
    state.sessions = await res.json()
  } catch {
    // Ignore fetch failure
  }
}

export async function newSession() {
  try {
    const res = await fetch("./api/sessions", {
      method: "POST",
      headers: apiHeaders(),
    })
    if (!res.ok) {
      const err = await res.text()
      state.error = `Could not create session: ${err}`
      return
    }
    state.currentSessionId = "live"
    await refreshSessionsList()
  } catch (e) {
    state.error = `Could not create session: ${e}`
  }
}

export async function resumeSession(id) {
  try {
    const res = await fetch(`./api/sessions/${encodeURIComponent(id)}/resume`, {
      method: "POST",
      headers: apiHeaders(),
    })
    if (!res.ok) {
      const err = await res.text()
      state.error = `Could not resume session: ${err}`
      return
    }
    state.currentSessionId = "live"
    await refreshSessionsList()
  } catch (e) {
    state.error = `Could not resume session: ${e}`
  }
}

export async function deleteSession(id) {
  try {
    const res = await fetch(`./api/sessions/${encodeURIComponent(id)}`, {
      method: "DELETE",
      headers: apiHeaders(),
    })
    if (!res.ok) {
      const err = await res.text()
      state.error = `Could not delete session: ${err}`
      return
    }
    await refreshSessionsList()
  } catch (e) {
    state.error = `Could not delete session: ${e}`
  }
}

