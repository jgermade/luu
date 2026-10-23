// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * A command the shell list leaves to a person: the draft's floor does not run
 * it, the card asks, *Allow for the session* runs it and lists it, the next
 * draft runs it without asking, and taking it back makes the one after ask
 * again. Under an environment out of `config.toml` whose shell list allows
 * `ls` and asks about `git`. The Permissions dialog reads the same session
 * back: who let each call through, what was used, and what is granted. See
 * `RECORD/2026-10-22.policies-not-postures.WIP.md`, step 2a.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7905
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
ask = ["git"]

[network.off]

[files.tree]
allow = [{ path = ".", access = "read-write" }]

[environment.asking]
shell = "tools"
network = "off"
files = "tree"
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
  home = mkdtempSync(join(tmpdir(), "luu-grants-"))
  writeFileSync(join(home, "config.toml"), CONFIG)
  server = spawn(
    binary(),
    [
      "serve",
      "--bind", `127.0.0.1:${PORT}`,
      // Named, so a new session keeps this destination — and these replies —
      // rather than following the file's default to a fresh mock.
      "--provider", "here",
      "--no-store",
      "--mock-delay-ms", "0",
      // A call and its answer per turn, three turns.
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
  // The session runs under the environment, not the checkout's `luu.toml`.
  const started = await fetch(`${BASE}/api/sessions`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    // Only the environment: the destination stays the one the flags set up.
    body: JSON.stringify({ posture: "asking" }),
  })
  if (!started.ok) throw new Error(`no session on the environment: ${await started.text()}`)
})

test.afterAll(() => {
  server?.kill("SIGTERM")
  if (home) rmSync(home, { recursive: true, force: true })
})

async function chooseFolder(page) {
  const picker = page.locator("dialog.modal").first()
  await picker.waitFor({ state: "visible", timeout: 5_000 }).catch(() => {})
  if (await picker.isVisible()) {
    await picker.locator('button:has-text("Use this folder")').click()
    await expect(picker).toBeHidden()
  }
}

test("a command left to a person is asked, allowed for the session, and taken back", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)
  const composer = page.locator(".composer input")
  const say = async text => {
    await expect(composer).toBeEnabled({ timeout: 15_000 })
    await composer.fill(text)
    await page.click('.composer button[type="submit"]')
  }
  // The chat's cards: the inspector lists the same calls under the same class.
  const calls = page.locator(".transcript .call", { hasText: "run_command" })

  // Asked: the floor does not run git, and the shell list leaves it to you.
  await say("which git?")
  const card = page.locator(".transcript .call", { has: page.locator(".question") })
  await expect(card).toBeVisible({ timeout: 30_000 })
  await expect(card.locator(".why")).toContainText("may not run git")
  await expect(card.locator(".question pre")).toHaveText("git --version")
  await expect(card.locator(".answers button")).toHaveText(["Allow this time", "Allow for the session", "Deny"])
  await card.locator('.answers button:has-text("Allow for the session")').click()
  await expect(calls.nth(0).locator(".status")).toHaveText("allowed for the session", { timeout: 15_000 })
  const grant = page.locator(".grants .grant")
  await expect(grant).toHaveCount(1)
  await expect(grant).toContainText("run git")

  // The next draft's call runs without a question.
  await say("again")
  await expect(calls.nth(1).locator(".status")).toHaveText("allowed earlier", { timeout: 30_000 })
  await expect(page.locator(".transcript .call .question")).toHaveCount(0)

  // The Permissions dialog: each call and who let it through, newest first;
  // what was used, beside where the lists put it; what is granted.
  await page.locator(".chat .col-head").getByRole("button", { name: "Permissions" }).click()
  const dialog = page.locator("dialog", { hasText: "Permissions" })
  await expect(dialog).toBeVisible()
  const rows = dialog.locator(".rows li")
  await expect(rows).toHaveCount(2)
  await expect(rows.locator(".who")).toHaveText(["your grant", "you, for the session"])
  await expect(rows.nth(0)).toContainText("git --version")
  await dialog.getByRole("tab", { name: /Used/ }).click()
  const git = dialog.locator(".tally tr", { hasText: "git" })
  await expect(git.locator("td")).toHaveText(["git", "2", "0", "1", "ask"])
  await expect(dialog.locator(".suggestion")).toContainText('allow = ["git"]')
  await dialog.getByRole("tab", { name: /Granted/ }).click()
  await expect(dialog.locator(".grants li")).toContainText("run git")
  await expect(dialog.locator(".list", { hasText: "shell" })).toContainText("[shell.tools]")
  await expect(dialog.locator(".list", { hasText: "shell" }).locator(".zone-row", { hasText: "ask" })).toContainText("git")
  await dialog.getByRole("button", { name: "Done" }).click()
  await expect(dialog).toBeHidden()

  // Taken back: the one after is asked again, and a no is a no.
  await grant.getByRole("button", { name: "Take back running git" }).click()
  await expect(grant).toHaveCount(0)
  await say("once more")
  await expect(card).toBeVisible({ timeout: 30_000 })
  await card.locator('.answers button:has-text("Deny")').click()
  await expect(calls.nth(2).locator(".status")).toHaveText("you denied it", { timeout: 15_000 })

  // And the no is in the activity, under Refused.
  await page.locator(".chat .col-head").getByRole("button", { name: "Permissions" }).click()
  await expect(dialog).toBeVisible()
  await dialog.getByRole("radio", { name: "Refused" }).click()
  await expect(rows).toHaveCount(1)
  await expect(rows.locator(".who")).toHaveText("you refused")

  expect(errors).toEqual([])
})
