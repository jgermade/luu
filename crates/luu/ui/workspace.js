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
  /// The file's rows, in the two blocks the viewer renders as two `:each`
  /// groups: `head` is the first screenful and `tail` is everything after it,
  /// one frame later.
  ///
  /// **Two arrays rather than one that grows.** The file arrives whole and
  /// already coloured; what costs is the row per line — ~50 µs, measured, with
  /// or without colour — and jq79 rebuilds an `:each` rather than appending to
  /// it. A growing array therefore rebuilds the rows already on screen every
  /// time it grows; two arrays mean the second render touches only the second
  /// block and the first is never built twice. See the phase 8 sections of
  /// `RECORD/2026-09-15.a-three-pane-inspector.WIP.md`.
  head: [],
  tail: [],
  /// How many rows `tail` *will* have, for the spacer that keeps the scrollbar
  /// the length of the file rather than the length of the first screen while
  /// the tail is still being built.
  pending: 0,
  /// Every changed path, mapped to git's own two-letter code.
  status: {},
  /// Why git could not be asked, when it could not. A workspace that is not a
  /// git repository is an ordinary case and lands here as a plain sentence.
  gitError: null,
  /// The last failure from any of these endpoints, for the panel to show.
  error: null,
  /// The icon theme's maps, by icon id, or `{ loaded: false }` when this
  /// machine named none. See `crate::icons` for why nothing is vendored, and
  /// `iconFor` for why the tree asks it per row rather than reading this flag.
  icons: { loaded: false },
})

/// The icon id for one entry, by VSCode's own rules.
///
/// Full filename first, then progressively shorter extensions — so `d.ts`
/// beats `ts` for `index.d.ts`, which is the distinction those themes draw —
/// then the theme's default. `null` when this row has no icon in this theme —
/// no theme loaded, or a theme that declares no default — and the tree draws
/// its own file or folder instead.
export function iconFor(name, isDir, expanded = false) {
  const theme = workspace.icons
  if (!theme.loaded) return null
  const lower = name.toLowerCase()
  if (isDir) {
    return (
      theme.folder_names?.[lower] ??
      (expanded ? theme.folder_expanded : theme.folder) ??
      theme.folder ??
      null
    )
  }
  const named = theme.file_names?.[lower]
  if (named) return named
  const parts = lower.split(".")
  for (let at = 1; at < parts.length; at++) {
    const suffix = parts.slice(at).join(".")
    const found = theme.file_extensions?.[suffix]
    if (found) return found
  }
  return theme.file ?? null
}

export async function loadIcons() {
  try {
    workspace.icons = await ask("./api/icons/manifest")
  } catch {
    // A page that cannot read the manifest simply has no icons; the tree is
    // the point and it still lists.
    workspace.icons = { loaded: false }
  }
}

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

/// Rows in the first pass: a screenful with room to scroll into, and nothing
/// to do with the window's real height — a viewer that measures itself has to
/// decide again on every resize.
const FIRST_ROWS = 200

/// Fills the second block, one frame after the first one is on screen.
function showTheRest(path, lines) {
  if (lines.length <= FIRST_ROWS) return
  requestAnimationFrame(() => {
    // Dropped if the viewer moved on while this frame was waiting: these rows
    // belong to the file on screen, never to the one that was.
    if (workspace.selected?.kind !== "file" || workspace.selected?.path !== path) return
    workspace.tail = lines.slice(FIRST_ROWS)
    workspace.pending = 0
  })
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
    workspace.head = file.lines.slice(0, FIRST_ROWS)
    workspace.tail = []
    workspace.pending = Math.max(0, file.lines.length - FIRST_ROWS)
    showTheRest(path, file.lines)
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
