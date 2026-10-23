// @ts-check
/// Settings → Policies, as calls: the shell, network and files lists and the
/// environments that combine them, read and written whole. The server checks
/// what it writes with the loader's own rules and answers with the tables as
/// they are now. See `put_policies` in `crate::serve` and
/// `RECORD/2026-10-22.policies-not-postures.WIP.md`.
import { apiHeaders, loadPostures } from "./store.js"

async function call(method, body) {
  const res = await fetch("./api/policies", {
    method,
    headers: { ...apiHeaders(), ...(body === undefined ? {} : { "content-type": "application/json" }) },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  })
  if (!res.ok) {
    const text = await res.text()
    let error = text || `${res.status}`
    try { error = JSON.parse(text).message || error } catch {}
    return { ok: false, error }
  }
  return { ok: true, view: await res.json() }
}

export const loadPolicies = () => call("GET")

/// `{ shell, network, files, environment }`, each a table by name. The
/// session starter offers the environments, so it is told.
export async function savePolicies(tables) {
  const result = await call("PUT", tables)
  if (result.ok) await loadPostures()
  return result
}
