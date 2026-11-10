// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * Row 52: *Always in this project*. A command the shell list leaves to a person
 * is asked once; the answer is written to luu's config for the session's
 * folder, the next call runs without asking, Settings → Policies lists it, and
 * taking it back there makes the next call ask again. See
 * `RECORD/2026-11-09.always-in-this-project.completed.md`.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7913
const BASE = `http://127.0.0.1:${PORT}`

const RUN = `Checking.
\`\`\`tool
{"name":"run_command","arguments":{"argv":["git","--version"]}}
\`\`\``

const CONFIG = `[provider.here]
backend = "mock"
model = "mock"

[shell.tools]
allow = ["ls"]
`

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
let home = ""

test.beforeAll(async () => {
  home = mkdtempSync(join(tmpdir(), "luu-always-"))
  writeFileSync(join(home, "config.toml"), CONFIG)
  server = spawn(
    binary(),
    [
      "serve", "--bind", `127.0.0.1:${PORT}`, "--provider", "here", "--no-store",
      "--mock-delay-ms", "0",
      "--mock-reply", RUN, "--mock-reply", "Done.",
      "--mock-reply", RUN, "--mock-reply", "Done.",
      "--mock-reply", RUN, "--mock-reply", "Done.",
    ],
    { cwd: root, env: { ...process.env, LUU_HOME: home }, stdio: "pipe" },
  )
  let output = ""
  server.stdout?.on("data", chunk => (output += chunk))
  server.stderr?.on("data", chunk => (output += chunk))
  const deadline = Date.now() + 30_000
  for (;;) {
    if (server.exitCode !== null) throw new Error(`luu serve exited with ${server.exitCode}:\n${output}`)
    try {
      if ((await fetch(`${BASE}/api/settings`)).ok) break
    } catch {
      // Not listening yet.
    }
    if (Date.now() > deadline) throw new Error(`luu serve never answered:\n${output}`)
    await new Promise(again => setTimeout(again, 200))
  }
  const started = await fetch(`${BASE}/api/sessions`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ lists: { shell: "tools" } }),
  })
  if (!started.ok) throw new Error(`no session on the lists: ${await started.text()}`)
})

test.afterAll(() => {
  server?.kill("SIGTERM")
  if (home) rmSync(home, { recursive: true, force: true })
})

test("always in this project is asked once, written for the folder, and taken back in Settings", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  const picker = page.locator("dialog.modal").first()
  await picker.waitFor({ state: "visible", timeout: 5_000 }).catch(() => {})
  if (await picker.isVisible()) {
    await picker.locator('button:has-text("Use this folder")').click()
    await expect(picker).toBeHidden()
  }

  const composer = page.locator(".composer input")
  const ask = async text => {
    await expect(composer).toBeEnabled({ timeout: 15_000 })
    await composer.fill(text)
    await expect(page.locator('.composer button[type="submit"]')).toBeEnabled()
    await page.click('.composer button[type="submit"]')
  }

  // Asked, and answered for the project.
  await ask("which git?")
  const question = page.locator(".question").last()
  await expect(question).toBeVisible({ timeout: 15_000 })
  await question.getByRole("button", { name: "Always in this project" }).click()
  await expect(page.locator(".status", { hasText: "allowed in this project" }).last()).toBeVisible({ timeout: 15_000 })
  const written = readFileSync(join(home, "config.toml"), "utf8")
  expect(written).toMatch(/\[project\."[^"]+"\]\ncommands = \["git"\]/)

  // The next call runs without asking.
  await ask("and again?")
  await expect(page.locator(".status", { hasText: "allowed in this project, earlier" }).last()).toBeVisible({ timeout: 15_000 })
  await expect(page.locator(".question")).toHaveCount(0)

  // Settings → Policies lists it, and takes it back.
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Policies")')
  const always = page.locator('[data-kind="project"]')
  await expect(always.locator("code", { hasText: "git" })).toBeVisible()
  await always.getByRole("button", { name: /Take back run git/ }).click()
  await expect(always.getByText("Nothing yet.")).toBeVisible()
  expect(readFileSync(join(home, "config.toml"), "utf8")).not.toContain("[project")
  await page.keyboard.press("Escape")
  await expect(page.locator(".modal .rail")).toHaveCount(0)

  // Asked again.
  await ask("once more?")
  await expect(page.locator(".question").last()).toBeVisible({ timeout: 15_000 })
  expect(errors).toEqual([])
})
