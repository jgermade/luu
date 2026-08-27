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
      state.messages.push({ id: nextId++, role: "user", text: message.prompt, reason: null, usage: null })
      state.messages.push({ id: nextId++, role: "assistant", text: "", reason: null, usage: null })
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
  state.budget = null
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
  state.prefix = null
}

export function cancel() {
  if (!socket || socket.readyState !== WebSocket.OPEN) return
  socket.send(JSON.stringify({ type: "cancel" }))
}
