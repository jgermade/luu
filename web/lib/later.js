// @ts-check
/// A component the page draws only when a person asks for it — a dialog, a
/// modal, a panel behind a button — asked for the first time it is wanted
/// rather than with the page.
///
/// jq79 0.7.11 starts every download a setup's top level is sure to make, and
/// those files' own, down the tree. So a dialog listed in `app.html`'s
/// `importAll` brought its whole tree to every first load, opened or not:
/// Settings alone was 31 files. A `later()` inside a `$:` is not a top-level
/// import, so the scanner leaves it alone, and a tag whose variable is still
/// `undefined` draws nothing until the component lands:
///
/// ```js
/// let SettingsModal
/// $: if (state.settingsOpen) later("@web/views/settings/settings.html").then(c => { SettingsModal = c })
/// ```
///
/// One definition per file for the visit, so a dialog opened again is the
/// same component rather than its text parsed a second time. A file that
/// failed is asked for again the next time, not remembered.
/// See `RECORD/2026-11-05.what-the-first-load-waits-on.completed.md`.

import { C79 } from "@web/vendor/jq79.js"

/** @type {Map<string, Promise<any>>} */
const asked = new Map()

/**
 * @param {string} specifier an `@web/` one: `C79.importAll` resolves a
 *   relative specifier against the page, not the file that calls it
 * @returns {Promise<any>} the component, or the module for a `.js`
 */
export function later(specifier) {
  let once = asked.get(specifier)
  if (!once) {
    once = C79.importAll([specifier]).then(([component]) => component)
    once.catch(() => asked.delete(specifier))
    asked.set(specifier, once)
  }
  return once
}
