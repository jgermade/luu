// @ts-check
/// A dialog as a bottom sheet on a phone: the rule for when, and the pull
/// that closes it. It is the theme's own, shared by `modal.html` and
/// `tab-modal.html`. `components/modal.html` keeps a copy of the same two
/// things, because nothing in `components/` imports from outside it.
///
/// **When.** A phone is narrower than 760px, or a touch screen whose short
/// side is under 760px, so a phone held sideways is still one. The page's
/// slides use the same rule (`lib/phone.js`). See
/// `RECORD/2026-11-08.every-dialog-a-sheet-on-a-phone.completed.md`.
///
/// **The pull.** Only the head drags, and only from what is not a control.
/// Past a quarter of the sheet's height, or a flick (above 0.5 px/ms over
/// the last move), the drag asks to close. Anything less springs back.

export const PHONE = "(max-width: 760px), (pointer: coarse) and (max-height: 760px)"

/** @returns {boolean} */
export const isPhone = () =>
  typeof window !== "undefined" && window.matchMedia(PHONE).matches

/**
 * Calls `then` whenever the answer to `isPhone()` changes. Returns what stops it.
 * @param {(phone: boolean) => void} then
 */
export function watchPhone(then) {
  const query = window.matchMedia(PHONE)
  const changed = () => then(query.matches)
  query.addEventListener("change", changed)
  return () => query.removeEventListener("change", changed)
}

const CONTROLS = "button, a, input, select, textarea, [role=tab]"

/**
 * Pointer handlers for a sheet's head.
 * @param {{ enabled: () => boolean, move: (dy: number, held: boolean) => void, release: (close: boolean) => void }} on
 */
export function grip(on) {
  /** @type {null | { y: number, lastY: number, lastAt: number, v: number, dy: number }} */
  let held = null
  return {
    /** @param {PointerEvent} event */
    down(event) {
      if (!on.enabled() || event.button > 0) return
      if (/** @type {Element} */ (event.target).closest?.(CONTROLS)) return
      const now = performance.now()
      held = { y: event.clientY, lastY: event.clientY, lastAt: now, v: 0, dy: 0 }
      // A pointer the browser no longer knows (a synthetic one) cannot be
      // captured, and the drag works without it while it stays on the head.
      try {
        /** @type {Element} */ (event.currentTarget).setPointerCapture?.(event.pointerId)
      } catch {}
      on.move(0, true)
    },
    /** @param {PointerEvent} event */
    move(event) {
      if (!held) return
      const now = performance.now()
      held.v = (event.clientY - held.lastY) / Math.max(1, now - held.lastAt)
      held.lastY = event.clientY
      held.lastAt = now
      held.dy = Math.max(0, event.clientY - held.y)
      on.move(held.dy, true)
    },
    /** @param {PointerEvent} event */
    up(event) {
      if (!held) return
      const { dy, v } = held
      held = null
      const sheet = /** @type {Element} */ (event.currentTarget).closest?.("dialog")
      const tall = /** @type {HTMLElement | null} */ (sheet)?.offsetHeight || window.innerHeight
      const close = dy > tall / 4 || (v > 0.5 && dy > 24)
      on.move(close ? dy : 0, false)
      on.release(close)
    },
  }
}
