// @ts-check
/// Whether this screen is a phone, by the one rule every part of the page uses.
///
/// A phone is narrower than 760px, **or** a touch screen whose short side is
/// under 760px. The second half is what keeps a phone a phone when it is held
/// sideways, where it is 844 wide and 390 tall. A desktop window made that
/// short has a fine pointer and is not caught by it. See
/// `RECORD/2026-11-08.a-phone-held-sideways.completed.md`.
///
/// CSS says the same as `(max-width: 760px), (pointer: coarse) and
/// (max-height: 760px)` (see `project.html`). Here it reads `innerWidth` and
/// `innerHeight`, so an emulated viewport and a real one agree.

export const NARROW = 760

const touch = typeof window === "undefined" ? null : window.matchMedia("(pointer: coarse)")

/** @returns {boolean} */
export function isPhone() {
  if (typeof window === "undefined") return false
  return (
    window.innerWidth < NARROW ||
    (!!touch?.matches && Math.min(window.innerWidth, window.innerHeight) < NARROW)
  )
}

/**
 * Calls `then` with `isPhone()` now and again whenever the answer changes.
 * Returns what stops it.
 * @param {(phone: boolean) => void} then
 */
export function onPhone(then) {
  let last = isPhone()
  then(last)
  const check = () => {
    const now = isPhone()
    if (now !== last) then((last = now))
  }
  window.addEventListener("resize", check)
  return () => window.removeEventListener("resize", check)
}
