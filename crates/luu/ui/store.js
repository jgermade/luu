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
  status: "connecting",   // connecting | ready | running | closed
  backend: "",
  model: "",
  protocol: 0,
  turn: null,
  messages: [],           // { id, role, text, reason, usage }
  budget: null,           // { limit, buckets: [{ name, tokens }] }
  prompt: "",             // the exact string sent to the model, last turn
  error: null,
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
      state.status = message.turn === null ? "ready" : "running"
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
      state.turn = null
      state.status = "ready"
      break

    case "failed":
      flush()
      state.error = message.message
      state.turn = null
      state.status = "ready"
      break
  }
}

function onTrace(message) {
  if (message.type === "prompt") state.prompt = message.text
  if (message.type === "budget") {
    state.budget = { limit: message.limit, buckets: message.buckets }
  }
}

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
  ws.onopen = () => { backoff = 250 }
  ws.onclose = () => {
    assign(null)
    state.status = "closed"
    // The server restarting is the ordinary case during development.
    setTimeout(connect, backoff)
    backoff = Math.min(backoff * 2, 5000)
  }
  return ws
}

export function connect() {
  if (socket) return
  socket = open("/ws", onProtocol, s => { socket = s })
  traceSocket = open("/ws/trace", onTrace, s => { traceSocket = s })
}

export function send(text) {
  if (!socket || socket.readyState !== WebSocket.OPEN || !text.trim()) return
  socket.send(JSON.stringify({ type: "prompt", text }))
  state.prompt = ""
  state.budget = null
}

export function cancel() {
  if (!socket || socket.readyState !== WebSocket.OPEN) return
  socket.send(JSON.stringify({ type: "cancel" }))
}
