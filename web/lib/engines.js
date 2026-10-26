// @ts-check
/// Settings → Engines, as calls: the model servers luu starts, and the
/// downloads of luu's own copies. Every answer is the whole view again, so the
/// page never works out a server's state for itself. See `crate::engines` and
/// `RECORD/2026-09-28.a-model-server-luu-starts.completed.md`.
import { apiHeaders, state, loadProviders, saveProviders } from "./store.js"

async function call(method, path, body) {
  const res = await fetch(path, {
    method,
    headers: { ...apiHeaders(), ...(body === undefined ? {} : { "content-type": "application/json" }) },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  })
  if (!res.ok) return { ok: false, error: (await res.text()) || `${res.status}` }
  return { ok: true, view: await res.json() }
}

export const loadEngines = () => call("GET", "./api/engines")
/// The whole `[engine.*]` table: the page writes one table at a time, the
/// way it writes the providers.
export const saveEngines = engines => call("PUT", "./api/engines", { engines })
// A JSON body, and it is load-bearing: see `start_engine`. `model`, a catalog
// reference, starts it on that model rather than its table's.
export const startEngine = (name, model = "") =>
  call("POST", `./api/engines/${encodeURIComponent(name)}/start`, model ? { model } : {})
export const stopEngine = name => call("POST", `./api/engines/${encodeURIComponent(name)}/stop`, {})
export const installEngine = (kind, version, variant) =>
  call("POST", "./api/engines/install", { kind, version: version || null, variant: variant || null })
/// Uninstalls an engine on luu's own copy: the copy, its table and the
/// providers that start it. See `uninstall_engine`.
export const uninstallEngine = name =>
  call("POST", `./api/engines/${encodeURIComponent(name)}/uninstall`, {})

/// Talks to an engine on a model, outside any session: `messages` is the
/// conversation so far, `onText` is handed each piece of the reply as it
/// streams. Resolves to `{ ok, usage }` or `{ ok: false, error }`. See
/// `chat_engine`.
export async function chatEngine(name, model, messages, onText) {
  const res = await fetch(`./api/engines/${encodeURIComponent(name)}/chat`, {
    method: "POST",
    headers: { ...apiHeaders(), "content-type": "application/json" },
    body: JSON.stringify({ model: model || null, messages }),
  })
  if (!res.ok || !res.body) return { ok: false, error: (await res.text()) || `${res.status}` }
  const reader = res.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ""
  let usage = null
  let error = ""
  for (;;) {
    const { value, done } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
      const line = buffer.slice(0, at).trim()
      buffer = buffer.slice(at + 1)
      if (!line) continue
      const event = JSON.parse(line)
      if (typeof event.text === "string") onText(event.text)
      if (event.error) error = event.error
      if (event.done) usage = event.done.usage || null
    }
  }
  return error ? { ok: false, error } : { ok: true, usage }
}

/// The first port from the kind's own upward that nothing has: `{ ok, port }`
/// or `{ ok: false, error }`. See `engines::free_port`.
export async function freePort(kind) {
  const res = await fetch(`./api/engines/port?kind=${encodeURIComponent(kind)}`, { headers: apiHeaders() })
  if (!res.ok) return { ok: false, error: (await res.text()) || `${res.status}` }
  return { ok: true, port: (await res.json()).port }
}

/// Puts an engine on another port, and the providers that send to it with it
/// — each only where its URL still says the old one, so a URL somebody
/// changed by hand is left alone. `view` is the engines view as the page has
/// it; answers `{ ok, view }` or `{ ok: false, error }`.
export async function moveEngine(view, name, port) {
  const engines = Object.fromEntries(Object.entries(view.engines).map(([n, e]) => [n, tableOf(e)]))
  const before = view.engines[name]?.url
  engines[name] = { ...engines[name], port }
  const written = await saveEngines(engines)
  if (!written.ok) return written
  const after = written.view.engines[name]?.url
  await loadProviders()
  const providers = { ...(state.providers?.providers || {}) }
  let changed = false
  for (const linked of view.engines[name]?.providers || []) {
    if (before !== after && providers[linked]?.url === before) {
      providers[linked] = { ...providers[linked], url: after }
      changed = true
    }
  }
  if (changed) {
    const result = await saveProviders({ default: state.providers?.default ?? null, providers })
    if (!result?.ok) return { ok: false, error: `The engine moved, and its provider did not: ${state.providersError}` }
  }
  return written
}

/// What the file says of one engine, out of the view — which also carries its
/// status, the resolved binary and the linked providers. `enabled` is among
/// the keys, so a write of one table does not switch another back on.
export const tableOf = e => Object.fromEntries(
  ["kind", "binary", "version", "variant", "port", "model", "args", "ready-timeout", "enabled"]
    .filter(key => e[key] !== undefined && e[key] !== null)
    .map(key => [key, e[key]]))

/// What a profile for this engine is: the backend each kind speaks, and the
/// URL its port answers on. The same two facts `EngineKind::url` states.
export function profileFor(name, kind, port) {
  return kind === "ollama"
    ? { backend: "ollama", url: `http://127.0.0.1:${port}`, engine: name }
    : { backend: "openai", url: `http://127.0.0.1:${port}/v1`, engine: name }
}

/// Each kind's own port — `EngineKind::default_port`.
export const defaultPort = kind => ({ ollama: 11434, mlx: 11234 })[kind] || 8080

/// Every model on this machine — ollama, llama.cpp, Hugging Face, luu's own —
/// each with a `reference` for `config.toml` and a `label` to show.
export const loadCatalog = () => call("GET", "./api/models")

const SOURCES = ["ollama", "llama.cpp", "huggingface", "luu"]

/// `name (source)` for a reference into the catalog, the text itself for any
/// other model name — the same rule as `models::label` on the server.
export function modelLabel(text) {
  if (!text) return ""
  const at = text.indexOf(":")
  const source = at > 0 ? text.slice(0, at) : ""
  if (!SOURCES.includes(source)) return text
  const rest = text.slice(at + 1)
  const name = source === "ollama" ? rest : rest.split("/").pop().replace(/\.gguf$/, "")
  return `${name} (${source})`
}
