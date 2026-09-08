// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * The half of the page the static twin cannot reach: the socket, the gate, an
 * approval, a tool call and the fold. Against a live `luu serve` on the mock —
 * no model, no network, three replies.
 *
 * Why it exists: two bugs in two days were found by a person opening this page
 * and none by a test. See
 * `RECORD/2026-09-08.a-test-that-clicks-approve.completed.md`.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7897
const BASE = `http://127.0.0.1:${PORT}`

/** The plan the model proposes. It does **not** declare README.md. */
const PLAN = `\`\`\`plan
{"objective": "say what this repository is",
 "steps": ["read the readme"],
 "files": ["AGENTS.md"],
 "commands": []}
\`\`\``

/**
 * The call the turn makes once the job is approved — of a path the plan never
 * asked for. It succeeds only if the amendment typed into the browser reached
 * the server and became part of the job's sandbox, which is the bug
 * `RECORD/2026-09-07.a-second-header.completed.md` found by hand. And the
 * amendment grants a *file*, so the job's root is a file: the bug
 * `RECORD/2026-09-08.the-surfaces-first.completed.md` found, asserted where it
 * was found.
 */
const CALL = `\`\`\`tool
{"name":"read_file","arguments":{"path":"README.md","max_lines":1}}
\`\`\``

const ANSWER = "It is luu."

/**
 * The binary under test. Not a skip when it is missing: a skipped test is a
 * test that is not there, and things that are not there is what this file
 * exists to catch.
 */
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
  // Its own state directory, so the run reads no `config.toml` of the person
  // running it and writes nothing into theirs.
  home = mkdtempSync(join(tmpdir(), "luu-gate-"))
  server = spawn(
    binary(),
    [
      "serve",
      "--bind", `127.0.0.1:${PORT}`,
      "--no-store",
      "--mock-delay-ms", "0",
      "--mock-reply", PLAN,
      "--mock-reply", CALL,
      "--mock-reply", ANSWER,
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

test("a prompt is planned, amended, approved, run and folded", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  await page.goto(`${BASE}/index.html`)
  await expect(page.locator("header strong")).toHaveText("luu")

  // The first run: nothing in `config.toml` says where this machine sends, so
  // the page opens on the editor rather than on a chat box that would answer
  // from the mock without saying so.
  const modal = page.locator(".modal-backdrop").first()
  await expect(modal).toBeVisible({ timeout: 15_000 })
  await expect(page.locator(".first-run")).toBeVisible()
  await modal.locator("button.link", { hasText: "close" }).click()
  await expect(modal).toBeHidden()

  // The prompt is held, unrun, until somebody answers for it.
  const composer = page.locator(".composer input")
  await expect(composer).toBeEnabled()
  await composer.fill("what is this repository?")
  await page.click('.composer button[type="submit"]')

  const gate = page.locator(".gate")
  await expect(gate).toBeVisible({ timeout: 30_000 })
  await expect(gate.locator(".objective")).toHaveText("say what this repository is")
  await expect(gate.locator(".plan li")).toHaveText(["read the readme"])
  await expect(gate.locator("p", { hasText: "reads:" })).toContainText("AGENTS.md")
  // Held, and nothing has run under it: the composer is refused while it waits.
  await expect(composer).toBeDisabled()

  // What the model forgot, added by hand. The plan is the job's sandbox, so
  // this is the only moment the file the turn will actually read can get in.
  await page.fill('.amend input[placeholder*="a path or command"]', "README.md")
  await page.click('.amend button:has-text("add read")')
  await expect(page.locator("p", { hasText: "adding reads:" })).toContainText("README.md")

  await page.click('.gate-buttons button:has-text("Approve")')
  await expect(gate).toBeHidden({ timeout: 15_000 })

  // The call the plan never declared, allowed by the amendment and answered.
  const call = page.locator(".timeline li").first()
  await expect(call).toBeVisible({ timeout: 30_000 })
  await expect(call.locator(".call b")).toHaveText("read_file")
  await expect(call.locator(".verdict .ok")).toHaveText("allowed")
  await expect(call.locator(".verdict")).toContainText("README.md")
  // A read that was granted and then failed — `ENOTDIR` on a file root was
  // exactly that — reads as an error line here, and as no bytes coming back.
  await expect(page.locator(".timeline .err")).toHaveCount(0)
  await expect(call.locator(".held")).toContainText("bytes back")

  // `toContainText`, not `toHaveText`: a turn that used a tool is one assistant
  // message carrying every round trip it made — the fenced call it emitted and
  // then the answer.
  await expect(page.locator("article.assistant .text").last()).toContainText(ANSWER)

  // One entry where its turns were.
  const live = page.locator(".live-job")
  await expect(live).toBeVisible()
  await live.locator("button", { hasText: "close & fold" }).click()
  await expect(live).toBeHidden({ timeout: 15_000 })
  await expect(page.locator(".fold .summary")).toBeVisible()

  // The fold, with what it cost: the compaction log is the fourth of the four
  // panels the design says justify building this client, and the numbers in it
  // are counted at the close because they are only true then. See
  // `RECORD/2026-09-08.what-a-fold-writes-down.completed.md`.
  const fold = page.locator(".folds li").first()
  await expect(fold).toBeVisible()
  await expect(fold).toContainText("job 1")
  // Both numbers come from the close and neither is counted in the page, so
  // this asserts digits rather than the word: an empty span either side would
  // still have read as "tokens of history … saved".
  await expect(fold).toContainText(/\d+ tokens of history →\s+\d+ in the line/)
  await expect(fold).toContainText(/-?\d+ saved/)
  await expect(fold).not.toContainText("not recorded")

  // And the two turns are still readable, without a reload: the panel keeps
  // what it was showing instead of resetting when the next turn starts.
  await expect(page.locator(".turn-picker select option")).toHaveText([
    "live", "turn 1", "turn 2",
  ])

  expect(errors, "the page logged errors").toEqual([])
})

/**
 * The panel over a turn that is no longer the live one — from the stream while
 * the page stays open, and from the read API after it is reloaded. Runs on the
 * session the test above left behind: turn 1 is the planning call, turn 2 is
 * the approved run with the tool call in it.
 *
 * See `RECORD/2026-09-08.the-panel-keeps-the-turn.completed.md`.
 */
test("the inspector can be pointed at a turn that has ended", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  // Reloaded, so the history comes from `/api/sessions/live` rather than from
  // messages this page watched go by. Both fill the same panel and only one of
  // them survives a refresh.
  await page.goto(`${BASE}/index.html`)
  // The same first run as above, and waited for rather than polled: it opens a
  // beat after the socket says hello, and a check that ran before that left it
  // to pop open later over the panel being clicked.
  const modal = page.locator(".modal-backdrop").first()
  await expect(modal).toBeVisible({ timeout: 15_000 })
  await modal.locator("button.link", { hasText: "close" }).click()
  await expect(modal).toBeHidden()

  const picker = page.locator(".turn-picker select")
  await expect(picker.locator("option")).toHaveText(["live", "turn 1", "turn 2"])

  // The planning call: a prompt was sent, and it used no tools.
  await picker.selectOption("1")
  await expect(page.locator(".reading")).toContainText("Turn 1 as it was sent")
  await expect(page.locator(".inspector pre")).toContainText("You are luu")
  await expect(page.locator(".timeline li")).toHaveCount(0)

  // The approved run: the call the amendment made possible, still there.
  await picker.selectOption("2")
  await expect(page.locator(".reading")).toContainText("Turn 2 as it was sent")
  await expect(page.locator(".timeline li")).toHaveCount(1)
  await expect(page.locator(".timeline .call b")).toHaveText("read_file")
  await expect(page.locator(".timeline .verdict")).toContainText("README.md")
  // Every bucket of that turn's budget, not the empty one a reset leaves.
  await expect(page.locator(".legend li")).not.toHaveCount(0)

  await page.locator(".turn-picker button", { hasText: "back to live" }).click()
  await expect(page.locator(".reading")).toHaveCount(0)

  expect(errors, "the page logged errors").toEqual([])
})
