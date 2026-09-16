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

import { $reactive } from "./vendor/jq79.js"
import {
  state, openSettings, loadProviders, loadPostures, providerModels,
  newSession, resumeSession,
} from "./store.js"

export const ui = $reactive({
  /// Which section the settings modal is showing: `general` or `models`.
  /// Sections rather than a scroll, because they are not steps.
  section: "general",
  /// The session starter, or `null` when it is closed. `{ provider, model,
  /// models, reason, from, loading, posture }`.
  starter: null,
  starting: false,
  /// Which of the starter's two things is running, so the one that is says so
  /// on its own button rather than on both.
  continuing: false,
  /// Whether the session history is open. It is the old tab strip, as a
  /// popover: one session is on screen, so a strip was spending a row to
  /// answer a question the column's head answers in words.
  history: false,
  /// Whether the model this session names is pulled where it sends:
  /// `{ verdict: "ok" | "missing" | "unreachable", detail }`, or `null` until
  /// asked. See `checkModel` for why only one of the three disables anything.
  reach: null,
  /// Which job ids the page has already answered on the person's behalf, so an
  /// automatic approval happens once per proposal rather than on every render.
  answered: [],
})

export function openSettingsAt(section) {
  ui.section = section === "models" ? "models" : "general"
  return openSettings()
}

export function showSection(section) {
  ui.section = section
}

export function openHistory() {
  ui.history = true
}

export function closeHistory() {
  ui.history = false
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
  if (!state.providers) await loadProviders()
  // What this session may be allowed to do, offered beside where it sends —
  // and chosen at the same moment, because it is the only one there is: a
  // posture is what a session's jobs are approved against, so it never moves.
  if (!state.postures) await loadPostures()
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
    // "" is the server's own policy file, which is what every session got
    // before a session could choose.
    posture: "",
  }
  if (chosen) await loadModels(chosen)
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
  ui.starter = null
  ui.continuing = false
}

export async function startSession() {
  ui.starting = true
  const ok = await newSession({
    provider: ui.starter.provider,
    model: ui.starter.model,
    posture: ui.starter.posture || undefined,
  })
  ui.starting = false
  if (ok) {
    ui.starter = null
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
    ui.starter = null
    await checkModel()
  }
  return ok
}
