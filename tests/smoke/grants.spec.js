// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * A command the shell list leaves to a person: the draft's floor does not run
 * it, the card asks, *Allow* with *Apply to session* runs it and lists it, the
 * next draft runs it without asking; taken back, *Deny* for the session refuses it
 * and the next one without asking; lifted, it is asked again. Under an
 * environment out of `config.toml` whose shell list allows `ls` and asks about
 * `git`. The Permissions dialog reads the same session back: each call in one
 * of four words, each a link to what decided it, and the answers for the
 * session with what was taken back still listed. See
 * `RECORD/2026-10-22.policies-not-postures.WIP.md`.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7905
const BASE = `http://127.0.0.1:${PORT}`

const RUN = `Checking.
\`\`\`tool
{"name":"run_command","arguments":{"argv":["git","--version"]}}
\`\`\``

const LS = `Looking.
\`\`\`tool
{"name":"run_command","arguments":{"argv":["ls"]}}
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
      // A call and its answer per turn, six turns: git four times, ls, git.
      "--mock-reply", RUN, "--mock-reply", "Done.",
      "--mock-reply", RUN, "--mock-reply", "Done.",
      "--mock-reply", RUN, "--mock-reply", "Done.",
      "--mock-reply", RUN, "--mock-reply", "Done.",
      "--mock-reply", LS, "--mock-reply", "Done.",
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

test("a command left to a person is asked, allowed or denied for the session, and taken back", async ({ page }) => {
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
  const card = page.locator(".transcript .call", { has: page.locator(".question") })
  const chips = page.locator(".grants .grant")

  // Asked: the floor does not run git, and the shell list leaves it to you.
  await say("which git?")
  await expect(card).toBeVisible({ timeout: 30_000 })
  await expect(card.locator(".why")).toContainText("may not run git")
  await expect(card.locator(".question pre")).toHaveText("git --version")
  await expect(card.locator(".answers button")).toHaveText(["Allow", "Deny"])
  const forTheSession = card.getByRole("checkbox", { name: "Apply to session" })
  await expect(forTheSession).not.toBeChecked()
  await card.getByText("Apply to session").click()
  await expect(forTheSession).toBeChecked()
  await card.locator('.answers button:text-is("Allow")').click()
  await expect(calls.nth(0).locator(".status")).toHaveText("allowed for the session", { timeout: 15_000 })
  await expect(chips).toHaveCount(1)
  await expect(chips).toContainText("run git")

  // The next draft's call runs without a question.
  await say("again")
  await expect(calls.nth(1).locator(".status")).toHaveText("allowed earlier", { timeout: 30_000 })
  await expect(page.locator(".transcript .call .question")).toHaveCount(0)

  // Taken back, and denied for the session: refused now and next time, unasked.
  await chips.getByRole("button", { name: "Take back running git" }).click()
  await expect(chips).toHaveCount(0)
  await say("once more")
  await expect(card).toBeVisible({ timeout: 30_000 })
  await card.getByText("Apply to session").click()
  await card.locator('.answers button:text-is("Deny")').click()
  await expect(calls.nth(2).locator(".status")).toHaveText("denied for the session", { timeout: 15_000 })
  await expect(chips).toHaveCount(1)
  await expect(chips).toHaveClass(/forbidden/)
  await say("and again")
  await expect(calls.nth(3).locator(".status")).toHaveText("denied earlier", { timeout: 30_000 })
  await expect(page.locator(".transcript .call .question")).toHaveCount(0)
  // And a program the list allows, which the floor runs.
  await say("what is here?")
  await expect(calls.nth(4).locator(".status")).toHaveText("done", { timeout: 30_000 })

  // The dialog: each call in one of four words, newest first.
  await page.locator(".chat .col-head").getByRole("button", { name: "Permissions" }).click()
  const dialog = page.locator("dialog", { hasText: "Permissions" })
  await expect(dialog).toBeVisible()
  const rows = dialog.locator(".rows li")
  await expect(rows.locator(".who")).toHaveText(["granted", "forbidden", "forbidden", "granted", "granted"])
  await dialog.getByRole("radio", { name: "Refused" }).click()
  await expect(rows).toHaveCount(2)
  await dialog.getByRole("radio", { name: "All", exact: true }).click()

  // Each word leads to what decided it. ls: the shell list's entry.
  await rows.nth(0).getByRole("button", { name: "granted", exact: true }).click()
  await expect(dialog.getByRole("tab", { selected: true })).toContainText("Policies")
  await expect(dialog.locator(".entry.focused")).toHaveText("ls")
  await expect(dialog.locator(".followed")).toContainText("ls")
  await dialog.getByRole("button", { name: "Back to activity" }).click()
  await expect(rows.nth(0)).toHaveClass(/focused/)

  // The second git: the yes for the session, now revoked and still listed.
  await rows.nth(3).getByRole("button", { name: "granted", exact: true }).click()
  await expect(dialog.getByRole("tab", { selected: true })).toContainText("Granted")
  const yes = dialog.locator(".answers li")
  await expect(yes).toHaveCount(1)
  await expect(yes).toHaveClass(/focused/)
  await expect(yes).toHaveClass(/revoked/)
  await expect(yes).toContainText("run git")
  await expect(yes).toContainText("2 calls")
  await expect(yes).toContainText("revoked")

  // The last git: the no for the session, in force, lifted from here.
  await dialog.getByRole("button", { name: "Back to activity" }).click()
  await rows.nth(1).getByRole("button", { name: "forbidden" }).click()
  await expect(dialog.getByRole("tab", { selected: true })).toContainText("Forbidden")
  const no = dialog.locator(".answers li")
  await expect(no).toHaveCount(1)
  await expect(no).toHaveClass(/focused/)
  await expect(no).toContainText("2 calls")
  await no.getByRole("button", { name: "Lift" }).click()
  await expect(no).toContainText("lifted")
  await expect(no).toHaveClass(/revoked/)
  await dialog.getByRole("button", { name: "Done" }).click()
  await expect(dialog).toBeHidden()
  await expect(chips).toHaveCount(0)

  // Lifted: asked again, and a no this time is the fourth word.
  await say("one last time")
  await expect(card).toBeVisible({ timeout: 30_000 })
  // A new question starts with the box clear: once, unless said otherwise.
  await expect(card.getByRole("checkbox", { name: "Apply to session" })).not.toBeChecked()
  await card.locator('.answers button:text-is("Deny")').click()
  await expect(calls.nth(5).locator(".status")).toHaveText("you denied it", { timeout: 15_000 })
  await page.locator(".chat .col-head").getByRole("button", { name: "Permissions" }).click()
  await expect(rows.nth(0).locator(".who")).toHaveText("denied")

  // A command the list allows and the kernel cannot hold — macOS under
  // `enforcement = "kernel"` — is traced past the list, to the enforcement,
  // rather than to its own `allow` entry. Linux holds it here, so the call is
  // put in the page's history as the server words it there.
  await page.evaluate(async () => {
    const { state } = await import("./lib/store.js")
    const kept = state.history[state.history.length - 1]
    state.history = [...state.history, { ...kept, turn: 99, job: null, tools: [{
      step: 1, name: "run_command", arguments: { argv: ["ls", "-la"] },
      verdict: {
        allowed: false,
        rule: "the kernel cannot hold this child (landlock and seccomp are Linux-only, and this is macos); a draft runs on the floor, which writes only to scratch — approve a plan to run this",
        enforced_by: "process",
      },
      asked: null, grant: null, held: null,
    }] }]
  })
  await expect(rows.nth(0).locator(".who")).toHaveText("forbidden")
  await rows.nth(0).getByRole("button", { name: "forbidden", exact: true }).click()
  await expect(dialog.getByRole("tab", { selected: true })).toContainText("Policies")
  await expect(dialog.locator(".marked.focused")).toContainText("kernel")
  await expect(dialog.locator(".entry.focused")).toHaveCount(0)
  await expect(dialog.locator(".followed .note")).toContainText("ls is in allow")

  // Where the lists are edited: the dialog plays its way out, and only then
  // does Settings open, on Policies. Opened at once, Settings unmounted the
  // dialog in the middle of its exit.
  await dialog.getByRole("button", { name: "Settings → Policies" }).click()
  await expect(dialog).toHaveClass(/\bleaving\b/)
  expect(await page.locator(".policies-pane").count(), "Settings waits for the exit").toBe(0)
  await expect(dialog).toHaveCount(0)
  await expect(page.locator(".modal .rail button.on")).toHaveText("Policies")
  await expect(page.locator(".policies-pane")).toBeVisible()

  expect(errors).toEqual([])
})
