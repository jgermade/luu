// The transport layer, kept out of the components on purpose.
//
// Two reasons, both deliberate:
//
// 1. jq79 has no teardown hook yet (RECORD/2026-08-26.web-debug-client.md), so
//    nothing a component owns can be closed when it unmounts. The socket lives
//    here, at module scope, where its lifetime is the page's. That is fine for
//    one page and will not survive per-session components — which is why the
//    hook is on the upstream list rather than crossed off it.
//
// 2. This is the half that will be type-checked. Once the protocol enums are
//    exported with `ts-rs`, this file gets `// @ts-check` against the generated
//    .d.ts; the templates stay untyped. Nothing here should need a DOM node.

import { $reactive } from "./vendor/jq79.js"

export const state = $reactive({
  status: "connecting",   // connecting | ready | running | closed | replay
  backend: "",
  model: "",
  protocol: 0,
  turn: null,
  messages: [],           // { id, role, text, reason, usage }
  budget: null,           // { limit, counter, buckets: [...], backendPrompt }
  prompt: "",             // the exact string sent to the model, last turn
  prefix: null,           // { shared_bytes, shared_tokens, prompt_tokens } — null on turn 1
  // This turn's tool calls, in order. A call is pushed before it is checked, so
  // one that is still running (or was denied) is visible as itself rather than
  // as nothing happening. Per turn, like the budget beside it.
  tools: [],              // [{ step, name, arguments, verdict, error, output, truncated, duration_ms }]
  // The model calls after the first one of this turn — the tool round trips.
  // The budget describes the first call only, while the backend's usage is
  // summed over all of them, so the two are comparable only with these added in.
  extraCalls: [],         // [{ step, prompt_tokens, shared_bytes, shared_tokens }]
  // The session's tasks, in the order they were proposed. A proposed one is a
  // gate: nothing runs behind it until someone answers.
  tasks: [],              // [{ id, objective, plan, state, summary }]
  error: null,
  fixtures: [],           // replay mode only: [{ name, file, about }]
  fixture: "",            // the one being replayed
})

let socket = null
let traceSocket = null
let nextId = 1
let backoff = 250

// Tokens are buffered and flushed once per frame. Assigning per token is the
// one thing guaranteed to stall the UI during exactly the fast generation
// worth watching — and jq79 has no effect batching to absorb it.
let pending = ""
let frame = null

// Messages are replaced, never mutated in place.
//
// jq79 does not wake an `:each` binding when a property of an object inside a
// reactive array is assigned from outside the component — `items[0].text = x`
// renders nothing, `items[0] = {...}` renders. Minimal repro and the upstream
// note are in RECORD/2026-08-26.walking-skeleton.md. Replacing costs one object
// per frame, and `:key` keeps the DOM.
function replaceLast(patch) {
  const index = state.messages.length - 1
  const last = state.messages[index]
  if (!last) return null
  state.messages[index] = { ...last, ...patch }
  return last
}

function patchTask(id, patch) {
  state.tasks = state.tasks.map(task => task.id === id ? { ...task, ...patch } : task)
}

/// The transcript as the context sees it: a closed task is one entry, its turns
/// folded behind the summary the model will get from now on.
///
/// Collapsed by default and expandable, which is the pair that matters — the
/// default view is what the prompt now contains, and opening one shows what it
/// no longer does. A debug client that could only show one of those would be
/// hiding the thing this whole design is about.
export function foldTranscript(messages, tasks, expanded) {
  const closed = new Map(tasks.filter(task => task.state === "closed").map(task => [task.id, task]))
  const entries = []
  const seen = new Set()

  for (const message of messages) {
    const task = closed.get(message.task)
    if (!task) {
      entries.push({ kind: "message", id: `m${message.id}`, message })
      continue
    }
    if (!seen.has(task.id)) {
      seen.add(task.id)
      entries.push({ kind: "fold", id: `t${task.id}`, task })
    }
    if (expanded.includes(task.id)) {
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

function url(path) {
  const scheme = location.protocol === "https:" ? "wss:" : "ws:"
  return `${scheme}//${location.host}${path}`
}

function onProtocol(message) {
  switch (message.type) {
    case "hello":
      state.backend = message.backend
      state.model = message.model
      state.protocol = message.protocol
      state.turn = message.turn
      state.status = message.turn === null ? idle() : "running"
      break

    case "turn_started":
      state.turn = message.turn
      state.status = "running"
      state.error = null
      // The task it belongs to travels with the turn, so the transcript can
      // group without replaying the lifecycle to work out what was open.
      state.messages.push({ id: nextId++, role: "user", text: message.prompt, task: message.task, reason: null, usage: null })
      state.messages.push({ id: nextId++, role: "assistant", text: "", task: message.task, reason: null, usage: null })
      state.tools = []
      state.extraCalls = []
      break

    // The lifecycle. Tasks are replaced rather than mutated, for the same
    // reason messages are: jq79 does not wake an `:each` on a property assigned
    // inside an array element.
    case "task_proposed":
      state.tasks = [...state.tasks, {
        id: message.task,
        objective: message.objective,
        plan: message.plan,
        state: "proposed",
        summary: null,
      }]
      break

    case "task_approved":
      patchTask(message.task, { state: "approved" })
      break

    case "task_rejected":
      patchTask(message.task, { state: "rejected" })
      break

    case "task_closed":
      patchTask(message.task, { state: "closed", summary: message.summary })
      break

    case "task_reopened":
      // The summary goes with the fold: it is an account of work that is being
      // written again.
      patchTask(message.task, { state: "approved", summary: null })
      break

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

function open(path, onMessage, assign) {
  const ws = new WebSocket(url(path))
  ws.onmessage = event => {
    try {
      onMessage(JSON.parse(event.data))
    } catch (error) {
      // One unreadable frame must not take the page down with it.
      console.error("unparseable frame", error)
    }
  }
  ws.onopen = () => { backoff = 250; everConnected = true }
  ws.onclose = async () => {
    assign(null)
    state.status = "closed"

    // No agent was ever there: this is a static deploy, not a server that
    // restarted. Offer the recorded sessions instead of retrying forever.
    if (!everConnected && await loadFixtures()) {
      socket = traceSocket = null
      return replay(state.fixtures[0].file)
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
  reset()
  isReplay = true
  state.status = "replay"
  state.fixture = file

  const response = await fetch(file)
  if (!response.ok) {
    state.error = `could not load ${file} (${response.status})`
    return
  }

  const lines = (await response.text()).split("\n").filter(Boolean).map(JSON.parse)
  const token = ++replayToken

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
  state.tasks = []
  state.budget = null
  state.tools = []
  state.extraCalls = []
  state.prompt = ""
  state.prefix = null
  state.error = null
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
    const response = await fetch("./api/sessions.json")
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
  socket = open("/ws", onProtocol, s => { socket = s })
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
function act(type, task) {
  if (!socket || socket.readyState !== WebSocket.OPEN) return
  socket.send(JSON.stringify({ type, task }))
}

export const approveTask = task => act("approve_task", task)
export const rejectTask = task => act("reject_task", task)
export const closeTask = task => act("close_task", task)
export const reopenTask = task => act("reopen_task", task)
