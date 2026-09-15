// @ts-check
/// The workspace panels' shared state: what the tree has open, what git says,
/// and what the middle column is showing.
///
/// A module rather than props threaded through `app.html`, for the reason
/// `store.js` is one: the Files panel, the Git panel and the content viewer are
/// siblings, and a parent whose only job is relaying between its children is a
/// parent the state does not need. Both panels write `selected`; the viewer
/// reads it. See `RECORD/2026-09-15.a-three-pane-inspector.WIP.md`.

import { $reactive } from "./vendor/jq79.js"
import { apiHeaders } from "./store.js"

export const workspace = $reactive({
  /// One entry per directory that has been opened, keyed by its path relative
  /// to the sandbox base. `""` is the base itself. Lazy, like every file tree:
  /// a repository with a `target/` in it is not something to walk eagerly.
  open: {},
  /// Directories currently expanded, in the order they were opened. A plain
  /// object beside `open` rather than a flag inside it: collapsing a directory
  /// should not throw away what was already fetched.
  expanded: {},
  /// `{ kind: "file" | "diff", path, staged }`, or null for nothing selected.
  selected: null,
  /// What the viewer is showing, once it has arrived.
  /// `{ kind: "file", path, text, truncated }` or
  /// `{ kind: "diff", path, staged, hunks, note }`.
  content: null,
  /// Set while a fetch for `selected` is in flight, so the viewer can say so
  /// instead of showing the previous file under the new file's name.
  loading: false,
  /// Every changed path, mapped to git's own two-letter code.
  status: {},
  /// Why git could not be asked, when it could not. A workspace that is not a
  /// git repository is an ordinary case and lands here as a plain sentence.
  gitError: null,
  /// The last failure from any of these endpoints, for the panel to show.
  error: null,
})

async function ask(url) {
  const answer = await fetch(url, { headers: apiHeaders() })
  if (!answer.ok) throw new Error((await answer.text()) || `HTTP ${answer.status}`)
  return answer.json()
}

/// Lists one directory, caching it. `force` re-reads one already listed, which
/// is what the refresh button wants.
export async function openDir(path = "", force = false) {
  const key = path || ""
  if (!force && workspace.open[key]) {
    workspace.expanded = { ...workspace.expanded, [key]: true }
    return
  }
  try {
    const tree = await ask(`./api/workspace/tree?path=${encodeURIComponent(key)}`)
    workspace.open = { ...workspace.open, [key]: tree.entries }
    workspace.expanded = { ...workspace.expanded, [key]: true }
    workspace.gitError = tree.git || null
    workspace.error = null
  } catch (e) {
    workspace.error = `Could not list ${key || "the workspace"}: ${e.message}`
  }
}

export function closeDir(path) {
  const next = { ...workspace.expanded }
  delete next[path]
  workspace.expanded = next
}

export function toggleDir(path) {
  if (workspace.expanded[path]) closeDir(path)
  else openDir(path)
}

/// Re-reads every directory that is currently open, and git with it. What the
/// refresh button does: a tree is a snapshot, and an agent that just wrote a
/// file has made it stale.
export async function refresh() {
  const keys = Object.keys(workspace.open)
  await Promise.all(keys.map(key => openDir(key, true)))
  await loadStatus()
}

export async function loadStatus() {
  try {
    workspace.status = await ask("./api/workspace/git-status")
    workspace.gitError = null
  } catch (e) {
    workspace.status = {}
    workspace.gitError = e.message
  }
}

export async function showFile(path) {
  workspace.selected = { kind: "file", path, staged: false }
  workspace.loading = true
  try {
    const file = await ask(`./api/workspace/file?path=${encodeURIComponent(path)}`)
    // Dropped if the person clicked something else while this was in flight:
    // the panel's title and its body have to be the same file.
    if (workspace.selected?.path !== path || workspace.selected?.kind !== "file") return
    workspace.content = { kind: "file", ...file }
    workspace.error = null
  } catch (e) {
    workspace.content = null
    workspace.error = `Could not read ${path}: ${e.message}`
  } finally {
    workspace.loading = false
  }
}

export async function showDiff(path, staged = false) {
  workspace.selected = { kind: "diff", path, staged }
  workspace.loading = true
  try {
    const diff = await ask(
      `./api/workspace/git-diff?path=${encodeURIComponent(path)}&staged=${staged}`,
    )
    if (workspace.selected?.path !== path || workspace.selected?.kind !== "diff") return
    workspace.content = { kind: "diff", ...diff }
    workspace.error = null
  } catch (e) {
    workspace.content = null
    workspace.error = `Could not diff ${path}: ${e.message}`
  } finally {
    workspace.loading = false
  }
}
