// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * The default theme's demo, at `/theme/default/`: every component renders,
 * says nothing in the console, and answers a person — a click, a key, a
 * keystroke — with the state printed beside it moving. The components are not
 * used by the app yet, so this page is the only place they run. See
 * `RECORD/2026-10-21.a-design-system-of-components.completed.md`.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7904
const BASE = `http://127.0.0.1:${PORT}`

function binary() {
  const named = process.env.LUU_BIN
  const candidates = named
    ? [resolve(root, named)]
    : [join(root, "target/release/luu"), join(root, "target/debug/luu")]
  const found = candidates.find(path => existsSync(path))
  if (!found) {
    throw new Error(
      `no luu binary at ${candidates.join(" or ")} — run \`cargo build --bin luu\`` +
        " or point LUU_BIN at one",
    )
  }
  return found
}

/** @type {import("node:child_process").ChildProcess | null} */
let server = null
/** @type {string} */
let home

test.beforeAll(async () => {
  home = mkdtempSync(join(tmpdir(), "luu-theme-"))
  server = spawn(binary(), ["serve", "--bind", `127.0.0.1:${PORT}`, "--no-store", "--no-log"], {
    cwd: root,
    env: { ...process.env, LUU_HOME: home },
    stdio: "pipe",
  })
  let output = ""
  server.stdout?.on("data", chunk => (output += chunk))
  server.stderr?.on("data", chunk => (output += chunk))
  const deadline = Date.now() + 30_000
  for (;;) {
    if (server.exitCode !== null) throw new Error(`luu serve exited with ${server.exitCode}:\n${output}`)
    try {
      if ((await fetch(`${BASE}/theme/default/`)).ok) return
    } catch {
      // Not listening yet.
    }
    if (Date.now() > deadline) throw new Error(`luu serve never answered:\n${output}`)
    await new Promise(again => setTimeout(again, 200))
  }
})

test.afterAll(() => {
  server?.kill("SIGTERM")
  rmSync(home, { recursive: true, force: true })
})

/** The demo, open, with every console complaint collected. */
async function demo(page) {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error" || message.type() === "warning") {
      errors.push(`${message.type()}: ${message.text()}`)
    }
  })
  await page.setViewportSize({ width: 1280, height: 900 })
  await page.emulateMedia({ colorScheme: "light" })
  await page.goto(`${BASE}/theme/default`)
  await expect(page).toHaveURL(`${BASE}/theme/default/`)
  await expect(page.locator("#modals-title")).toContainText("Modals")
  return errors
}

test("every section renders, and the tokens are read back from the CSS", async ({ page }) => {
  const errors = await demo(page)
  // Every link in the index has its section, and every section its link.
  const ids = await page.locator(".index a").evaluateAll(links => links.map(a => a.hash.slice(1)))
  expect(ids).toHaveLength(27)
  for (const id of ids) await expect(page.locator(`section#${id}`)).toHaveCount(1)
  // The page's ground, as the computed style has it.
  await expect(page.locator(".swatch").first().locator(".value")).toHaveText("#f4f5f8")

  // The theme switch re-points the colours and the swatches follow.
  // Icons, named for whoever cannot see them.
  await page.locator(".top").getByRole("radio", { name: "Dark" }).click()
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark")
  await expect(page.locator(".swatch").first().locator(".value")).toHaveText("#111111")
  // And it is remembered.
  await page.reload()
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark")
  await page.locator(".top").getByRole("radio", { name: "Light" }).click()
  // Auto is the system's, and follows it.
  await page.locator(".top").getByRole("radio", { name: "Auto" }).click()
  await page.emulateMedia({ colorScheme: "dark" })
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark")
  await page.emulateMedia({ colorScheme: "light" })
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light")

  expect(errors, "the page logged errors").toEqual([])
})

test("the index marks the section being read, as the page scrolls", async ({ page }) => {
  const errors = await demo(page)
  const on = page.locator(".index a.on")
  await expect(on).toHaveText("Colours")
  await expect(on).toHaveAttribute("aria-current", "location")

  await page.locator("#select").evaluate(section => section.scrollIntoView())
  await expect(on).toHaveText("Select")
  // A click marks it at once and scrolls there over `--luu-duration-scroll`,
  // not in one jump; the address gets the section's id once it lands.
  const before = await page.evaluate(() => window.scrollY)
  await page.locator(".index a", { hasText: "Tabs" }).click()
  await expect(on).toHaveText("Tabs")
  const midway = await page.evaluate(() => window.scrollY)
  await expect(page).toHaveURL(/#tabs$/)
  const landed = await page.evaluate(() => window.scrollY)
  expect(Math.abs(midway - before)).toBeLessThan(Math.abs(landed - before))
  const top = await page.locator("#tabs").evaluate(el => el.getBoundingClientRect().top)
  expect(Math.abs(top - 96)).toBeLessThan(2)
  // Square on the side the bar is, rounded on the other.
  expect(await on.evaluate(a => getComputedStyle(a).borderTopLeftRadius)).toBe("0px")
  expect(await on.evaluate(a => getComputedStyle(a).borderTopRightRadius)).not.toBe("0px")
  // The last section is short, and is reached at the bottom of the page.
  await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight))
  await expect(on).toHaveText("Modals")
  await page.evaluate(() => window.scrollTo(0, 0))
  await expect(on).toHaveText("Colours")
  await expect(page.locator(".index a.on")).toHaveCount(1)

  expect(errors, "the page logged errors").toEqual([])
})

test("a reload comes back where the page was, and an address with a section opens on it", async ({ page, context }) => {
  const errors = await demo(page)
  await page.locator("#select").evaluate(section => section.scrollIntoView())
  await expect(page.locator(".index a.on")).toHaveText("Select")
  const was = await page.evaluate(() => window.scrollY)
  expect(was).toBeGreaterThan(1000)

  await page.reload()
  await expect(page.locator("#modals-title")).toContainText("Modals")
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(was - 2)
  expect(Math.abs((await page.evaluate(() => window.scrollY)) - was)).toBeLessThan(2)
  await expect(page.locator(".index a.on")).toHaveText("Select")

  // A tab of its own has nothing kept, and opens on the section it names.
  const fresh = await context.newPage()
  await fresh.setViewportSize({ width: 1280, height: 900 })
  await fresh.goto(`${BASE}/theme/default/#tabs`)
  await expect(fresh.locator("#modals-title")).toContainText("Modals")
  await expect.poll(() => fresh.locator("#tabs").evaluate(el => Math.round(el.getBoundingClientRect().top)))
    .toBeLessThan(110)
  await expect(fresh.locator(".index a.on")).toHaveText("Tabs")
  await fresh.close()

  expect(errors, "the page logged errors").toEqual([])
})

test("an input container, a typeahead, and an index inside a box", async ({ page }) => {
  const errors = await demo(page)

  // The box is the control: its name is the field's name, its description
  // the field's description, and an error takes the description's place.
  const containers = page.locator("#input-container")
  const url = containers.getByRole("textbox", { name: "URL" })
  await expect(url).toHaveAccessibleDescription("where the server listens, /v1 included for OpenAI")
  // The name is drawn over the field and lets the pointer through, so a click
  // where it is lands on the field: forced, because the field is what takes it.
  await containers.getByText("URL", { exact: true }).click({ force: true })
  await expect(url).toBeFocused()
  await url.fill("http://127.0.0.1:8080/v1")
  await expect(containers.locator(".state")).toContainText('url: "http://127.0.0.1:8080/v1"')
  const name = containers.getByRole("textbox", { name: "Name" })
  await expect(name).toHaveAttribute("aria-invalid", "true")
  await expect(containers.getByRole("alert")).toHaveText("lowercase letters, digits and dashes")
  await name.fill("workstation")
  await expect(containers.getByRole("alert")).toHaveCount(0)
  await containers.getByRole("combobox", { name: "Backend" }).selectOption("mock")
  await expect(containers.locator(".state")).toContainText("backend: mock")
  // The control fills the box, the name drawn over it: all of the box is it.
  const [frame, field] = await Promise.all([
    containers.locator("label").first().boundingBox(),
    url.boundingBox(),
  ])
  expect(field.height).toBeGreaterThan(frame.height - 4)
  expect(field.width).toBeGreaterThan(frame.width - 4)

  // The same box around a typeahead: named, described, and its list.
  const tc = page.locator("#typeahead-container")
  const tcModel = tc.getByRole("combobox", { name: "Model" })
  await expect(tcModel).toHaveAccessibleDescription("one the provider has pulled")
  await tcModel.fill("gemma")
  await expect(page.getByRole("listbox", { name: "Model" }).getByRole("option")).toHaveCount(2)
  await tcModel.press("ArrowDown")
  await tcModel.press("Enter")
  await expect(tc.locator(".state")).toContainText('taken: "gemma3:12b"')

  // Typed, filtered, chosen by the keyboard; a disabled option is skipped.
  const typeahead = page.locator("#typeahead")
  const model = typeahead.getByRole("combobox", { name: "Model" })
  const list = page.getByRole("listbox", { name: "Model" })
  await model.click()
  await expect(list).toBeVisible()
  await model.fill("gemma")
  await expect(list.getByRole("option")).toHaveCount(2)
  await expect(list.locator("mark").first()).toHaveText("gemma")
  await model.press("ArrowDown")
  await model.press("ArrowDown")
  await expect(model).toHaveAttribute("aria-activedescendant", /-0$/)
  await model.press("Enter")
  await expect(list).toBeHidden()
  await expect(typeahead.locator(".state")).toContainText('taken: "gemma3:12b"')
  // ESC closes the list and leaves what was typed.
  await model.fill("qwen")
  await expect(list).toBeVisible()
  await model.press("Escape")
  await expect(list).toBeHidden()
  await expect(model).toHaveValue("qwen")
  // A click on an option takes it.
  await model.fill("phi")
  await list.getByRole("option", { name: /phi4/ }).click()
  await expect(typeahead.locator(".state")).toContainText('taken: "phi4:14b"')
  // Strict: leaving on something that is no option puts the last one back.
  const strict = typeahead.getByRole("combobox", { name: "Backend, strict" })
  await strict.fill("nothing like it")
  await strict.blur()
  await expect(strict).toHaveValue("ollama")

  // The box's own index follows the box, and the page's stays where it was.
  const pageOn = await page.locator(".index a.on").textContent()
  // The box is a Scrollable: the scroller is its inner element.
  const box = page.locator(".mini [data-scrollable]")
  await box.evaluate(el => { el.scrollTop = el.scrollHeight })
  await expect(page.locator(".mini a.on")).toHaveText("Fold")
  await expect(page.locator("#anchors .state")).toContainText("mini-fold")
  await expect(page.locator(".index a.on")).toHaveText(pageOn)
  await box.evaluate(el => { el.scrollTop = 0 })
  await expect(page.locator(".mini a.on")).toHaveText("Draft")

  // A Scrollable's ground is on the element that does not scroll; what is in
  // it has none, so the ground is what shows, bounce or not.
  const wide = page.locator("#scrollable .scroll-pre.wide")
  expect(await wide.evaluate(el => getComputedStyle(el).backgroundColor)).toBe("rgba(0, 0, 0, 0)")
  expect(await wide.evaluate(el => getComputedStyle(el.closest(".ground")).backgroundColor)).toBe("rgb(17, 17, 17)")

  expect(errors, "the page logged errors").toEqual([])
})

test("a modal's head follows its size, and a tab modal's head is its tabs", async ({ page }) => {
  const errors = await demo(page)
  const modals = page.locator("#modal")

  await modals.getByRole("button", { name: "A form…" }).click()
  const small = page.getByRole("dialog", { name: "New session" })
  await expect(small.locator("header")).not.toHaveAttribute("data-tone", "dark")
  await page.keyboard.press("Escape")
  await expect(small).toBeHidden()
  await modals.getByRole("button", { name: "A large one…" }).click()
  const large = page.getByRole("dialog", { name: "Context of turn 12" })
  await expect(large.locator("header")).toHaveAttribute("data-tone", "dark")
  await page.keyboard.press("Escape")
  await expect(large).toBeHidden()

  await page.locator("#tab-modal").getByRole("button", { name: "workstation…" }).click()
  const tabbed = page.getByRole("dialog", { name: "workstation" })
  await expect(tabbed.locator("header")).toHaveAttribute("data-tone", "dark")
  await expect(tabbed.getByRole("tab", { name: "Connection" })).toBeFocused()
  await expect(tabbed.getByRole("tabpanel")).toContainText("URL")
  // Measured once it has arrived: it opens from a scale a little under one.
  const settled = () => tabbed.evaluate(d => Promise.all(d.getAnimations().map(a => a.finished)))
  await settled()
  const box = await tabbed.boundingBox()
  await tabbed.getByRole("tab", { name: /Models/ }).click()
  await expect(tabbed.getByRole("tabpanel")).toContainText("offered")
  // The height is fixed: changing tabs does not move the dialog.
  await settled()
  expect((await tabbed.boundingBox())?.height).toBeCloseTo(box?.height ?? 0, 0)
  await page.keyboard.press("ArrowRight")
  await expect(tabbed.getByRole("tab", { selected: true })).toHaveText("Limits")
  // The disabled tab is passed over on the way round.
  await page.keyboard.press("ArrowRight")
  await expect(tabbed.getByRole("tab", { selected: true })).toHaveText("Connection")
  await tabbed.getByRole("button", { name: "Save" }).click()
  await expect(tabbed).toBeHidden()
  await expect(page.locator("#tab-modal .state")).toContainText("saved: 1")

  expect(errors, "the page logged errors").toEqual([])
})

/**
 * A head in a tone, with and without tabs: the bar in the tone's strong
 * colour and the solid badge's ink on it, whatever the size — and on a tab
 * modal, the chosen tab and its line in that ink, since the accent's line
 * would vanish on accent. Read against what the tokens compute to, so the
 * test follows the theme rather than a copy of its values.
 */
test("a modal's head can be any tone, with tabs or without", async ({ page }) => {
  const errors = await demo(page)
  // What a token computes to, read off an element painted with it.
  const computed = token => page.evaluate(name => {
    const probe = document.createElement("div")
    probe.style.background = `var(${name})`
    document.body.append(probe)
    const colour = getComputedStyle(probe).backgroundColor
    probe.remove()
    return colour
  }, token)
  const paint = locator => locator.evaluate(e => ({
    bg: getComputedStyle(e).backgroundColor,
    fg: getComputedStyle(e).color,
  }))
  const ink = await computed("--luu-on-accent")

  const kind = page.locator("#modal")
  const TITLES = { accent: "Connect a provider", ok: "Connected", warn: "Runs on the host", bad: "Remove workstation?" }
  for (const [tone, title] of Object.entries(TITLES)) {
    await kind.getByRole("radio", { name: tone, exact: true }).click()
    await kind.getByRole("button", { name: "With this head…" }).click()
    const dialog = page.getByRole("dialog", { name: title })
    const head = dialog.locator("header")
    await expect(head).toHaveClass(new RegExp(`\\btone-${tone}\\b`))
    // A tone is not the dark island: the page's own values, re-pointed none.
    await expect(head).not.toHaveAttribute("data-tone", "dark")
    expect(await paint(head)).toEqual({ bg: await computed(`--luu-${tone}`), fg: ink })
    await page.keyboard.press("Escape")
    await expect(dialog).toBeHidden()
  }
  // The other three are still what they were: at 30rem, `auto` is part of the
  // body, and `dark` says it outright whatever the size.
  await kind.getByRole("radio", { name: "dark", exact: true }).click()
  await kind.getByRole("button", { name: "With this head…" }).click()
  await expect(page.getByRole("dialog", { name: "Context of turn 12" }).locator("header"))
    .toHaveAttribute("data-tone", "dark")
  await page.keyboard.press("Escape")
  await kind.getByRole("radio", { name: "auto", exact: true }).click()
  await kind.getByRole("button", { name: "With this head…" }).click()
  const auto = page.getByRole("dialog", { name: "Head: auto" }).locator("header")
  await expect(auto).not.toHaveAttribute("data-tone", "dark")
  await expect(auto).not.toHaveClass(/\btoned\b/)
  await page.keyboard.press("Escape")

  // With tabs: the head in the tone, the chosen tab and its line in the ink.
  const tabs = page.locator("#tab-modal")
  await tabs.getByRole("radio", { name: "warn", exact: true }).click()
  await tabs.getByRole("button", { name: "workstation…" }).click()
  const tabbed = page.getByRole("dialog", { name: "workstation" })
  const head = tabbed.locator("header")
  await expect(head).toHaveClass(/\btone-warn\b/)
  expect((await paint(head)).bg).toBe(await computed("--luu-warn"))
  const chosen = tabbed.getByRole("tab", { selected: true })
  await expect(chosen).toBeFocused()
  expect((await paint(chosen)).fg).toBe(ink)
  expect((await paint(tabbed.locator(".line"))).bg).toBe(ink)
  // A tab not chosen is a step back from the ink, not the theme's grey.
  const other = await paint(tabbed.getByRole("tab", { name: "Limits" }))
  expect(other.fg).not.toBe(ink)
  expect(other.fg).not.toBe(await computed("--luu-fg-muted"))
  await page.keyboard.press("ArrowRight")
  await expect(tabbed.getByRole("tab", { selected: true })).toHaveText(/Models/)
  await page.keyboard.press("Escape")
  await expect(tabbed).toBeHidden()
  await expect(page.locator("#tab-modal .state")).toContainText("head: warn")

  expect(errors, "the page logged errors").toEqual([])
})

test("a sub-modal comes out of its parent's head, and ESC closes it alone", async ({ page }) => {
  const errors = await demo(page)
  await page.locator("#sub-modal").getByRole("button", { name: "Edit a provider…" }).click()
  const parent = page.getByRole("dialog", { name: "workstation" })
  await expect(parent).toBeVisible()

  // Without tabs: the focus goes to its first field, the parent's body is
  // inert under it, and its head is not.
  await parent.getByRole("button", { name: "Add a model…" }).click()
  const sub = page.getByRole("dialog", { name: "Add a model" })
  await expect(sub).toBeVisible()
  const field = sub.getByRole("textbox", { name: "Model" })
  await expect(field).toBeFocused()
  expect(await parent.getByRole("textbox", { name: "URL" }).evaluate(i => !!i.closest("[inert]"))).toBe(true)
  expect(await parent.locator("header").first().evaluate(h => !!h.closest("[inert]"))).toBe(false)
  // It sits under the parent's head, not over it — measured once both have
  // arrived: the parent swings in, and the sheet slides down out of its head.
  const arrived = dialog => dialog.evaluate(d => Promise.all(d.getAnimations({ subtree: true }).map(a => a.finished)))
  await arrived(parent)
  await arrived(sub)
  const head = await parent.locator("header").first().boundingBox()
  expect((await sub.boundingBox())?.y).toBeGreaterThanOrEqual((head?.y ?? 0) + (head?.height ?? 0) - 1)
  // Narrower than its parent, and centred on it.
  const outer = await parent.boundingBox()
  const sheet = await sub.boundingBox()
  expect(sheet?.width ?? 0).toBeLessThan(outer?.width ?? 0)
  const left = (sheet?.x ?? 0) - (outer?.x ?? 0)
  const right = (outer?.x ?? 0) + (outer?.width ?? 0) - (sheet?.x ?? 0) - (sheet?.width ?? 0)
  expect(Math.abs(left - right)).toBeLessThan(2)
  // The veil over the parent's body rounds where the parent does, below.
  const corners = await page.evaluate(() => {
    const veil = getComputedStyle(document.querySelector(".veil.shown"))
    const dialog = getComputedStyle(document.querySelector("dialog.modal[open]"))
    return [veil.borderBottomLeftRadius, veil.borderBottomRightRadius, veil.borderTopLeftRadius, dialog.borderBottomLeftRadius]
  })
  expect(corners[0]).toBe(corners[3])
  expect(corners[1]).toBe(corners[3])
  expect(corners[2]).toBe("0px")
  await field.fill("llama3.2:3b")
  await sub.getByRole("button", { name: "Add" }).click()
  await expect(sub).toBeHidden()
  await expect(page.locator("#sub-modal .state")).toContainText('["llama3.2:3b"]')
  await expect(parent.locator("[inert]")).toHaveCount(0)

  // With tabs: chosen by click and by the arrows; ESC closes it and not the parent.
  await parent.getByRole("button", { name: "Test the connection…" }).click()
  const test = page.getByRole("dialog", { name: "Test" })
  await test.getByRole("tab", { name: "Response" }).click()
  await expect(test.locator("pre")).toContainText("200 OK")
  await page.keyboard.press("ArrowLeft")
  await expect(test.getByRole("tab", { selected: true })).toHaveText("Request")
  await page.keyboard.press("Escape")
  await expect(test).toBeHidden()
  await expect(parent).toBeVisible()

  // The parent's own close, with a sheet open, closes both, and leaves nothing inert.
  await parent.getByRole("button", { name: "Add a model…" }).click()
  await expect(sub).toBeVisible()
  await parent.locator("header").first().getByRole("button", { name: "Close" }).click()
  await expect(parent).toBeHidden()
  await page.locator("#sub-modal").getByRole("button", { name: "Edit a provider…" }).click()
  await expect(parent).toBeVisible()
  await expect(sub).toBeHidden()
  await expect(parent.locator("[inert]")).toHaveCount(0)

  expect(errors, "the page logged errors").toEqual([])
})

/**
 * The heads' tones as buttons: the ground each tone's token computes to, the
 * solid badge's ink on it, and `dark` the island the dark head is.
 */
test("a button takes the tones a modal's head takes", async ({ page }) => {
  const errors = await demo(page)
  const computed = token => page.evaluate(name => {
    const probe = document.createElement("div")
    probe.style.background = `var(${name})`
    document.body.append(probe)
    const colour = getComputedStyle(probe).backgroundColor
    probe.remove()
    return colour
  }, token)
  const ink = await computed("--luu-on-accent")
  const tones = page.locator("#button .tones")
  for (const tone of ["accent", "ok", "warn", "bad"]) {
    const button = tones.getByRole("button", { name: tone, exact: true })
    const painted = await button.evaluate(b => [getComputedStyle(b).backgroundColor, getComputedStyle(b).color])
    expect(painted).toEqual([await computed(`--luu-${tone}`), ink])
    await expect(button).not.toHaveAttribute("data-tone", "dark")
  }
  await expect(tones.getByRole("button", { name: "dark", exact: true })).toHaveAttribute("data-tone", "dark")
  // A tone's hover is a step from its ground, not the theme's grey.
  const ok = tones.getByRole("button", { name: "ok", exact: true })
  await ok.hover()
  await expect.poll(() => ok.evaluate(b => getComputedStyle(b).backgroundColor)).not.toBe(await computed("--luu-ok"))
  expect(await ok.evaluate(b => getComputedStyle(b).backgroundColor)).not.toBe(await computed("--luu-hover"))
  await expect(tones.getByRole("button", { name: "ok, disabled" })).toBeDisabled()
  expect(errors, "the page logged errors").toEqual([])
})

/**
 * One `size` for every control: at sm, md and lg a Button, a TextInput, a
 * Select, a Choice segmented and a Typeahead are each as tall as the token
 * says, and on one row their tops line up.
 */
test("every control is as tall as its size's token", async ({ page }) => {
  const errors = await demo(page)
  await expect(page.locator("#segmented-title")).toContainText("Choice segmented")
  for (const size of ["sm", "md", "lg"]) {
    const row = page.locator(`#sizes .size-row[data-size="${size}"]`)
    const boxes = await row.evaluate(r => [...r.querySelectorAll(".cell")].map(cell => {
      const box = cell.querySelector(".btn, .field, .select, .seg").getBoundingClientRect()
      return { h: Math.round(box.height), top: Math.round(box.top) }
    }))
    const token = await page.evaluate(z => parseFloat(getComputedStyle(document.documentElement)
      .getPropertyValue(`--luu-control-${z}`)), size)
    expect(boxes).toHaveLength(6)
    expect(new Set(boxes.map(b => b.h)), `heights at ${size}`).toEqual(new Set([token]))
    expect(new Set(boxes.map(b => b.top)).size, `tops at ${size}`).toBe(1)
  }
  // A number is typed: no arrows beside it.
  await expect.poll(() => page.locator('#text-input input[type="number"]').first()
    .evaluate(i => getComputedStyle(i).appearance)).toBe("textfield")
  expect(errors, "the page logged errors").toEqual([])
})

test("buttons, badges and alerts take their attributes, bare or bound", async ({ page }) => {
  const errors = await demo(page)
  const buttons = page.locator("#button")

  await buttons.getByRole("button", { name: "Primary" }).click()
  await buttons.getByRole("button", { name: "Danger" }).click()
  await expect(buttons.locator(".state")).toContainText("clicks: 2")
  // A bare `disabled` is disabled, and a click on it is not one.
  const off = buttons.getByRole("button", { name: "Disabled" }).first()
  await expect(off).toBeDisabled()
  await off.click({ force: true })
  await expect(buttons.locator(".state")).toContainText("clicks: 2")
  // Loading keeps its width and refuses a second click.
  const save = buttons.getByRole("button", { name: "Save" })
  const width = (await save.boundingBox())?.width
  await save.click()
  await expect(save).toHaveAttribute("aria-busy", "true")
  expect((await save.boundingBox())?.width).toBe(width)
  await expect(save).not.toHaveAttribute("aria-busy", "true", { timeout: 3_000 })
  // An icon alone is named by its label, and the icon has a size.
  const square = buttons.getByRole("button", { name: "Settings" })
  expect((await square.locator("svg").boundingBox())?.width).toBeGreaterThan(10)

  await expect(page.locator("#badge .badge.solid")).toHaveCount(5)
  await expect(page.locator("#badge .dot")).toHaveCount(3)

  const alerts = page.locator("#alert")
  await expect(alerts.getByRole("alert")).toHaveCount(1)
  await alerts.getByRole("button", { name: "Dismiss" }).first().click()
  await expect(alerts.getByText("Sessions name themselves")).toHaveCount(0)
  await alerts.getByRole("button", { name: "Bring them back" }).click()
  await expect(alerts.getByText("Sessions name themselves")).toBeVisible()

  expect(errors, "the page logged errors").toEqual([])
})

test("form controls write back what a person does to them", async ({ page }) => {
  const errors = await demo(page)

  const text = page.locator("#text-input")
  const url = text.getByRole("textbox", { name: "URL" })
  await url.fill("http://127.0.0.1:8080/v1")
  await url.press("Enter")
  await expect(text.locator(".state")).toContainText('url: "http://127.0.0.1:8080/v1"')
  await expect(text.locator(".state")).toContainText('last Enter: "http://127.0.0.1:8080/v1"')
  await text.getByRole("button", { name: "Clear" }).click()
  await expect(text.locator(".state")).toContainText('url: ""')
  await expect(text.getByRole("textbox", { name: "Invalid" })).toHaveAttribute("aria-invalid", "true")
  await expect(text.getByRole("textbox", { name: "Disabled" })).toBeDisabled()

  const select = page.locator("#select")
  await select.getByRole("combobox", { name: "Backend" }).selectOption("mock")
  await expect(select.locator(".state")).toContainText("backend: mock")
  await select.getByRole("combobox", { name: "Model" }).selectOption("llama3.1:8b")
  await expect(select.locator(".state")).toContainText("model: llama3.1:8b")

  // Some of the files is a dash; ticking it takes all of them, and untick none.
  const checks = page.locator("#checkbox")
  const all = checks.locator("label.check", { hasText: "All files" })
  await expect(all).toHaveClass(/mixed/)
  await all.click()
  await expect(checks.locator(".state")).toContainText('{"a":true,"b":true,"c":true}')
  await all.click()
  await expect(checks.locator(".state")).toContainText('{"a":false,"b":false,"c":false}')
  await checks.locator("label.check", { hasText: "src/store.rs" }).click()
  await expect(all).toHaveClass(/mixed/)
  await expect(checks.getByRole("checkbox", { name: "Disabled and checked" })).toBeChecked()

  const radios = page.locator("#radio-group")
  await radios.getByRole("radio", { name: /always.*token/ }).check()
  await expect(radios.locator(".state")).toContainText("host: always")
  await expect(radios.getByRole("radio", { name: "never" }).last()).toBeDisabled()

  const switches = page.locator("#switch")
  await switches.getByRole("switch", { name: "Sound when a turn ends" }).click()
  await expect(switches.locator(".state")).toContainText("sounds: true")
  await expect(switches.getByRole("switch", { name: "Disabled and on" })).toHaveAttribute("aria-checked", "true")

  // One radio group to the keyboard: the arrows move the choice.
  const segmented = page.locator("#segmented")
  await segmented.getByRole("radio", { name: "Three", exact: true }).focus()
  await page.keyboard.press("ArrowLeft")
  await expect(segmented.locator(".state")).toContainText("layout: two")
  await expect(segmented.getByRole("radio", { name: "Two", exact: true })).toBeFocused()

  // A wrong URL says so in place of the hint, and the field says it is invalid.
  const field = page.locator("#field")
  await field.getByRole("textbox").nth(1).fill("192.168.1.20")
  await expect(field.getByRole("alert")).toHaveText("a URL starts with http:// or https://")
  await field.getByText("URL", { exact: true }).click()
  await expect(field.getByRole("textbox").nth(1)).toBeFocused()

  expect(errors, "the page logged errors").toEqual([])
})

test("tabs, menus, tooltips and modals open, move and close", async ({ page }) => {
  const errors = await demo(page)

  const tabs = page.locator("#tabs")
  await tabs.getByRole("tab", { name: /Answer/ }).click()
  await expect(tabs.locator(".panel")).toHaveText("The answer as the model wrote it.")
  await page.keyboard.press("ArrowRight")
  await expect(tabs.locator(".state")).toContainText("tab: calls")
  // A disabled tab is skipped on the way round.
  await page.keyboard.press("ArrowRight")
  await expect(tabs.locator(".state")).toContainText("tab: context")

  // In the top layer, keyboard inside, and gone on a choice or on ESC.
  const menus = page.locator("#menu")
  await menus.getByRole("button", { name: "Session" }).click()
  const list = page.getByRole("menu", { name: "Session" })
  await expect(list).toBeVisible()
  await expect(list.getByRole("menuitem", { name: /Rename/ })).toBeFocused()
  await page.keyboard.press("ArrowDown")
  await page.keyboard.press("Enter")
  await expect(list).toBeHidden()
  await expect(menus.locator(".state")).toContainText("selected: copy")
  await menus.getByRole("button", { name: "Session" }).click()
  await expect(list).toBeVisible()
  await page.keyboard.press("Escape")
  await expect(list).toBeHidden()

  const tips = page.locator("#tooltip")
  await tips.getByRole("button", { name: "bottom" }).focus()
  const tip = page.getByRole("tooltip", { name: "Below" })
  await expect(tip).toBeVisible()
  const under = await tips.getByRole("button", { name: "bottom" }).boundingBox()
  expect((await tip.boundingBox())?.y).toBeGreaterThan((under?.y ?? 0) + (under?.height ?? 0) - 1)
  await page.keyboard.press("Escape")
  await expect(tip).toBeHidden()

  // ESC asks; the owner closes. The answer is what moves the count.
  const modals = page.locator("#modal")
  await modals.getByRole("button", { name: "Delete a session…" }).click()
  const ask = page.getByRole("dialog", { name: "Delete this session?" })
  await expect(ask).toBeVisible()
  await page.keyboard.press("Escape")
  await expect(ask).toBeHidden()
  await modals.getByRole("button", { name: "Delete a session…" }).click()
  await ask.getByRole("button", { name: "Delete" }).click()
  await expect(ask).toBeHidden()
  await expect(modals.locator(".state")).toContainText("deleted: 1")

  // One with no way back stays through ESC.
  await modals.getByRole("button", { name: "No way back…" }).click()
  const locked = page.getByRole("dialog", { name: "Choose a folder" })
  await expect(locked).toBeVisible()
  await expect(locked.getByRole("button", { name: "Close" })).toHaveCount(0)
  await page.keyboard.press("Escape")
  await expect(locked).toBeVisible()
  await locked.getByRole("button", { name: "This one" }).click()
  await expect(locked).toBeHidden()

  expect(errors, "the page logged errors").toEqual([])
})

/**
 * How a section is used: its *Integrate*, beside its title, opens that
 * section's own — a `Modal` for one component, a `TabModal` with a tab per
 * component for several — each snippet a parent component that uses it,
 * coloured by the server's highlighter, and copied as written. The head's
 * *Integrate* is what a page loads first.
 */
test("each section opens how it is used, a tab per component, coloured", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"])
  const errors = await demo(page)

  // The page's own: the files it loads.
  await page.locator(".top").getByRole("button", { name: "Integrate" }).click()
  let dialog = page.getByRole("dialog", { name: "Load the theme" })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByRole("tab")).toHaveCount(0)
  await expect(dialog.locator(".snippet-code").first()).toContainText('href="/theme/default/tokens.css"')
  await dialog.getByRole("button", { name: "Copy" }).first().click()
  await expect(dialog.getByRole("button", { name: "Copied" })).toHaveCount(1)
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain("</script>")
  await dialog.getByRole("button", { name: "Done" }).click()
  await expect(dialog).toBeHidden()

  // Every section has one, and one component is a plain modal: the Button,
  // used in a parent that imports it, written as a tag, coloured.
  await expect(page.locator(".content section.section > .head").getByRole("button", { name: "Integrate" })).toHaveCount(27)
  await page.locator("#button").getByRole("button", { name: "Integrate" }).first().click()
  dialog = page.getByRole("dialog", { name: "Button" })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByRole("tab")).toHaveCount(0)
  const code = dialog.locator(".snippet-code").first()
  await expect(code).toContainText('await import("@theme/components/button.html")')
  await expect(code).toContainText("<Button variant=\"ok\"")
  await expect(code.locator(".hl-tag").first()).toBeVisible()
  await expect(code.locator(".hl-keyword").first()).toBeVisible()
  // A colour of its own, not the text's.
  const [tag, ink] = await code.evaluate(el => [
    getComputedStyle(el.querySelector(".hl-tag")).color,
    getComputedStyle(el).color,
  ])
  expect(tag).not.toBe(ink)
  await page.keyboard.press("Escape")
  await expect(dialog).toBeHidden()

  // Several components: a tab each, each with its own parent.
  await page.locator("#modals").getByRole("button", { name: "Integrate" }).first().click()
  dialog = page.getByRole("dialog", { name: "Modals" })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByRole("tab")).toHaveText(["Modal", "TabModal", "SubModal"])
  await expect(dialog.locator(".snippet-code").first()).toContainText("<Modal :open=")
  await dialog.getByRole("tab", { name: "SubModal" }).click()
  await expect(dialog.locator(".snippet-head code")).toHaveText("workstation.html")
  await expect(dialog.locator(".snippet-code").first()).toContainText("<SubModal :open=")
  await expect(dialog.locator(".snippet-code .hl-tag").first()).toBeVisible()
  await dialog.getByRole("button", { name: "Done" }).click()
  await expect(dialog).toBeHidden()
  expect(errors, "the page logged errors").toEqual([])
})
