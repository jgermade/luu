// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * An HTML block in a reply opens as the page it is, in a frame that can reach
 * neither this page nor `luu serve`.
 *
 * Its own server, because the reply is the one thing the test needs to choose:
 * the gate spec's mock replies are a plan, a call and an answer, in order.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7900
const BASE = `http://127.0.0.1:${PORT}`

// The page sets a mark when its script runs, and others when a request to the
// server that drew it fails. The image is the one that tests the policy: a
// `fetch` from the frame's own origin would fail on CORS alone, and an image
// is not subject to CORS — only `img-src` without `http:` stops it.
const PAGE = [
  "<!doctype html>",
  "<html><head><title>t</title></head><body>",
  '<h1 id="hello">Hola</h1>',
  `<img src="${BASE}/logo.svg" onload="document.body.dataset.image = 'yes'" onerror="document.body.dataset.image = 'no'">`,
  "<script>",
  "document.body.dataset.ran = 'yes'",
  `fetch('${BASE}/api/terminal').then(() => { document.body.dataset.reached = 'yes' },`,
  "  () => { document.body.dataset.reached = 'no' })",
  "</script>",
  "</body></html>",
].join("\n")
// Prose before the block, in the Markdown a model writes, and a tag in it that
// must be shown rather than made.
const PROSE = [
  "Soy **luu**. Lo que carga:",
  "",
  "- one",
  "- two",
  "",
  "El fichero `tokens.css`, en [la demo](https://example.com/). <b>raw</b>",
  "",
  "| a | b |",
  "|---|--:|",
  "| 1 | 2 |",
].join("\n")
const REPLY = `${PROSE}\n\nHere it is:\n\n\`\`\`html\n${PAGE}\n\`\`\`\n`

function binary() {
  const named = process.env.LUU_BIN
  const candidates = named
    ? [resolve(root, named)]
    : [join(root, "target/release/luu"), join(root, "target/debug/luu")]
  const found = candidates.find(path => existsSync(path))
  if (!found) throw new Error(`no luu binary at ${candidates.join(" or ")}`)
  return found
}

/** @type {import("node:child_process").ChildProcess | null} */
let server = null
let scratch = ""

test.beforeAll(async () => {
  scratch = mkdtempSync(join(tmpdir(), "luu-snippet-"))
  mkdirSync(join(scratch, "home"), { recursive: true })
  writeFileSync(join(scratch, "home/config.toml"), '[provider.here]\nbackend = "mock"\nmodel = "mock"\n')
  server = spawn(
    binary(),
    ["serve", "--bind", `127.0.0.1:${PORT}`, "--no-store", "--mock-delay-ms", "0", "--mock-reply", REPLY],
    { cwd: root, env: { ...process.env, LUU_HOME: join(scratch, "home") }, stdio: "pipe" },
  )
  let output = ""
  server.stdout?.on("data", chunk => (output += chunk))
  server.stderr?.on("data", chunk => (output += chunk))
  const deadline = Date.now() + 30_000
  for (;;) {
    if (server.exitCode !== null) throw new Error(`luu serve exited with ${server.exitCode}:\n${output}`)
    try {
      if ((await fetch(`${BASE}/index.html`)).ok) return
    } catch {
      // Not listening yet.
    }
    if (Date.now() > deadline) throw new Error(`luu serve never answered:\n${output}`)
    await new Promise(again => setTimeout(again, 250))
  }
})

test.afterAll(() => {
  server?.kill("SIGTERM")
  if (scratch) rmSync(scratch, { recursive: true, force: true })
})

test("an HTML block opens as a page in a frame that cannot reach the server", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  const picker = page.locator("dialog.modal").first()
  await picker.waitFor({ state: "visible", timeout: 5_000 }).catch(() => {})
  if (await picker.isVisible()) {
    await picker.locator('button:has-text("Use this folder")').click()
    // It closes once the session has moved there, two requests later; until
    // then it is modal, the composer behind it is inert, and a fill there
    // types into nothing.
    await expect(picker).toBeHidden()
  }

  const composer = page.locator(".composer input")
  await expect(composer).toBeEnabled()
  await composer.fill("a page, please")
  await page.click('.composer button[type="submit"]')

  const block = page.locator(".snippet").first()
  await expect(block.locator(".lang")).toHaveText("html", { timeout: 30_000 })
  // Left of copy, and only on HTML.
  const buttons = block.locator("figcaption button")
  await expect(buttons).toHaveCount(2)
  await expect(buttons.nth(0)).toHaveAccessibleName("Open as a page")
  await expect(buttons.nth(1)).toHaveAccessibleName("Copy the code")

  await buttons.nth(0).click()
  const modal = page.getByRole("dialog", { name: "Preview" })
  await expect(modal).toBeVisible()
  const frame = page.frameLocator("dialog.modal iframe.page")
  await expect(frame.locator("#hello")).toHaveText("Hola")
  await expect(frame.locator("body")).toHaveAttribute("data-ran", "yes")
  // The server that drew it is out of its reach.
  await expect(frame.locator("body")).toHaveAttribute("data-reached", "no")
  await expect(frame.locator("body")).toHaveAttribute("data-image", "no")

  await page.keyboard.press("Escape")
  await expect(modal).toHaveCount(0)
})

/**
 * A reply's prose is drawn from its Markdown — escaped first, so a tag the
 * model wrote is shown as the text it is — a step smaller than the prompt,
 * which a bar in the page's black marks as one.
 */
test("a reply's Markdown is drawn, escaped, under a prompt marked with a bar", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  const picker = page.locator("dialog.modal").first()
  await picker.waitFor({ state: "visible", timeout: 5_000 }).catch(() => {})
  if (await picker.isVisible()) {
    await picker.locator('button:has-text("Use this folder")').click()
    await expect(picker).toBeHidden()
  }

  const composer = page.locator(".composer input")
  await expect(composer).toBeEnabled()
  await composer.fill("what are you?")
  await page.click('.composer button[type="submit"]')

  const prose = page.locator("article.assistant .reply-md").last()
  await expect(prose.locator("strong")).toHaveText("luu", { timeout: 30_000 })
  await expect(prose.locator("li")).toHaveText(["one", "two"])
  await expect(prose.locator("code")).toHaveText("tokens.css")
  const link = prose.locator("a")
  await expect(link).toHaveText("la demo")
  await expect(link).toHaveAttribute("href", "https://example.com/")
  await expect(link).toHaveAttribute("rel", "noopener noreferrer")
  await expect(prose).toContainText("<b>raw</b>")
  await expect(prose.locator("b")).toHaveCount(0)
  await expect(prose).not.toContainText("**")
  // A table, out of what jq79's sanitiser keeps, laid out as one.
  const cells = prose.locator(".md-table .md-cell")
  await expect(cells).toHaveText(["a", "b", "1", "2"])
  expect(await cells.nth(3).evaluate(el => [getComputedStyle(el).display, getComputedStyle(el).textAlign]))
    .toEqual(["table-cell", "right"])
  // A link opens beside the session, never in its place.
  await page.route("https://example.com/**", route => route.fulfill({ body: "elsewhere" }))
  const [popup] = await Promise.all([page.waitForEvent("popup"), link.click()])
  await popup.close()
  await expect(page).toHaveURL(`${BASE}/index.html`)

  const asked = page.locator("article.user .text").last()
  await expect(asked).toHaveText("what are you?")
  const bar = await asked.evaluate(el => {
    const before = getComputedStyle(el, "::before")
    return { content: before.content, color: before.color }
  })
  expect(bar).toEqual({ content: '"| "', color: "rgb(51, 58, 62)" })
  const size = async (/** @type {import("@playwright/test").Locator} */ at) =>
    at.evaluate(el => parseFloat(getComputedStyle(el).fontSize))
  const [askedSize, answerSize] = [await size(asked), await size(prose)]
  expect(answerSize).toBeLessThan(askedSize)
  expect(answerSize).toBeGreaterThan(askedSize * 0.85)
})
