// @ts-check
/// The workspace panels' shared state: which folder is being looked at, what
/// the tree has open, what git says, and what the content column is showing.
///
/// A module rather than props threaded through `app.html`, for the reason
/// `store.js` is one: the Files panel, the Git panel and the content viewer are
/// siblings, and a parent whose only job is relaying between its children is a
/// parent the state does not need. Both panels write tabs; the viewer reads
/// them. See `RECORD/2026-09-15.a-three-pane-inspector.completed.md` and
/// `RECORD/2026-09-16.three-columns-that-each-have-a-footer.completed.md`.

import { $reactive } from "@web/vendor/jq79.js"
import { apiHeaders, refreshSettings, state } from "./store.js"
import { currentHost } from "./hosts.js"
import { setPane } from "./prefs.js"

export const workspace = $reactive({
  /// The absolute path of the session's folder — its sandbox's base — as the
  /// server reports it. Display only — nothing is ever asked for by absolute
  /// path, because an absolute path in a URL is an invitation to send a
  /// different one.
  base: "",
  /// Where `luu serve` was started, absolute: what a session's folder is
  /// chosen under, and what the remembered folder is keyed by.
  serveRoot: "",
  /// The session's folder, relative to `serveRoot`: `""` is the root itself.
  /// See `RECORD/2026-10-30.sessions-side-by-side.WIP.md` §5.
  folder: "",
  /// What the last folder choice could not do, said where it was made: a
  /// session with turns stays in its folder, and the next one starts in the
  /// chosen one.
  notice: null,
  /// The chosen folder, relative to the base and `/`-separated. `""` is the
  /// base itself. **`null` means nobody has chosen yet**, which is what opens
  /// the picker over the whole window on a first visit — it is not the same
  /// state as `""`, and collapsing the two would mean the picker either never
  /// appears or appears every time.
  root: null,
  /// One entry per directory that has been opened, keyed by its path relative
  /// to the sandbox base — never to the root, so narrowing the root does not
  /// invalidate a listing that is still correct. `""` is the base itself.
  /// Lazy, like every file tree: a repository with a `target/` in it is not
  /// something to walk eagerly.
  open: {},
  /// Directories currently expanded, in the order they were opened. A plain
  /// object beside `open` rather than a flag inside it: collapsing a directory
  /// should not throw away what was already fetched.
  expanded: {},
  /// What the content column is holding: `{ id, kind, title, path, staged,
  /// preview }`, where `kind` is `file`, `diff` or `debug`. One tab per open
  /// thing, so looking at a diff no longer loses the file that was on screen;
  /// at most one of them a `preview`, which the next single click replaces.
  tabs: [],
  /// The active tab's id, or `null` for an empty column.
  active: null,
  /// The active tab itself. `picked` rather than `selected` in the components
  /// that read it, and the name is load-bearing: a scope variable called
  /// `selected` collides with the DOM property of that name, and jq79's render
  /// effect ends up writing what it reads. See `views/editor/editor.html`.
  selected: null,
  /// What the viewer is showing, once it has arrived.
  /// `{ kind: "file", path, lines, truncated }`,
  /// `{ kind: "diff", path, staged, hunks, note }` or
  /// `{ kind: "debug", text }`.
  content: null,
  /// Set while a fetch for the active tab is in flight, so the viewer shows
  /// nothing rather than the previous file under the new file's name. It no
  /// longer says "Reading…": a line that came and went on every click moved the
  /// file down and back up.
  loading: false,
  /// Every changed path in the whole base, mapped to git's own two-letter code.
  /// Asked for the base and filtered to the root in the page rather than
  /// parameterised: it is one `git status --porcelain` for a repository either
  /// way, and a second parameter would be a second thing to keep consistent
  /// with the prefix.
  status: {},
  /// Whether the base is in a git repository at all: `false` for a folder
  /// nobody ran `git init` in, which is an answer and not an error. `null`
  /// until git has been asked.
  repository: null,
  /// Why git could not be asked, when it could not. A workspace that is not a
  /// git repository is an ordinary case and lands here as a plain sentence.
  gitError: null,
  /// The last failure from any of these endpoints, for the panel to show.
  error: null,
  /// The icon theme's maps, by icon id, or `{ loaded: false }` when this
  /// machine named none. See `crate::icons` for why nothing is vendored, and
  /// `iconFor` for why the tree asks it per row rather than reading this flag.
  icons: { loaded: false },
  /// The folder picker: whether it is open, and the directory it is showing.
  /// `forced` is a first visit, where it covers the window and has no way out
  /// other than choosing — there is nothing behind it to go back to.
  picker: { open: false, forced: false, at: "" },
})

// ---- which folder ---------------------------------------------------------
//
// The folder `luu serve` was started in is the **ceiling**, and what is chosen
// is a subdirectory of it. Nothing here widens anything: `/api/workspace/*`
// resolves every path through `Sandbox::check_path` against that base, and a
// chosen root is a prefix this page puts on what it asks for. A picker that
// could climb above the base would be a browser control that moves the sandbox,
// and that is the one thing the workspace surface has never been.

/// Where the remembered root is kept, keyed by the base — two servers on one
/// machine do not inherit each other's answer — and by the host, so two
/// machines with the same `$HOME` do not either. This machine's key is the one
/// it always was, so nothing remembered before hosts is lost.
export const rootKey = (base, host = currentHost) =>
  host ? `luu.root@${host}:${base}` : `luu.root:${base}`

/// A base-relative path as the panels *show* it — relative to the chosen root.
/// Returns `null` for anything outside it, which is what filters git's status
/// down to the folder somebody chose.
///
/// **Only the display goes through here.** Every path this module sends to the
/// server stays relative to the base, exactly as the server hands it out: a
/// second spelling for the same file is a second chance to get the prefix wrong,
/// and the root is a narrowing of what is *listed*, not a second address space.
export function inside(path) {
  const root = workspace.root || ""
  if (!root) return path
  if (path === root) return ""
  return path.startsWith(`${root}/`) ? path.slice(root.length + 1) : null
}

/// Reads the session's folder off the settings the server already answers.
/// **The folder is the session's** since 2026-10-30: the panels show the whole
/// of it (`root` is `""`), and what this browser remembers — keyed by where
/// `serve` was started — is the folder a fresh session is moved into on a
/// visit, and the one new sessions start in. Opens the picker when nothing was
/// remembered and the session is at the root. See
/// `RECORD/2026-10-30.sessions-side-by-side.WIP.md` §5.
export async function loadRoot(settings) {
  following(settings)
  let remembered = null
  try {
    remembered = localStorage.getItem(rootKey(workspace.serveRoot))
  } catch {
    // A private window. The picker opens; the choice is simply not kept.
  }
  if (remembered === null) {
    if (workspace.folder) return true
    workspace.picker = { open: true, forced: true, at: "" }
    return false
  }
  // A session with nothing in it yet moves to the remembered folder; one with
  // turns is refused by the server and stays where it is.
  if (remembered !== workspace.folder) await moveFolder(remembered)
  return true
}

/// Takes the folder and the root out of the settings.
function following(settings) {
  workspace.serveRoot = settings?.root || settings?.base || ""
  workspace.base = settings?.base || ""
  workspace.folder = settings?.folder || ""
  workspace.root = ""
}

/// The folder new sessions start in: what was last chosen here, else the
/// folder of the session on screen.
export function folderForNew() {
  try {
    const remembered = localStorage.getItem(rootKey(workspace.serveRoot))
    if (remembered !== null) return remembered
  } catch {
    // Not kept: the one on screen.
  }
  return workspace.folder || ""
}

/// Moves the session on screen to `folder` (`PUT /api/session/folder`), which
/// the server allows before its first turn. `true` when it moved or was
/// already there; else `workspace.notice` says why.
async function moveFolder(folder) {
  try {
    const res = await fetch("./api/session/folder", {
      method: "PUT",
      headers: { ...apiHeaders(), "content-type": "application/json" },
      body: JSON.stringify({ folder }),
    })
    if (!res.ok) {
      workspace.notice = (await res.text()) || `HTTP ${res.status}`
      return false
    }
    workspace.notice = null
    // Through the page's own settings, so what follows the session's folder
    // sees the same one this just moved to.
    await refreshSettings()
    following(state.settings)
    return true
  } catch (e) {
    workspace.notice = e.message
    return false
  }
}

/// The panels, over again on the session's folder: what a session switch and
/// a folder move both need. Everything open was open relative to another
/// folder, so none of it survives.
export async function followFolder(settings) {
  following(settings)
  await startOver()
}

async function startOver() {
  workspace.tabs = []
  workspace.active = null
  workspace.selected = null
  workspace.content = null
  workspace.open = {}
  workspace.expanded = {}
  await openDir("", true)
  await loadStatus()
}

export function openPicker() {
  workspace.picker = { open: true, forced: false, at: workspace.root || "" }
  // The picker lists directories, which means listing them: a folder nobody has
  // opened in the tree has never been fetched.
  openDir(workspace.picker.at)
}

export function closePicker() {
  workspace.notice = null
  // A first visit has nothing behind it to go back to, so the only way out of a
  // forced picker is choosing.
  if (workspace.picker.forced) return
  workspace.picker = { open: false, forced: false, at: "" }
}

export function browseTo(path) {
  workspace.picker = { ...workspace.picker, at: path }
  openDir(path)
}

/// Takes the folder: remembered for the sessions that start here, and the
/// session on screen moved into it when it has no turns yet — then the panels
/// start over on it. A session that has turns stays where it is, and the
/// picker closes saying so: the next session starts in the folder chosen.
export async function chooseRoot(path) {
  try {
    localStorage.setItem(rootKey(workspace.serveRoot), path)
  } catch {
    // Applied for this visit, and asked again next time.
  }
  const moved = await moveFolder(path)
  if (!moved) {
    // Stays open, saying why, so the choice is not silently half-made.
    workspace.notice = `${workspace.notice} — new sessions will start in ${path || "the root"}.`
    return
  }
  workspace.picker = { open: false, forced: false, at: "" }
  await startOver()
}

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

/// The tree's own shape for a row when no icon theme draws it: a folder, open
/// or shut, or a sheet with its kind on it — `{ id, kind }`, `id` the sprite's
/// symbol and `kind` what colours it (`--luu-glyph-<kind>`), or `""` for a
/// sheet with nothing on it. By extension, and by the names that carry none.
/// Not an icon theme: a kind is a family, not a language.
const KINDS = {
  code: "rs js mjs cjs ts tsx jsx py go java kt kts c h cc cpp hpp cs rb php swift scala lua zig ex exs hs ml dart r jl sql vue svelte",
  markup: "html htm xml xhtml css scss sass less",
  data: "json jsonc json5 toml yaml yml ini cfg conf lock env csv tsv plist properties",
  doc: "md mdx txt rst adoc org pdf log",
  image: "png jpg jpeg gif svg webp ico bmp avif tif tiff",
  media: "mp3 wav ogg flac m4a mp4 mov webm mkv avi",
  archive: "zip tar gz tgz bz2 xz 7z rar zst",
  shell: "sh bash zsh fish ps1 bat cmd",
}
const BY_EXTENSION = Object.fromEntries(
  Object.entries(KINDS).flatMap(([kind, list]) => list.split(" ").map(ext => [ext, kind])))
const BY_NAME = {
  dockerfile: "data", containerfile: "data", makefile: "shell", justfile: "shell",
  license: "doc", licence: "doc", readme: "doc", changelog: "doc",
}
/// The theme's sprite of those shapes, relative so it resolves under a host's
/// prefix (`/h/<name>/`) as well. See `theme/default/files.svg`.
export const FILES_SPRITE = "./theme/default/files.svg"

export function glyphFor(name, isDir, open = false) {
  if (isDir) return { id: `${FILES_SPRITE}#${open ? "i-folder-open" : "i-folder"}`, kind: "folder" }
  const lower = name.toLowerCase()
  const dot = lower.lastIndexOf(".")
  const kind = BY_NAME[lower] ?? BY_EXTENSION[dot >= 0 ? lower.slice(dot + 1) : ""] ??
    // A dotfile with no kind of its own (`.gitignore`, `.editorconfig`) is
    // configuration, which is what nearly all of them are.
    (dot === 0 ? "data" : "")
  return { id: `${FILES_SPRITE}#${kind ? `i-file-${kind}` : "i-file"}`, kind }
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

/// Every theme Settings offers, drawn: `{ samples: [{ name, dir }], previews:
/// { path: [url | null] } }`, one icon per sample per theme. One request for
/// all of them; empty, never an error, when the server cannot say.
export async function loadIconPreviews() {
  try {
    return await ask("./api/icon-theme/previews")
  } catch {
    return { samples: [], previews: {} }
  }
}

/// `[ui] icon-theme` as Settings shows it: what is set, whether it loaded, and
/// every theme this machine has. See
/// `RECORD/2026-09-28.an-icon-theme-from-settings.completed.md`.
export async function loadIconTheme() {
  return ask("./api/icon-theme")
}

/// Sets the theme to a path (or clears it with `null`), and redraws the tree's
/// icons once the server has swapped it. Resolves to `{ ok, view }` or
/// `{ ok: false, error }`: a path that does not load is refused with the
/// loader's own message, which is the one worth showing.
export async function saveIconTheme(path) {
  return answered(await fetch("./api/icon-theme", {
    method: "PUT",
    headers: { ...apiHeaders(), "Content-Type": "application/json" },
    body: JSON.stringify({ icon_theme: path }),
  }))
}

/// Imports the folder a directory picker chose, then selects it. `files` is
/// the input's `FileList`; each one goes up under the path the picker gave it.
export async function importIconTheme(files) {
  return importThemeFiles([...files].map(file => ({ path: file.webkitRelativePath || file.name, blob: file })))
}

/// The files of a theme as the server takes them: `{ path, blob }`, every path
/// under one folder whose name the import is called by.
export async function importThemeFiles(entries) {
  const form = new FormData()
  for (const { path, blob } of entries) form.append("file", blob, path)
  return answered(await fetch("./api/icon-theme/import", {
    method: "POST",
    headers: apiHeaders(),
    body: form,
  }))
}

// ---- a theme, from whatever was given ----------------------------------------
//
// A folder, a zip, or a theme JSON, picked or dropped — each made into the one
// shape the server takes: the files of one folder. See
// `RECORD/2026-10-21.the-design-system-applied.completed.md`.

const stem = name => name.replace(/\.[^.]+$/, "") || "theme"

/// Under one folder: as they are when every path already shares its first
/// part, and under `name` otherwise — a zip of a theme's contents, or a JSON
/// with its icons beside it.
function underOneFolder(entries, name) {
  const tops = new Set(entries.map(entry => entry.path.split("/")[0]))
  const nested = entries.every(entry => entry.path.includes("/"))
  return tops.size === 1 && nested
    ? entries
    : entries.map(entry => ({ ...entry, path: `${name}/${entry.path}` }))
}

/// Files picked or dropped loose: a zip is opened and is the theme; anything
/// else — a theme JSON, with whatever came with it — is the theme's folder,
/// named after the JSON.
export async function themeFromFiles(files) {
  const list = [...files]
  const zip = list.find(file => /\.zip$/i.test(file.name) || file.type === "application/zip")
  if (zip) {
    const { unzip } = await import("./unzip.js")
    return underOneFolder(await unzip(zip), stem(zip.name))
  }
  const json = list.find(file => /\.json$/i.test(file.name))
  return underOneFolder(
    list.map(file => ({ path: file.webkitRelativePath || file.name, blob: file })),
    stem(json?.name || list[0]?.name || "theme"))
}

/// What was dropped: a folder walked to its files, or loose files as above.
export async function themeFromDrop(transfer) {
  const roots = [...transfer.items]
    .map(item => item.webkitGetAsEntry?.())
    .filter(Boolean)
  const folder = roots.find(entry => entry.isDirectory)
  if (!folder) return themeFromFiles(transfer.files)
  const out = []
  const walk = async (entry, path) => {
    if (entry.isFile) {
      const blob = await new Promise((done, fail) => entry.file(done, fail))
      if (entry.name !== ".DS_Store") out.push({ path, blob })
      return
    }
    const reader = entry.createReader()
    // `readEntries` answers in batches until one comes back empty.
    for (;;) {
      const batch = await new Promise((done, fail) => reader.readEntries(done, fail))
      if (!batch.length) break
      for (const child of batch) await walk(child, `${path}/${child.name}`)
    }
  }
  await walk(folder, folder.name)
  return out
}

async function answered(response) {
  if (!response.ok) return { ok: false, error: (await response.text()) || `HTTP ${response.status}` }
  const view = await response.json()
  await loadIcons()
  return { ok: true, view }
}

async function ask(url) {
  const answer = await fetch(url, { headers: apiHeaders() })
  if (!answer.ok) throw new Error((await answer.text()) || `HTTP ${answer.status}`)
  return answer.json()
}

/// Lists one directory, caching it. `force` re-reads one already listed, which
/// is what the refresh button wants. `path` is relative to the base.
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
    const answer = await ask("./api/workspace/git-status")
    workspace.status = answer.files || {}
    workspace.repository = answer.repository
    workspace.gitError = null
  } catch (e) {
    workspace.status = {}
    workspace.gitError = e.message
  }
}

// ---- the content column's tabs --------------------------------------------
//
// **A tab holds its identity, not its bytes.** Activating one re-fetches. The
// server answers the largest file in this repository in 20 ms and a small one in
// 0.6; the 271 ms a file costs is building the rows, which a cache would not
// save because the rows are rebuilt on activation either way. So a cache would
// hold megabytes of JSON to save 20 ms of the 291 that switching a tab costs.
// See `RECORD/2026-09-16.what-the-debug-ui-does-not-need.completed.md`.

const fileId = path => `file:${path}`
// **One tab per file's diff, not one per side.** "Staged" and "working tree"
// are two different answers about the same file, and the foot's toggle is how
// you read the other one — a second tab with the same name in it would make the
// strip say there are two files open when there is one.
const diffId = path => `diff:${path}`

const basename = path => path.split("/").pop() || path

/// Opens a tab, or focuses the one already holding this thing, and points the
/// second column at the content — the click already said which column the
/// person wants, and in the two-column layout having nothing happen is the bug
/// this rule exists to prevent.
///
/// **A single click opens a preview, the way VS Code does.** There is one
/// preview tab at most, drawn in italics, and the next single click replaces
/// it in place rather than adding a tab beside it: looking through ten files
/// should not leave ten tabs to close. A double click, the tree's pencil, or a
/// double click on the tab itself pins it ([`pinTab`]). Reopening a tab that is
/// already pinned never unpins it — the foot's "staged" toggle calls this with
/// the same id.
function openTab(tab, { preview = false } = {}) {
  const found = workspace.tabs.find(open => open.id === tab.id)
  // Replaced rather than mutated, the way every reactive list in this page is:
  // jq79 does not wake an `:each` when a property of an object inside a
  // reactive array is assigned from outside the component. See `store.js`.
  if (found) {
    const next = { ...found, ...tab, preview: found.preview && preview }
    workspace.tabs = workspace.tabs.map(open => open.id === tab.id ? next : open)
  } else {
    const next = { ...tab, preview }
    const old = preview ? workspace.tabs.findIndex(open => open.preview) : -1
    workspace.tabs = old < 0
      ? [...workspace.tabs, next]
      : workspace.tabs.map((open, at) => at === old ? next : open)
  }
  setPane("content")
  return activate(tab.id)
}

/// Keeps a preview tab: the next single click opens beside it instead of over
/// it. Also what an edit should call, once this column can edit — the viewer
/// is read-only today, so nothing does yet.
export function pinTab(id) {
  const found = workspace.tabs.find(open => open.id === id)
  if (!found?.preview) return
  workspace.tabs = workspace.tabs.map(open => open.id === id ? { ...open, preview: false } : open)
}

export function closeTab(id) {
  const at = workspace.tabs.findIndex(tab => tab.id === id)
  if (at < 0) return
  const left = workspace.tabs.filter(tab => tab.id !== id)
  workspace.tabs = left
  if (workspace.active !== id) return
  // Falls to the neighbour on the right, then on the left: closing the tab you
  // are looking at should land you next to it rather than at the far end.
  const next = left[at] || left[at - 1] || null
  if (next) {
    activate(next.id)
  } else {
    workspace.active = null
    workspace.selected = null
    workspace.content = null
  }
}

export function activate(id) {
  const tab = workspace.tabs.find(open => open.id === id)
  if (!tab) return
  workspace.active = id
  // A **copy**, never the element out of `tabs`. Putting the same object at two
  // paths in a reactive store means every read of `selected` re-wraps what is
  // already wrapped under `tabs`, the identity changes on every pass, and jq79
  // gives up after 100 of them with "an effect re-woke itself" — which names
  // neither this line nor this file. Copying is what makes the two paths two
  // values. See `RECORD/2026-09-16.three-columns-that-each-have-a-footer.completed.md`.
  workspace.selected = { ...tab }
  if (tab.kind === "file") return loadFile(tab)
  if (tab.kind === "diff") return loadDiff(tab)
  // A debug tab carries its own text: there is nothing to fetch, because the
  // context panel already had it.
  workspace.content = { kind: "debug", text: tab.text, path: tab.title }
  workspace.loading = false
}


/// `pinned` is the double click and the pencil; anything else is a preview.
export function showFile(path, { pinned = false } = {}) {
  return openTab(
    { id: fileId(path), kind: "file", path, title: basename(path), staged: false },
    { preview: !pinned },
  )
}

export function showDiff(path, staged = false, { pinned = false } = {}) {
  return openTab(
    { id: diffId(path), kind: "diff", path, staged, title: basename(path) },
    { preview: !pinned },
  )
}

/// A prompt, a trace, a tool's output: text the context panel already has, in
/// the column that is wide enough to read it. The one thing a 20rem rail has
/// never been able to show.
export function showDebug(id, title, text) {
  return openTab({ id: `debug:${id}`, kind: "debug", path: title, title, text })
}

/// `quiet` is a re-read of the tab already on screen, because the file changed
/// under it: no `loading`, which would blank the column and lose the scroll,
/// and nothing replaced when nothing changed.
async function loadFile(tab, { quiet = false } = {}) {
  if (!quiet) workspace.loading = true
  try {
    const file = await ask(`./api/workspace/file?path=${encodeURIComponent(tab.path)}`)
    // Dropped if the person clicked something else while this was in flight:
    // the column's tab and its body have to be the same file.
    if (workspace.active !== tab.id) return
    if (quiet && same(workspace.content, { kind: "file", ...file })) return
    // The rows themselves are not stored: `content.lines` is what the viewer
    // paints from, and `rows.js` owns the two passes that put them on screen.
    // They used to be three reactive fields here — `head`, `tail` and
    // `pending` — and all three existed only to work around a `:each` that
    // re-derives its list rather than appending to it. See
    // `RECORD/2026-09-16.the-viewer-in-plain-js.completed.md`.
    workspace.content = { kind: "file", ...file }
    workspace.error = null
  } catch (e) {
    workspace.content = null
    workspace.error = `Could not read ${tab.path}: ${e.message}`
  } finally {
    if (workspace.active === tab.id) workspace.loading = false
  }
}

async function loadDiff(tab, { quiet = false } = {}) {
  if (!quiet) workspace.loading = true
  try {
    const diff = await ask(
      `./api/workspace/git-diff?path=${encodeURIComponent(tab.path)}&staged=${tab.staged}`,
    )
    if (workspace.active !== tab.id) return
    if (quiet && same(workspace.content, { kind: "diff", ...diff })) return
    workspace.content = { kind: "diff", ...diff }
    workspace.error = null
  } catch (e) {
    workspace.content = null
    workspace.error = `Could not diff ${tab.path}: ${e.message}`
  } finally {
    if (workspace.active === tab.id) workspace.loading = false
  }
}

/// One file's diff, without a tab: what the content column's foot shows in a
/// modal. The same endpoint a diff tab reads, so the two never disagree.
export function readDiff(path, staged = false) {
  return ask(`./api/workspace/git-diff?path=${encodeURIComponent(path)}&staged=${staged}`)
}

// ---- following the disk -----------------------------------------------------
//
// The server watches the base and says what changed, in batches (see
// `crate::workspace::Watch`). What is re-read is what is on screen and nothing
// else: the open directories a change sits under, git, and the tab being
// looked at. Another tab re-reads when it is activated, as it always did. See
// `RECORD/2026-10-01.the-tree-follows-the-disk.completed.md`.

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b)

/// Every open directory a path sits under, the base included: a directory's
/// row carries the loudest git letter beneath it, so a write three levels
/// down can change the letter on all three.
function openAbove(path) {
  const parts = path.split("/")
  const above = []
  for (let n = 0; n < parts.length; n++) {
    const dir = parts.slice(0, n).join("/")
    if (workspace.open[dir]) above.push(dir)
  }
  return above
}

function forget(key) {
  const open = { ...workspace.open }
  const expanded = { ...workspace.expanded }
  delete open[key]
  delete expanded[key]
  workspace.open = open
  workspace.expanded = expanded
}

const parentOf = path => path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : ""

/// Re-lists open directories, shallowest first, so a directory whose parent's
/// new listing no longer has it is forgotten without being asked for: a
/// folder that was deleted is not an error, and asking would log a 404 for it.
async function relist(keys) {
  const byDepth = new Map()
  for (const key of keys) {
    const depth = key ? key.split("/").length : 0
    byDepth.set(depth, [...(byDepth.get(depth) || []), key])
  }
  for (const depth of [...byDepth.keys()].sort((a, b) => a - b)) {
    await Promise.all(byDepth.get(depth).map(async key => {
      const parent = workspace.open[parentOf(key)]
      if (key && parent && !parent.some(entry => entry.dir && entry.path === key)) return forget(key)
      try {
        const tree = await ask(`./api/workspace/tree?path=${encodeURIComponent(key)}`)
        if (workspace.open[key]) workspace.open = { ...workspace.open, [key]: tree.entries }
      } catch {
        forget(key)
      }
    }))
  }
}

/// A file or a folder renamed from the tree's pencil: the one write the tree
/// makes. Answers `{ ok, path }` with the new path, or `{ ok: false, error }`
/// in the server's words — a name that is taken, a posture that only reads.
///
/// **Everything keyed by the old path moves with it**: a folder stays open
/// under its new name, and so does everything open inside it, and a tab on
/// the file — or on anything under the folder — follows rather than being
/// left on a path that no longer exists. The disk's own event arrives after
/// this and finds nothing left to do. See
/// `RECORD/2026-10-21.the-tree-renames.completed.md`.
export async function renameEntry(path, name) {
  let answer
  try {
    answer = await fetch("./api/workspace/rename", {
      method: "POST",
      headers: { ...apiHeaders(), "Content-Type": "application/json" },
      body: JSON.stringify({ path, name }),
    })
  } catch (e) {
    return { ok: false, error: e.message }
  }
  if (!answer.ok) return { ok: false, error: (await answer.text()) || `HTTP ${answer.status}` }
  const renamed = (await answer.json()).path
  if (renamed === path) return { ok: true, path }

  const moved = key => key === path ? renamed
    : key.startsWith(`${path}/`) ? renamed + key.slice(path.length)
    : null
  // The listings under it name the old paths in every entry, so they are
  // dropped and read again under the new ones; what was expanded stays so.
  const reopen = Object.keys(workspace.expanded).map(moved).filter(key => key !== null)
  workspace.open = Object.fromEntries(Object.entries(workspace.open).filter(([key]) => moved(key) === null))
  workspace.expanded = Object.fromEntries(
    Object.entries(workspace.expanded).map(([key, on]) => [moved(key) ?? key, on]))

  const idOf = tab => tab.kind === "diff" ? diffId(tab.path) : fileId(tab.path)
  let active = null
  workspace.tabs = workspace.tabs.map(tab => {
    const to = (tab.kind === "file" || tab.kind === "diff") && moved(tab.path)
    if (!to) return tab
    const next = { ...tab, path: to, title: basename(to) }
    next.id = idOf(next)
    if (tab.id === workspace.active) active = next
    return next
  })
  if (active) {
    workspace.active = active.id
    workspace.selected = { ...active }
    if (active.kind === "file") loadFile(active, { quiet: true })
    else loadDiff(active, { quiet: true })
  }

  await relist(new Set([parentOf(path)]))
  await Promise.all(reopen.map(key => openDir(key, true)))
  statusSoon()
  return { ok: true, path: renamed }
}

/// `git status` at most once a second, and once more after a burst. Not only
/// for a build's thousand writes: `git status` itself rewrites the index now
/// and then, which comes back here as a change to git, and unthrottled that
/// is a loop.
let statusAt = 0
let statusLater = null
function statusSoon() {
  if (statusLater) return
  const wait = Math.max(0, statusAt + 1000 - Date.now())
  statusLater = setTimeout(() => {
    statusLater = null
    statusAt = Date.now()
    loadStatus()
  }, wait)
}

async function changed({ paths = [], git = false, all = false }) {
  const keys = new Set(all || git ? Object.keys(workspace.open) : paths.flatMap(openAbove))
  await relist(keys)
  statusSoon()

  const tab = workspace.tabs.find(open => open.id === workspace.active)
  if (!tab || (tab.kind !== "file" && tab.kind !== "diff")) return
  // A diff moves with the index as well as with the file: staging it is a
  // change to git and not to any path.
  const touched = all || paths.includes(tab.path) || (git && tab.kind === "diff")
  if (!touched) return
  if (tab.kind === "file") loadFile(tab, { quiet: true })
  else loadDiff(tab, { quiet: true })
}

/// One batch handled at a time. `changed` awaits its re-reads, and the stream
/// does not wait for it: a batch that arrives meanwhile is folded into the
/// next run rather than starting a second set of the same requests beside it.
let pending = null
let draining = false
async function follow(changes) {
  pending = pending
    ? {
        paths: [...new Set([...(pending.paths || []), ...(changes.paths || [])])],
        git: pending.git || changes.git,
        all: pending.all || changes.all,
      }
    : changes
  if (draining) return
  draining = true
  try {
    while (pending) {
      const next = pending
      pending = null
      await changed(next)
    }
  } finally {
    draining = false
  }
}

/// Calls `each` with every `data:` an event stream carries, until it ends.
async function readEvents(body, each) {
  const reader = body.pipeThrough(new TextDecoderStream()).getReader()
  let buffer = ""
  for (;;) {
    const { value, done } = await reader.read()
    if (done) return
    buffer += value
    let end
    while ((end = buffer.indexOf("\n\n")) >= 0) {
      const event = buffer.slice(0, end)
      buffer = buffer.slice(end + 2)
      // Lines starting `:` are the server's keep-alive and say nothing.
      const data = event.split("\n").filter(line => line.startsWith("data:"))
        .map(line => line.slice(5).trimStart()).join("\n")
      if (data) each(JSON.parse(data))
    }
  }
}

let watching = false

/// Follows the disk for as long as the page is open. Read with `fetch` rather
/// than `EventSource`, which cannot send the bearer header a guarded server
/// asks for.
///
/// A dropped stream is reconnected every three seconds, and the panels catch
/// up once it is back, since anything could have changed while nobody was
/// listening. A server that answers with an error — one too old to have the
/// route, or a refusal — is not asked again: the refresh button still works.
export async function watchWorkspace() {
  if (watching) return
  watching = true
  let dropped = false
  for (;;) {
    let answer = null
    try {
      answer = await fetch("./api/workspace/events", { headers: apiHeaders() })
    } catch {
      // The server is gone; it may come back.
    }
    if (answer && !answer.ok) {
      watching = false
      return
    }
    if (answer?.body) {
      if (dropped) follow({ all: true })
      try {
        await readEvents(answer.body, changes => { follow(changes) })
      } catch {
        // Cut mid-stream: the same as ending.
      }
    }
    dropped = true
    await new Promise(again => setTimeout(again, 3000))
  }
}
