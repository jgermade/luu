// @ts-check
/// Remote access: one luu linked to another, over SSH by key.
///
/// Two halves, one per kind of server. **A luu with no users dials**: its
/// `[peer.*]`, how each link is doing, and its own key (`/api/peers`, from
/// this machine only). **A server with users is dialled**: each user's keys,
/// and the luus those keys linked (`/api/auth/keys`). Every path is relative,
/// so under `/h/<name>/` the page asks that luu — which refuses Remote access
/// to a peer, as it should. See
/// `RECORD/2026-11-03.one-luu-to-another-over-ssh.WIP.md`.

import { apiHeaders } from "./store.js"

async function call(method, path, body) {
  try {
    const res = await fetch(path, {
      method,
      credentials: "same-origin",
      headers: body ? { ...apiHeaders(), "content-type": "application/json" } : apiHeaders(),
      body: body ? JSON.stringify(body) : undefined,
    })
    const text = await res.text()
    if (!res.ok) return { ok: false, status: res.status, error: text || `HTTP ${res.status}` }
    return { ok: true, view: text ? JSON.parse(text) : null }
  } catch (e) {
    return { ok: false, status: 0, error: `${e}` }
  }
}

/// `{ peers, status, key, fingerprint, editable, may }`.
export const loadPeers = () => call("GET", "./api/peers")

/// Replaces every `[peer.*]`, and redials.
export const savePeers = peers => call("PUT", "./api/peers", { peers })

/// What a server's page says about linking to it: `{ url, ssh, host_key,
/// fingerprint, key }`, `key` being this luu's own.
export const probePeer = url => call("POST", "./api/peers/probe", { url })

/// `{ keys, links, accepts }`, the logged-in user's.
export const loadKeys = () => call("GET", "./api/auth/keys")

export const addKey = (key, name) => call("POST", "./api/auth/keys", { key, name })

export const removeKey = key => call("POST", "./api/auth/keys/remove", { key })

/// Where a server's page adds this luu's key, filled in.
export const addKeyLink = (url, key, name) =>
  `${url.replace(/\/+$/, "")}/?add-key=${encodeURIComponent(key)}&name=${encodeURIComponent(name)}`

/// What the address bar brought: a key to add, from another luu's Remote
/// access. Read once.
export function keyAsked() {
  const query = new URLSearchParams(location.search)
  const key = query.get("add-key")
  return key ? { key, name: query.get("name") || "" } : null
}

/// The address bar without it, once it is added or dismissed.
export function forgetKeyAsked() {
  const url = new URL(location.href)
  url.searchParams.delete("add-key")
  url.searchParams.delete("name")
  history.replaceState(null, "", url.pathname + url.search + url.hash)
}
