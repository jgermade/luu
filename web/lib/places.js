// @ts-check
/// Where a session's commands may run, as a list a picker offers: this
/// machine, and every image `config.toml` names on every container runtime.
/// One list for the session starter, which opens a session, and the Runtime
/// dialog, which moves one. Nothing here is a name: a session is a runtime
/// and three lists it copied, and where it runs is the only part that moves.
/// See `RECORD/2026-10-01.the-terminal-follows-the-session.completed.md` and
/// `RECORD/2026-11-10.a-session-is-a-runtime-and-three-lists.completed.md`.
import { brandSrc, runtimeBrand } from "./brands.js"

/// How a place reads: by where it runs, which is the question both pickers
/// answer.
const runs = place => !place ? "?"
  : place.runtime === "direct" ? "direct (no container)"
  : place.image ? `${place.runtime} · ${place.image}` : place.runtime

/// Where the live session runs, drawn as `runtimeOptions` draws a place,
/// from `/api/terminal`'s `posture`: `composed` — the runtime and image it
/// was made on — or the worker's label (`docker (luu-worker:dev)`, `host`,
/// `direct (no container)`). `null` until the server has said.
export function hereOption(live) {
  if (!live) return null
  const [word, rest] = (live.runtime || "").split(" ")
  const runtime = live.composed?.runtime || word || "host"
  const image = live.composed?.image || (rest?.startsWith("(") && rest !== "(no" ? rest.slice(1, -1) : null)
  const place = { runtime, image }
  const name = live.name || "this session"
  return {
    value: `${runtime}|${image || ""}`,
    runtime,
    where: runtime,
    name,
    image,
    label: runs(place),
    title: `${name} · ${runs(place)}`,
    disabled: false,
  }
}

/// The mark a place wears, by the runtime it runs on: this machine for `host`
/// and `direct`, the runtime's brand where there is one, and a box for a
/// container runtime with none — `{ src, glyph }`, one of them set.
export function placeMark(where) {
  if (!where || where === "host" || where === "direct") return { src: null, glyph: "#i-machine" }
  const src = brandSrc(runtimeBrand(where))
  return { src, glyph: src ? null : "#i-box" }
}

/// Where a session may run, out of `/api/postures`'
/// `runtimes`: this machine, and every image `config.toml` declares on every
/// container runtime — `[{ value, runtime, image, where, label, title, gap,
/// disabled }]`, the value `runtime|image`. The new session's *Runtime*, and
/// where the Runtime dialog moves a session; see `RECORD/2026-10-30.a-session-carries-its-policies.completed.md` §2.
/// `gap` is what that place cannot hold: the container's network on Apple's
/// `container`, or this machine's kernel.
export function runtimeOptions(postures) {
  return (postures?.runtimes || []).map(place => ({
    value: `${place.runtime}|${place.image || ""}`,
    runtime: place.runtime,
    image: place.image || null,
    where: place.runtime,
    label: runs(place),
    title: place.missing || place.gap || "",
    gap: place.gap || null,
    disabled: !!place.missing,
  }))
}
