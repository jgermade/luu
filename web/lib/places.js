// @ts-check
/// Where a session's commands may run, as a list a picker offers: the
/// server's own policy file, every `[posture.<name>]` in `config.toml` and
/// every `[environment.<name>]` beside them,
/// each on its own runtime first and — where it is contained — on every other
/// container runtime after it. One list for the terminal's foot, which moves
/// a session, and the session starter, which opens one. See
/// `RECORD/2026-10-01.the-terminal-follows-the-session.completed.md`.
///
/// The value is `posture|runtime`: the posture's name (`""` is the server's
/// own) and the runtime where it is not the one that posture names.
import { brandSrc, runtimeBrand } from "./brands.js"

/// How a place reads: by where it runs, which is the question both pickers
/// answer.
const runs = place => !place ? "?"
  : place.runtime === "direct" ? "direct (no container)"
  : place.image ? `${place.runtime} · ${place.image}` : place.runtime

/// `[{ value, posture, runtime, where, name, label, title, disabled }]` out of what
/// `/api/postures` answered. A runtime this machine does not have is in the
/// list, disabled, saying so.
export function placeOptions(postures) {
  const option = (name, label, place, declared) => {
    const runtime = place?.runtime === declared ? "" : place?.runtime || ""
    return {
      value: `${name}|${runtime}`,
      posture: name,
      runtime,
      // The runtime it runs on, variant or not: what its mark is.
      where: place?.runtime || "",
      name: label,
      label: runs(place),
      title: place?.missing
        ? `${label} — ${place.missing}`
        : `${label} · ${place?.policy || ""}${place?.gap ? " · the container keeps its network" : ""}`,
      disabled: !!place?.missing,
    }
  }
  const variants = (name, label, places) =>
    (places || []).map(place => option(name, label, place, places[0]?.runtime))
  return [
    ...variants("", "server default", postures?.own),
    ...[...Object.keys(postures?.postures || {}), ...(postures?.environments || [])].flatMap(name =>
      variants(name, name, postures?.places?.[name])),
  ]
}

/// Which of `placeOptions` the live session is, from `/api/terminal`'s
/// `posture` — `{ name, runtime }`, the runtime a word and maybe an image
/// after it: its posture, and the runtime only where it is not the one that
/// posture names.
export function livePlace(postures, live) {
  const name = live?.name || ""
  const runtime = (live?.runtime || "").split(" ")[0]
  const declared = (name ? postures?.places?.[name] : postures?.own)?.[0]?.runtime
  return `${name}|${runtime && runtime !== declared ? runtime : ""}`
}

/// The mark a place wears, by the runtime it runs on: this machine for `host`
/// and `direct`, the runtime's brand where there is one, and a box for a
/// container runtime with none — `{ src, glyph }`, one of them set.
export function placeMark(where) {
  if (!where || where === "host" || where === "direct") return { src: null, glyph: "#i-machine" }
  const src = brandSrc(runtimeBrand(where))
  return { src, glyph: src ? null : "#i-box" }
}

/// `value` if it is still a place that can be chosen, else the server's own.
/// A remembered place may name a posture since taken out of `config.toml`, or
/// a runtime this machine no longer has.
export function stillThere(postures, value) {
  const found = placeOptions(postures).find(option => option.value === value && !option.disabled)
  return found ? found.value : "|"
}

/// Where a session made from lists may run, out of `/api/postures`'
/// `runtimes`: this machine, and every image a posture or an environment
/// names on every container runtime — `[{ value, runtime, image, label, title,
/// disabled }]`, the value `runtime|image`. The new session's *Runtime*; see
/// `RECORD/2026-10-22.policies-not-postures.WIP.md`, *every session from lists*.
export function runtimeOptions(postures) {
  return (postures?.runtimes || []).map(place => ({
    value: `${place.runtime}|${place.image || ""}`,
    runtime: place.runtime,
    image: place.image || null,
    label: runs(place),
    title: place.missing || (place.gap ? "the container keeps its network" : ""),
    disabled: !!place.missing,
  }))
}
