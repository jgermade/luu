// @ts-check
/// Settings → Policies, as calls: the shell, network and files lists, read
/// and written whole. The server checks
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

/// `{ shell, network, files }`, each a table by name.
export async function savePolicies(tables) {
  const result = await call("PUT", tables)
  if (result.ok) await loadPostures()
  return result
}

/// What was allowed *always in this project*, by folder: `{ projects: {
/// "<folder>": { commands, writes } }, editable }`. Row 52; see
/// `RECORD/2026-11-09.always-in-this-project.completed.md`.
export async function loadProjectGrants() {
  const res = await fetch("./api/grants/project", { headers: apiHeaders() })
  if (!res.ok) return { ok: false, error: (await res.text()) || `${res.status}` }
  return { ok: true, view: await res.json() }
}

/// Takes one back: the next call it covered in that folder is asked again.
export async function removeProjectGrant(folder, kind, target) {
  const res = await fetch("./api/grants/project/remove", {
    method: "POST",
    headers: { ...apiHeaders(), "content-type": "application/json" },
    body: JSON.stringify({ folder, kind, target }),
  })
  if (!res.ok) return { ok: false, error: (await res.text()) || `${res.status}` }
  return { ok: true, view: await res.json() }
}
