// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * Between a call that finished and the model's next word, the chat says the
 * model is reading what came back. That wait is the prefill of the result —
 * a whole file, on a local 7B — and it was the one stretch of a turn with
 * nothing on screen moving.
 *
 * The mock streams word by word with a delay before each, and the call below
 * has no spaces, so it is one word: two seconds to write it, then two seconds
 * of reading before `Done.` arrives.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7902
const BASE = `http://127.0.0.1:${PORT}`

const READ = '```tool\n{"name":"read_file","arguments":{"path":"AGENTS.md"}}\n```'
// A call tried out while thinking and dropped, then the one made. Three words,
// so six seconds, the first four of them with the thinking still open.
const DRAFT = '```tool\n{"name":"read_file","arguments":{"path":"NOT-THIS.md"}}\n```'
const THINKING = `<think>Maybe ${DRAFT} no.</think>\n${READ}`

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
  home = mkdtempSync(join(tmpdir(), "luu-reading-"))
  writeFileSync(join(home, "config.toml"), '[provider.here]\nbackend = "mock"\nmodel = "mock"\n')
  server = spawn(
    binary(),
    [
      "serve",
      "--bind", `127.0.0.1:${PORT}`,
      "--no-store",
      "--mock-delay-ms", "2000",
      // One call and its answer per test, in the order the tests run.
      "--mock-reply", READ,
      "--mock-reply", "Done.",
      "--mock-reply", THINKING,
      "--mock-reply", "Done.",
    ],
    { cwd: root, env: { ...process.env, LUU_HOME: home }, stdio: "pipe" },
  )
  let output = ""
  server.stdout?.on("data", chunk => (output += chunk))
  server.stderr?.on("data", chunk => (output += chunk))

  const deadline = Date.now() + 30_000
  for (;;) {
    if (server.exitCode !== null) {
      throw new Error(`luu serve exited with ${server.exitCode}:\n${output}`)
    }
    try {
      const answer = await fetch(`${BASE}/api/settings`)
      if (answer.ok) return
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

test("the chat says the model is reading what a call returned", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)

  const composer = page.locator(".composer input")
  await expect(composer).toBeEnabled()
  await composer.fill("lee AGENTS.md")
  await page.click('.composer button[type="submit"]')

  const answer = page.locator(".transcript article.assistant").last()
  // Before anything: the wait that was already drawn.
  await expect(answer.locator(".waiting")).toContainText("waiting for the model")
  // The call ran, and the model has not written again: the new row.
  await expect(answer.locator(".call .status")).toHaveText("done", { timeout: 10_000 })
  await expect(answer.locator(".waiting")).toHaveText("the model is reading what read_file returned…")
  // And it goes when the model does.
  await expect(answer.locator(".parts .text").last()).toHaveText("Done.", { timeout: 10_000 })
  await expect(answer.locator(".waiting")).toHaveCount(0)

  expect(errors).toEqual([])
})

/**
 * What the model writes while thinking is drawn folded, says *thinking…* while
 * it streams, and is never read for a call: the read it tried out and dropped
 * there is not run, and the one after it is. See
 * `RECORD/2026-10-21.a-call-written-while-thinking.completed.md`.
 */
test("a call written while thinking is not run, and the thinking is folded", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)

  const composer = page.locator(".composer input")
  await expect(composer).toBeEnabled()
  await composer.fill("piensa y lee")
  await page.click('.composer button[type="submit"]')

  const answer = page.locator(".transcript article.assistant").last()
  const thought = answer.locator("details.think")
  await expect(thought.locator("summary")).toContainText("thinking…", { timeout: 10_000 })
  // No card for what was only considered, while it is being considered.
  await expect(answer.locator(".call")).toHaveCount(0)

  // The call that was made, and only that one.
  await expect(answer.locator(".call .status")).toHaveText("done", { timeout: 15_000 })
  await expect(answer.locator(".call")).toHaveCount(1)
  await expect(answer.locator(".call .subject")).toHaveText("AGENTS.md")
  await expect(thought.locator("summary")).toContainText("thought")

  // Folded, and the dropped call is in it when opened.
  await expect(thought.locator(".think-text")).toBeHidden()
  await thought.locator("summary").click()
  await expect(thought.locator(".think-text")).toContainText("NOT-THIS.md")

  await expect(answer.locator(".parts .text").last()).toHaveText("Done.", { timeout: 10_000 })
  expect(errors).toEqual([])
})
