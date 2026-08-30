// @ts-check
import { expect, test } from "@playwright/test"

/**
 * The one error the static twin is expected to produce: there is no server
 * behind it, so the socket the live page opens cannot connect, and the page
 * falls back to replaying a recording. Everything else is a real failure —
 * a component that did not mount, a fixture that does not parse, a store that
 * read a field the export does not write.
 */
const EXPECTED = /WebSocket|ws:\/\/|wss:\/\//i

/** Collects console errors and uncaught exceptions for the whole test. */
function watch(page) {
  const errors = []
  page.on("console", message => {
    if (message.type() === "error" && !EXPECTED.test(message.text())) {
      errors.push(`console: ${message.text()}`)
    }
  })
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  return errors
}

test("the page mounts and replays a recording, with nothing in the console", async ({ page }) => {
  const errors = watch(page)

  await page.goto("/index.html")

  // Mounted at all: the components are fetched at runtime, so a broken path
  // here is an empty page rather than a build failure.
  await expect(page.locator("header strong")).toHaveText("loude")

  // The static twin was found and the page fell back to it. The picker is the
  // signal: it renders only when `state.fixtures` was filled, which only the
  // static fallback does.
  //
  // Not the composer, and not the status word. Both say "live" while a recorded
  // turn is mid-replay — `replaying` is `status === "replay"`, and the status is
  // "running" until the turn ends, so the input is enabled and reads "Ask
  // something…" over a recording nobody can send into. Found by this test, left
  // alone by it: it is a real bug and fixing it is not what this change is.
  await expect(page.locator("select.fixtures")).toBeVisible()

  // The recording plays: a prompt and an answer, from the exported session.
  await expect(page.locator(".transcript article.user .text").first()).not.toBeEmpty()
  await expect(page.locator(".transcript article.assistant .text").first()).not.toBeEmpty({
    timeout: 20_000,
  })

  // The panel the whole debug client exists for. `history` is the bucket that
  // was empty in every recording made before eviction worked, so it is the one
  // worth naming here.
  await expect(page.locator(".inspector .legend li")).not.toHaveCount(0)
  await expect(page.locator(".inspector")).toContainText("tokens sent")

  expect(errors, `the page logged: ${errors.join(" | ")}`).toEqual([])
})

test("every recorded session in the picker can be selected", async ({ page }) => {
  const errors = watch(page)

  await page.goto("/index.html")
  const picker = page.locator("select.fixtures")
  await expect(picker).toBeVisible()

  const files = await picker.locator("option").evaluateAll(options =>
    options.map(option => option.value),
  )
  expect(files.length).toBeGreaterThan(1)

  // Each one is a real recording of the real protocol, so a fixture the store
  // cannot read is a format drift — the failure this exists to catch.
  for (const file of files) {
    await picker.selectOption(file)
    await expect(page.locator(".transcript article").first()).toBeVisible()
  }

  expect(errors, `the page logged: ${errors.join(" | ")}`).toEqual([])
})
