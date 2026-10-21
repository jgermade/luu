// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * The earlier sessions, under the clock in the chat's head: picking one loads
 * it, and the list is in the order sessions were last **written** to — opening
 * one does not move it to the top, a turn in it does. Against a live server
 * with a session store, which is the one thing `reading.spec.js` turns off.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7903
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
  home = mkdtempSync(join(tmpdir(), "luu-history-"))
  writeFileSync(join(home, "config.toml"), 'default = "here"\n\n[provider.here]\nbackend = "mock"\nmodel = "mock"\n')
  server = spawn(binary(), ["serve", "--bind", `127.0.0.1:${PORT}`, "--mock-delay-ms", "0"], {
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
      if ((await fetch(`${BASE}/api/settings`)).ok) return
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

/** The folder question a fresh browser gets first — see `gate.spec.js`. */
async function chooseFolder(page) {
  const picker = page.locator("dialog.modal").first()
  await picker.waitFor({ state: "visible", timeout: 5_000 }).catch(() => {})
  if (await picker.isVisible()) {
    await picker.locator('button:has-text("Use this folder")').click()
    await expect(picker).toBeHidden()
  }
}

test("an earlier session loads when picked, and only writing in it moves it up", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)

  // Sent, and *finished*: a session with a turn still running refuses to be
  // left (409), so the next step must not start before this one ends.
  const say = async text => {
    const done = page.locator(".transcript article.assistant button.meta")
    const before = await done.count()
    await page.locator(".composer input").fill(text)
    await page.click('.composer button[type="submit"]')
    await expect(done).toHaveCount(before + 1)
  }
  // The rows as the popover shows them, the current one starred, each by how
  // many turns it holds — which is what tells the two sessions apart here.
  // Polled: the popover asks for the list again as it opens, and draws the one
  // it had until the answer lands.
  const dialog = page.getByRole("dialog", { name: "Sessions" })
  const rowsAre = async expected => {
    await page.click('button[title="Earlier sessions"]')
    await expect.poll(() => dialog.locator("ul.sessions li").evaluateAll(all =>
      all.map(row => (row.classList.contains("on") ? "*" : "") +
        row.querySelector(".facts").textContent.match(/(\d+) turns?/)[1] + "t")))
      .toEqual(expected)
    await page.keyboard.press("Escape")
    await expect(dialog).toHaveCount(0)
  }
  const pickOther = async () => {
    await page.click('button[title="Earlier sessions"]')
    await dialog.locator("ul.sessions li:not(.on) .pick").first().click()
    await expect(dialog).toHaveCount(0)
  }

  // A with one turn; then B, with one, which is the one written to last.
  await say("uno")
  await page.click('button[title^="New session"]')
  await page.getByRole("dialog", { name: "Where a session sends" }).locator('button:has-text("Start session")').click()
  await expect(page.locator(".transcript article.user")).toHaveCount(0)
  await say("dos")
  await say("dos más")
  await rowsAre(["*2t", "1t"])

  // A, picked: its turn is back, and it stays where it was.
  await pickOther()
  await expect(page.locator(".transcript article.user .text")).toHaveText(["uno"])
  await rowsAre(["2t", "*1t"])

  // Written in: now it is the last one written to.
  await say("tres")
  await rowsAre(["*2t", "2t"])

  // Started and left empty: listed while it is the one on screen, and not
  // once another is — it holds nothing to open.
  await page.click('button[title^="New session"]')
  await page.getByRole("dialog", { name: "Where a session sends" }).locator('button:has-text("Start session")').click()
  await expect(page.locator(".transcript article.user")).toHaveCount(0)
  await rowsAre(["*0t", "2t", "2t"])
  await pickOther()
  await expect(page.locator(".transcript article.user .text")).toHaveText(["uno", "tres"])
  await rowsAre(["*2t", "2t"])

  // Searched: a piece of one session's id finds that one, and nothing found
  // says so.
  await page.click('button[title="Earlier sessions"]')
  const other = await dialog.locator("ul.sessions li:not(.on) .pick").first().getAttribute("title")
  const search = dialog.locator('input[placeholder="Search by name"]')
  await expect(search).toBeFocused()
  await search.fill(other.slice(-8))
  await expect(dialog.locator("ul.sessions li")).toHaveCount(1)
  await expect(dialog.locator("ul.sessions li .pick")).toHaveAttribute("title", other)
  await search.fill("nada-que-se-llame-así")
  await expect(dialog.locator("ul.sessions li")).toHaveCount(0)
  await expect(dialog).toContainText("No session matches")
  await page.keyboard.press("Escape")

  expect(errors).toEqual([])
})
