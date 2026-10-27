// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * A turn at its tool limit asks rather than ends: the card under its last
 * call, *more*, *stop*, *no limit for the session* — and Auto, which answers
 * *more* itself. Against a live `luu serve` on the mock, started with
 * `--max-tool-steps 2` and a model that makes three calls before it answers,
 * over and over. And Settings → General writing the file's own limit, which
 * that flag holds over. See
 * `RECORD/2026-10-27.a-turn-asks-before-its-tool-limit-ends-it.completed.md`.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7907
const BASE = `http://127.0.0.1:${PORT}`

const CALL = `Looking.
\`\`\`tool
{"name":"list_dir","arguments":{"path":"."}}
\`\`\``

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
  home = mkdtempSync(join(tmpdir(), "luu-limit-"))
  writeFileSync(join(home, "config.toml"), '[provider.here]\nbackend = "mock"\nmodel = "mock"\n')
  server = spawn(
    binary(),
    [
      "serve",
      "--bind", `127.0.0.1:${PORT}`,
      "--no-store",
      "--mock-delay-ms", "0",
      "--max-tool-steps", "2",
      // Three calls and an answer, round and round: every turn that starts on
      // a call reaches its limit at the third.
      "--mock-reply", CALL,
      "--mock-reply", CALL,
      "--mock-reply", CALL,
      "--mock-reply", "Done.",
      "--mock-cycle",
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

test("a turn at its tool limit asks, and Auto goes on by itself", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)

  const metas = page.locator(".transcript button.meta")
  const ask = page.locator(".limit-ask")
  const send = async text => {
    await page.locator(".composer input").fill(text)
    await page.click('.composer button[type="submit"]')
  }
  const ended = async n => {
    await expect(metas).toHaveCount(n, { timeout: 20_000 })
    return metas.nth(n - 1)
  }

  // Two calls, and the model asks for a third: the turn waits on the card,
  // not on the model. *Continue* lets it make two more, and it answers.
  await send("list it")
  await expect(ask).toBeVisible({ timeout: 20_000 })
  await expect(ask).toContainText("2 tool calls")
  await expect(ask.getByRole("button", { name: "Continue · 2 more" })).toBeVisible()
  await expect(page.locator(".transcript .waiting")).toHaveCount(0)
  await ask.getByRole("button", { name: "Continue · 2 more" }).click()
  await expect(ask).toHaveCount(0)
  let meta = await ended(1)
  await expect(meta.locator(".warn")).toHaveCount(0)

  // *Stop here* is the end every turn had before anybody was asked.
  await send("again")
  await expect(ask).toBeVisible({ timeout: 20_000 })
  await ask.getByRole("button", { name: "Stop here" }).click()
  meta = await ended(2)
  await expect(meta.locator(".warn")).toHaveText("tool_limit")
  // The mock's answer, on its own: no calls, nothing asked.
  await send("say it")
  await ended(3)

  // Auto answers *more* itself: the card never stays, and the turn answers.
  const face = page.locator(".options .dropup .face")
  await face.click()
  await page.locator(".options .dropup .list .opt", { hasText: "Auto" }).click()
  await expect(face).toContainText("Auto")
  await send("on your own")
  meta = await ended(4)
  await expect(meta.locator(".warn")).toHaveCount(0)
  await expect(ask).toHaveCount(0)
  await face.click()
  await page.locator(".options .dropup .list .opt", { hasText: "Manual" }).click()

  // *No limit this session*: asked once, and never again in it.
  await send("all of it")
  await expect(ask).toBeVisible({ timeout: 20_000 })
  await ask.getByRole("button", { name: "No limit this session" }).click()
  await ended(5)
  await send("and more")
  await ended(6)
  await expect(ask).toHaveCount(0)

  // Settings → General: the file's own limit, written; and said to be held
  // over by the flag this server was started with.
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("General")')
  const field = page.getByRole("textbox", { name: "Tool calls per turn" })
  await expect(field).toHaveAttribute("placeholder", "8")
  await field.fill("12")
  await field.press("Enter")
  await expect.poll(() => readFileSync(join(home, "config.toml"), "utf8")).toContain("max-tool-steps = 12")
  await expect(page.locator(".general-pane")).toContainText("--max-tool-steps 2")
  await field.fill("")
  await field.press("Enter")
  await expect.poll(() => readFileSync(join(home, "config.toml"), "utf8")).not.toContain("max-tool-steps")

  expect(errors).toEqual([])
})
