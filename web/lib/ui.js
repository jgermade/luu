// @ts-check
/// The page's own chrome state: which settings section is open, the session
/// starter, the session history, and whether the model this session names is
/// actually there.
///
/// A module for the reason `workspace.js` is one: the settings modal, the chat's
/// head and the chat's foot all touch these, and they are siblings. The rule
/// that keeps it from becoming a junk drawer is that **nothing in here is a
/// preference and nothing in here is session state** — preferences are
/// `prefs.js` and are remembered; the session is `store.js` and is the server's.
/// This is what is open on screen right now. See
/// `RECORD/2026-09-16.three-columns-that-each-have-a-footer.completed.md`.

import { $reactive } from "@web/vendor/jq79.js"
import { prefs, setStart } from "./prefs.js"
import { runtimeOptions } from "./places.js"
import { loadPolicies } from "./policies.js"
import {
  state, openSettings, loadProviders, loadPostures, providerModels,
  newSession, resumeSession, apiHeaders, refreshSettings,
} from "./store.js"

export const ui = $reactive({
  /// Which section the settings modal is showing: `general` or `models`.
  /// Sections rather than a scroll, because they are not steps.
  section: "general",
  /// The session starter, or `null` when it is closed. `{ provider, model,
  /// models, reason, from, loading, runtime, lists, policies }`.
  starter: null,
  starting: false,
  /// Which of the starter's two things is running, so the one that is says so
  /// on its own button rather than on both.
  continuing: false,
  /// The sessions dialog's tab — `new` (start one) or `history` (open an
  /// earlier one) — or `null` while it is closed. One dialog with two tabs:
  /// the `+` and the clock in the chat's head each open it on their own. See
  /// `RECORD/2026-10-21.the-design-system-applied.completed.md`.
  sessions: null,
  /// The sessions dialog on its way out: `:open` goes false and it plays its
  /// exit, and `sessions` goes `null` — unmounting it — once that is over. A
  /// dialog unmounted at once has no exit to play.
  sessionsLeaving: false,
  /// The permissions dialog's tab — `activity`, `granted`, `forbidden` or
  /// `policies` — or `null` while it is closed, and the same way out as the
  /// sessions dialog. What the session did and what decided it, so the lists
  /// can start lax and be narrowed from it. See `RECORD/2026-10-22.policies-not-postures.WIP.md`.
  permissions: null,
  permissionsLeaving: false,
  /// Two columns at a width that has room for three, chosen from the chat's
  /// head for as long as the page is open. Not a preference: `General →
  /// Columns` is the setting, and this is the button beside the work.
  twoColumns: false,
  /// Whether the model this session names is pulled where it sends:
  /// `{ verdict: "ok" | "missing" | "unreachable", detail }`, or `null` until
  /// asked. See `checkModel` for why only one of the three disables anything.
  reach: null,
  /// The engine luu starts for this session, while it matters: `{ name,
  /// state, model, from, secs, error }` — see `watchEngine` for which states
  /// are kept and for how long — or `null`. A switch no longer waits for the
  /// model to load, so this is where the wait is seen instead. See `watchEngine`.
  engine: null,
  /// An engine's sheet Settings → Engines opens on once it has loaded —
  /// `{ name, tab }` — asked for from outside Settings: the engine's state
  /// in the chat's foot opens its output. `null` once taken.
  engineSheet: null,
  /// The model the last switch left, while its feedback is on screen.
  switchedFrom: null,
  /// Which turn the inspection modal shows, or `null` when it is closed.
  inspecting: null,
  /// Whether the context modal is open: what the window holds as of the
  /// newest turn, opened from the indicator in the chat's foot.
  contextOpen: false,
  /// The path whose diff the content column's foot opened in a modal, or
  /// `null`. See `views/editor/diff-modal.html`.
  diff: null,
  /// Whether the terminal panel is drawn under the editor. Hiding it leaves
  /// the shell running — see `lib/terminal.js`.
  terminal: false,
  /// Which job ids the page has already answered on the person's behalf, so an
  /// automatic approval happens once per proposal rather than on every render.
  answered: [],
})

/// `sheet` is an engine's — `{ name, tab }` — for `engines`.
export function openSettingsAt(section, sheet = null) {
  ui.section = ["models", "appearance", "engines", "policies"].includes(section) ? section : "general"
  ui.engineSheet = section === "engines" ? sheet : null
  return openSettings()
}

export function showSection(section) {
  ui.section = section
}

export function openHistory() {
  stayOpen()
  ui.sessions = "history"
}

/// The other tab, from inside the dialog. New prepares the starter the first
/// time it is shown, as the `+` would have.
export function showSessionsTab(tab) {
  stayOpen()
  ui.sessions = tab
  if (tab === "new" && !ui.starter) openStarter()
}

// The way out, in two steps: the dialog is told it is closed and plays its
// exit, and is unmounted when the exit is over — the theme's own duration, so
// reduced motion is no wait at all.
let leaving = null
const exitMs = () => {
  const ms = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--luu-duration"))
  return Number.isFinite(ms) ? ms : 0
}
/// Opened again while it was leaving: it stays.
function stayOpen() {
  clearTimeout(leaving)
  ui.sessionsLeaving = false
}

export function closeSessions() {
  if (!ui.sessions || ui.sessionsLeaving) return
  ui.sessionsLeaving = true
  clearTimeout(leaving)
  leaving = setTimeout(() => {
    ui.sessions = null
    ui.starter = null
    ui.continuing = false
    ui.sessionsLeaving = false
  }, exitMs())
}

// ---- permissions -----------------------------------------------------------

const PERMISSION_TABS = ["activity", "granted", "forbidden", "policies"]
let permissionsLeaving = null

/// Opens on the tab asked for, and asks for the lists again: Settings may
/// have been read before this session's environment was.
export function openPermissions(tab = "activity") {
  clearTimeout(permissionsLeaving)
  ui.permissionsLeaving = false
  ui.permissions = PERMISSION_TABS.includes(tab) ? tab : "activity"
  return refreshSettings()
}

export function showPermissionsTab(tab) {
  if (PERMISSION_TABS.includes(tab)) ui.permissions = tab
}

/// Plays the dialog's way out, and resolves once it has gone — so whatever
/// opens next waits for it. A dialog opened again meanwhile stays, and the
/// promise never resolves: nothing should open over it.
let permissionsClosed = Promise.resolve()
export function closePermissions() {
  if (!ui.permissions) return Promise.resolve()
  if (ui.permissionsLeaving) return permissionsClosed
  ui.permissionsLeaving = true
  clearTimeout(permissionsLeaving)
  permissionsClosed = new Promise(gone => {
    permissionsLeaving = setTimeout(() => {
      ui.permissions = null
      ui.permissionsLeaving = false
      gone()
    }, exitMs())
  })
  return permissionsClosed
}

/// From the permissions dialog to a section of Settings: the dialog leaves
/// first, with its exit, and Settings opens after. Opened at once, Settings
/// unmounted the dialog mid-exit — the page mounts it only while Settings is
/// closed — and it vanished.
export async function permissionsToSettings(section) {
  await closePermissions()
  return openSettingsAt(section)
}

// ---- is the model actually there ------------------------------------------

/// Asks the provider what it has, and compares.
///
/// **Three answers, and only one of them disables anything.**
///
/// - the model is in the list — nothing is shown.
/// - the provider answered a list and the model is not in it — `missing`. This
///   is the case worth building: it is a typo or a model nobody pulled, it is
///   certain, and today it costs a whole turn at the far end to discover.
/// - the provider did not answer — `unreachable`, and the composer stays
///   enabled. A provider that is starting up is an ordinary state, and a page
///   that locked its composer over one would be wrong far more often than it
///   was right.
export async function checkModel() {
  watchEngine()
  const profile = state.settings?.profile
  const model = state.settings?.model
  // Nothing named a profile — the mock, or a destination given on the command
  // line. There is no list to ask for, so there is nothing to contradict.
  if (!profile || !model) {
    ui.reach = null
    return
  }
  const answer = await providerModels(profile)
  if (answer.reason || !answer.models?.length) {
    ui.reach = {
      verdict: "unreachable",
      detail: answer.reason || "it offered no list",
    }
    return
  }
  ui.reach = answer.models.includes(model)
    ? { verdict: "ok", detail: "" }
    : { verdict: "missing", detail: `${profile} has no ${model}` }
}

// ---- starting a session ---------------------------------------------------
//
// `null` when the dialog is closed. A session picks where it sends and which
// model there; the destination becomes the *session's*, and this is the only
// moment it can be chosen — see
// `RECORD/2026-09-07.the-first-run-has-no-provider.completed.md`.

export async function openStarter(provider) {
  stayOpen()
  ui.sessions = "new"
  if (!state.providers) await loadProviders()
  // What this session may be allowed to do, offered beside where it sends —
  // and chosen at the same moment, because it is the only one there is: what
  // a session may do is what its jobs are approved against, so it never moves.
  // Asked again on every opening, with the lists: Settings → Policies may
  // have changed them since, and the images the runtimes offer with them.
  const [policies] = await Promise.all([loadPolicies(), loadPostures()])
  ui.continuing = false
  const names = Object.keys(state.providers?.providers || {})
  const chosen = provider
    || state.providers?.running
    || state.providers?.default
    || names[0]
    || ""
  ui.starter = {
    provider: chosen,
    model: "",
    models: [],
    reason: null,
    from: null,
    loading: false,
    // The tables of `config.toml`, and what the session is made from: where
    // it runs and one list of each axis by name, `""` for the empty one.
    // What the last session started with, where it still can; else this
    // machine and the empty lists — read the folder, ask the rest.
    policies: policies.ok ? policies.view : null,
    ...rememberedStart(policies.ok ? policies.view : null),
  }
  if (chosen) await loadModels(chosen)
}

const KINDS = ["shell", "network", "files"]

/// The runtime and lists the last session started with, each where it still
/// is: a runtime this machine still offers, a list `config.toml` still has.
/// The runtime Settings → Runtimes marks as the default comes first, where it
/// is offered: the first of its places. See
/// `RECORD/2026-10-27.a-session-starts-on-the-default-runtime.completed.md`.
function rememberedStart(policies) {
  let kept = {}
  try {
    kept = JSON.parse(prefs.start || "{}") || {}
  } catch {}
  const options = runtimeOptions(state.postures)
  const preferred = state.postures?.default_runtime
  const runtime = (preferred && options.find(option => option.runtime === preferred && option.image && !option.disabled)?.value)
    || options.find(option => option.value === kept.runtime && !option.disabled)?.value
    || options.find(option => !option.disabled)?.value
    || "host|"
  const lists = Object.fromEntries(KINDS.map(kind => {
    const name = kept.lists?.[kind]
    return [kind, name && policies?.[kind]?.[name] ? name : ""]
  }))
  return { runtime, lists }
}

/// Where the session's commands run: `runtime|image`.
export function chooseRuntime(value) {
  ui.starter = { ...ui.starter, runtime: value }
}

/// One of the three lists, by name; `""` is the empty one.
export function chooseList(kind, name) {
  ui.starter = { ...ui.starter, lists: { ...ui.starter.lists, [kind]: name } }
}

/// The list is fetched from the provider itself, so it is what is *pulled*
/// rather than what somebody remembered. An empty one with a reason is the
/// ordinary "that server is not running right now", and the field stays
/// typable: a model not pulled yet is a legitimate thing to write down.
export async function loadModels(name) {
  ui.starter = { ...ui.starter, provider: name, loading: true, models: [], reason: null }
  const answer = await providerModels(name)
  ui.starter = {
    ...ui.starter,
    loading: false,
    models: answer.models || [],
    reason: answer.reason || null,
    from: answer.suggested_from || null,
    model: answer.suggested || "",
  }
}

export function editStarter(patch) {
  ui.starter = { ...ui.starter, ...patch }
}

export function closeStarter() {
  closeSessions()
}

export async function startSession() {
  ui.starting = true
  // Every session is made from a runtime and three lists, the empty ones
  // left out — which the server reads as empty. Over stdio the process is
  // the session, and there is nothing to choose.
  const choosable = !!state.postures?.choosable
  const [runtime, image] = (ui.starter.runtime || "host|").split("|")
  const lists = Object.fromEntries(KINDS.map(kind => [kind, ui.starter.lists?.[kind]]).filter(([, name]) => name))
  const ok = await newSession({
    provider: ui.starter.provider,
    model: ui.starter.model,
    ...(choosable ? { runtime, ...(image ? { image } : {}), lists } : {}),
  })
  ui.starting = false
  if (ok) {
    setStart(JSON.stringify({ runtime: ui.starter.runtime, lists: ui.starter.lists }))
    closeSessions()
    await checkModel()
  }
  return ok
}

/// The same two answers, against the session already on screen. Its history
/// comes along, and the server writes a header into its stream saying where it
/// changed — so turn 8 on another model is not silently comparable with turn 7
/// on this one.
export async function continueHere() {
  ui.starting = true
  ui.continuing = true
  const ok = await resumeSession(state.session, {
    provider: ui.starter.provider,
    model: ui.starter.model,
  })
  ui.starting = false
  ui.continuing = false
  if (ok) {
    closeSessions()
    await checkModel()
  }
  return ok
}

// ---- is the engine up -----------------------------------------------------

/// The states of an engine that is on its way up — `engines::IN_PROGRESS`.
export const LOADING = ["stopping", "downloading", "starting"]

let watching = null
let settling = null

/// Polls `GET /api/engine` until the live session's engine has loaded its
/// model, or failed to. One poll at a time: a second switch while the first
/// is loading restarts the loop rather than running two.
///
/// What it leaves in `ui.engine`: the phase while one is under way, `failed`
/// until the next switch, and `ready` for a few seconds after a load this page
/// watched — so the feedback beside the picker ends in an answer rather than
/// in nothing. An engine that was already up is `null`: nothing happened.
///
/// Nothing here disables the composer, for `checkModel`'s own reason — a turn
/// sent now waits on the server for the same load, and says so if it fails.
export function watchEngine() {
  if (watching) clearTimeout(watching)
  const poll = async () => {
    watching = null
    let answer = null
    try {
      const res = await fetch("./api/engine", { headers: apiHeaders() })
      if (res.ok) answer = await res.json()
    } catch {
      // The server restarting; the next switch or reload asks again.
    }
    const was = ui.engine?.state
    if (answer && LOADING.includes(answer.state)) {
      if (settling) clearTimeout(settling)
      ui.engine = answer
      watching = setTimeout(poll, 700)
    } else if (answer?.state === "failed") {
      ui.engine = answer
    } else if (answer?.state === "ready" && LOADING.includes(was)) {
      ui.engine = { ...answer, took: ui.engine.secs }
      settling = setTimeout(() => { ui.engine = null, ui.switchedFrom = null }, 3000)
    } else if (!LOADING.includes(was) && was !== "ready") {
      ui.engine = null
      ui.switchedFrom = null
    }
  }
  poll()
}

let watchedTurn = null

/// `watchEngine` once per turn, for the page's reactive line. That line runs
/// on every change to `state`, and a streaming answer changes it once per
/// token — so calling `watchEngine` from it directly was one `GET /api/engine`
/// per token, 40 a second at the peak and 1478 in one day's log. The engine a
/// turn waits on is decided when the turn starts; asking again mid-answer
/// learns nothing.
///
/// Never in a replay: a recorded turn waited on no engine this page can ask
/// about, and the static twin has no `/api/engine` to answer.
export function watchTurnEngine(turn) {
  if (turn != null && turn !== watchedTurn && !state.replay) watchEngine()
  watchedTurn = turn
  return turn
}

/// Said by the picker the moment somebody chooses: the model the session is
/// leaving, for the feedback to name while the new one loads. The server only
/// knows it when one engine restarts on another model (`from`); when the
/// switch moves to another engine the old one was stopped before the answer
/// came back, and this page is the only one that remembers what it ran.
export function switching(from) {
  ui.switchedFrom = from || null
}

// ---- inspecting a turn ----------------------------------------------------

export function inspect(turn) {
  ui.inspecting = turn
}

export function closeInspect() {
  ui.inspecting = null
}

// ---- the context so far -----------------------------------------------------

export function openContext() {
  ui.contextOpen = true
}

export function closeContext() {
  ui.contextOpen = false
}

// ---- one file's changes -----------------------------------------------------

export function openDiff(path) {
  ui.diff = path
}

export function closeDiff() {
  ui.diff = null
}

// ---- the terminal panel ------------------------------------------------------

export function toggleTerminal() {
  ui.terminal = !ui.terminal
}
