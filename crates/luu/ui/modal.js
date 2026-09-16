// @ts-check
/// The page's modals, as the element the platform already has.
///
/// Three of them — settings, the folder picker, the session starter — were a
/// `.modal-backdrop` div with a click handler and no keyboard at all. A
/// `<dialog>` opened with `showModal()` brings four things this page was
/// otherwise going to write by hand: **ESC**, a focus trap, everything behind
/// it made inert, and a `::backdrop` that is one pseudo-element rather than a
/// positioned div holding a `z-index`.
///
/// What `<dialog>` does *not* bring is the page's own state. Whether a modal is
/// on screen is `state.settingsOpen`, `workspace.picker.open` or `ui.starter`,
/// and a dialog the browser closed behind the page's back would leave a
/// component mounted, invisible, and holding the top layer with no way back to
/// it. So every native dismissal is routed to the same close function the close
/// button calls, and the component unmounts the way it always did.
///
/// See `RECORD/2026-09-16.the-modals-are-dialogs.completed.md`.

/// Opens the `<dialog>` with that id as a modal, once the template has put it
/// in the document.
///
/// Called straight from a component's `:setup`, which jq79 runs to completion
/// *before* the template renders — so the element does not exist yet, and this
/// waits a frame for it. The same seam `content-viewer.html` uses for Monaco
/// and for `rows.js`, and for the same stated reason: the id is the one thing
/// both sides can name.
///
/// `locked` is asked at dismissal time rather than read once. The folder picker
/// on a first visit is the case: there is nothing behind it to go back to, so
/// it refuses to close, and a dialog that let ESC through anyway would take the
/// page somewhere it cannot leave.
export function asModal(id, { close, locked = () => false }) {
  requestAnimationFrame(() => {
    const dialog = document.getElementById(id)
    if (!dialog || dialog.open) return

    // ESC — and anything else the browser counts as a dismissal — fires
    // `cancel` first, and `cancel` can be refused.
    dialog.addEventListener("cancel", event => {
      if (locked()) event.preventDefault()
    })

    // Whatever closed it, the page's own state is what decides what is on
    // screen, so it is told rather than inferred.
    dialog.addEventListener("close", () => {
      if (locked()) return dialog.showModal()
      close()
    })

    dialog.showModal()
  })
}

/// Is any modal on screen? What the page's ESC handler asks before doing
/// anything of its own: a `<dialog>` cancels itself, and a keypress that both
/// closed a modal *and* moved the columns underneath it would be one keypress
/// doing two things.
export function modalOpen() {
  return !!document.querySelector("dialog[open]")
}
