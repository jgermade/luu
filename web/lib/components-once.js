// @ts-check
/// The page's component files, asked for once a visit.
///
/// jq79 fetches a component's `.html` every time a `setup` that imports it
/// runs, and a setup runs whenever what it draws is drawn again — a card in a
/// list, a dialog opened a second time. Counted on 2026-10-27: one visit that
/// opened Settings and the sessions dialog three times asked for
/// `scrollable.html` 14 times and `brand-icon.html` 13. A component file does
/// not change while the page is open, so this keeps each one's text the first
/// time it arrives and answers every later ask with it — before jq79 is
/// loaded, by wrapping `fetch`. A reload starts with nothing kept and asks
/// the server, which answers a 304 for what did not change. See
/// `RECORD/2026-10-27.the-page-asks-for-a-file-once.completed.md`.
///
/// Only the page's own files: a `GET` for a path ending in `.html` on this
/// origin, outside `/api/` — a workspace file named `x.html` is asked for by
/// a query, never by its path.

const plain = window.fetch.bind(window)
/** @type {Map<string, Promise<{ text: string, type: string } | null>>} */
const kept = new Map()

/** @param {RequestInfo | URL} input @param {RequestInit} [init] */
function component(input, init) {
  if (init?.method && init.method.toUpperCase() !== "GET") return null
  const href = typeof input === "string" ? input : input instanceof URL ? input.href : null
  if (!href) return null
  const url = new URL(href, location.href)
  if (url.origin !== location.origin || !url.pathname.endsWith(".html") || url.pathname.includes("/api/")) return null
  return url.href
}

window.fetch = (input, init) => {
  const key = component(input, init)
  if (!key) return plain(input, init)
  let once = kept.get(key)
  if (!once) {
    once = plain(input, init).then(async response => {
      if (!response.ok) return null
      return { text: await response.text(), type: response.headers.get("content-type") || "text/html" }
    })
    kept.set(key, once)
  }
  return once.then(file => {
    // Not kept: asked again next time, and answered as the server answers.
    if (!file) {
      kept.delete(key)
      return plain(input, init)
    }
    return new Response(file.text, { status: 200, headers: { "content-type": file.type } })
  })
}
