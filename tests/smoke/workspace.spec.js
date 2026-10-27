// @ts-check
import { expect, test } from "@playwright/test"
import { execSync, spawn } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * The file tree and the open file follow the disk.
 *
 * Its own server, started in a scratch repository rather than in this one,
 * because the test writes files: the other live specs serve the checkout, and
 * a test that wrote into it would leave the working tree dirty. See
 * `RECORD/2026-10-01.the-tree-follows-the-disk.completed.md`.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7898
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
let scratch = ""

test.beforeAll(async () => {
  scratch = mkdtempSync(join(tmpdir(), "luu-workspace-"))
  const work = join(scratch, "work")
  mkdirSync(join(work, "src"), { recursive: true })
  const lines = Array.from({ length: 600 }, (_, n) => `line ${n + 1}`).join("\n")
  writeFileSync(join(work, "src/long.txt"), `${lines}\n`)
  const git = args => execSync(`git -c user.email=t@t -c user.name=t ${args}`, { cwd: work })
  git("init -q")
  git("add .")
  git("commit -qm init")

  server = spawn(binary(), ["serve", "--bind", `127.0.0.1:${PORT}`, "--mock-delay-ms", "0"], {
    cwd: work,
    // `SHELL` is the host terminal's shell: the plain one, so the test does
    // not depend on the login rc files of whoever runs it — which may ask
    // for a passphrase, as one did here.
    env: { ...process.env, LUU_HOME: join(scratch, "home"), SHELL: "/bin/sh" },
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
  rmSync(scratch, { recursive: true, force: true })
})

test("the tree and the open file follow the disk", async ({ page }) => {
  const work = join(scratch, "work")
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })
  let statusAsked = 0
  page.on("request", request => {
    if (request.url().includes("/api/workspace/git-status")) statusAsked++
  })

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  const picker = page.locator("dialog.modal").first()
  await picker.locator('button:has-text("Use this folder")').click()
  await expect(picker).toBeHidden()

  await page.click('.inspector .tabs button:has-text("Files")')
  const tree = page.locator(".inspector .tree")
  await tree.locator('button:has-text("src")').first().click()
  await tree.locator('button:has-text("long.txt")').first().click()
  const rows = page.locator(".content .code-rows li")
  await expect(rows).toHaveCount(601)

  // Scrolled half way, then written to from outside the page. The file on
  // screen changes and stays where it was: re-read, not reopened.
  const scroller = await page.evaluateHandle(() => {
    let el = document.querySelector(".content .code-rows")
    while (el && !(el.scrollHeight > el.clientHeight && getComputedStyle(el).overflowY !== "visible")) {
      el = el.parentElement
    }
    if (el) el.scrollTop = 4000
    return el
  })
  const file = join(work, "src/long.txt")
  writeFileSync(file, readFileSync(file, "utf8").replace("line 1\n", "CHANGED 1\n"))
  await expect(rows.first()).toContainText("CHANGED 1")
  expect(await scroller.evaluate(el => el?.scrollTop)).toBe(4000)
  // And git's letter came with it, on the row the tree already had.
  await expect(tree.locator("li", { hasText: "long.txt" }).locator(".status")).toHaveText(" M")

  // A folder made and then removed: it appears, and it goes — without the
  // page asking for a directory that is no longer there.
  mkdirSync(join(work, "src/fresh"))
  writeFileSync(join(work, "src/fresh/new.rs"), "fn main() {}\n")
  await tree.locator('button:has-text("fresh")').first().click()
  await expect(tree.locator('button:has-text("new.rs")')).toBeVisible()
  rmSync(join(work, "src/fresh"), { recursive: true })
  await expect(tree.locator('button:has-text("fresh")')).toHaveCount(0)

  // Staged in a terminal: only the index moved, and the letter says so.
  execSync("git add src/long.txt", { cwd: work })
  await expect(tree.locator("li", { hasText: "long.txt" }).locator(".status")).toHaveText("M ")

  // At rest, git is not asked again. `git status` rewrites the index now and
  // then, and an unthrottled page would answer its own write forever.
  await page.waitForTimeout(1500)
  const settled = statusAsked
  await page.waitForTimeout(4000)
  expect(statusAsked - settled, "git status asked while nothing changed").toBe(0)

  expect(errors, "the page logged errors").toEqual([])
})

/**
 * The open file's tab: the file's icon, and a button between the name and the
 * close that shows git's diff in a modal — only while there is one. The foot
 * named the file until the tab took it over; see
 * `RECORD/2026-10-01.the-foot-names-its-file.completed.md`. Then the column's
 * one foot, which carries the terminal's controls while it is up.
 */
test("the tab shows a changed file's diff in a modal", async ({ page }) => {
  const work = join(scratch, "work")
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })
  const git = args => execSync(`git -c user.email=t@t -c user.name=t ${args}`, { cwd: work })
  writeFileSync(join(work, "src/foot.txt"), "one\ntwo\nthree\n")
  git("add src/foot.txt")
  git("commit -qm foot")

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  const picker = page.locator("dialog.modal").first()
  await picker.locator('button:has-text("Use this folder")').click()
  await expect(picker).toBeHidden()
  await page.evaluate(async () => {
    const { showFile } = await import("./lib/workspace.js")
    await showFile("src/foot.txt")
  })
  const tab = page.locator(".content .tabs.files .tab.on")
  await expect(tab.locator(".pick")).toHaveAttribute("title", "src/foot.txt")
  // The tree's shape, before the name: this server names no icon theme.
  await expect(tab.locator('svg.kind use[href="#i-file"]')).toHaveCount(1)
  // A strip under the content's head, not an icon on the tab: how much each
  // side of git changed, and the way into the diff.
  const strip = page.locator(".content .strip")
  await expect(tab.locator("button.diff")).toHaveCount(0)
  // Committed and untouched: nothing to show, so no strip.
  await expect(strip).toHaveCount(0)

  writeFileSync(join(work, "src/foot.txt"), "one\nTWO\nthree\n")
  await expect(strip).toContainText("working tree")
  await expect(strip.locator(".add")).toHaveText("+1")
  await expect(strip.locator(".del")).toHaveText("−1")
  // In the flow: the file starts under it, not under the head.
  const stripBottom = await strip.evaluate(el => el.getBoundingClientRect().bottom)
  const fileTop = await page.locator(".content .col-main").evaluate(el => el.getBoundingClientRect().top)
  expect(Math.abs(fileTop - stripBottom)).toBeLessThan(2)
  await strip.click()
  const modal = page.locator("dialog.modal")
  await expect(modal.locator(".modal-head strong")).toHaveText("src/foot.txt")
  await expect(modal.locator(".line.add")).toContainText("TWO")
  await expect(modal.locator(".line.del")).toContainText("two")
  await expect(modal.locator('.which button.on')).toHaveText("working tree")

  // Staged while it is open: the working tree side empties, and the other
  // side is one click away.
  git("add src/foot.txt")
  await expect(modal.locator(".diff .pad")).toBeVisible()
  await modal.locator('.which button:has-text("staged")').click()
  await expect(modal.locator(".line.add")).toContainText("TWO")

  await modal.locator(".modal-head button.close", { hasText: "close" }).click()
  await expect(modal).toHaveCount(0)
  // And the file is still what the column shows: the modal was a glance.
  await expect(tab.locator(".pick")).toHaveAttribute("title", "src/foot.txt")

  // This server runs its tools on the host and is bound on loopback, so the
  // terminal opens here, in the folder the session works in. See
  // `RECORD/2026-10-01.the-terminal-follows-the-session.completed.md`.
  const foot = page.locator(".content .col-foot")
  const terminal = foot.locator("button.term")
  await expect(terminal).toHaveAttribute("title", "Open a terminal on this machine, where the session runs")
  await terminal.click()
  await expect(terminal).toHaveClass(/\bon\b/)
  const panel = page.locator(".content .terminal")
  // Open, on the host, and the foot says so under *Runtime*, by the runtime's
  // name and this machine's mark.
  await expect(foot.locator('.state.open[data-place="host"]')).toHaveCount(1, { timeout: 15_000 })
  await expect(foot.locator(".title")).toHaveText("Runtime")
  await expect(foot.locator("button.place")).toHaveText("host")
  await expect(foot.locator('button.place use[href="#i-machine"]')).toHaveCount(1)
  // Its dialog names the place it runs now; with nowhere else to go, there is
  // nothing to confirm and nothing to move.
  await foot.locator("button.place").click()
  const runtime = page.getByRole("dialog", { name: "Runtime" })
  await expect(runtime.getByRole("radio", { name: /host.*now/ })).toHaveAttribute("aria-checked", "true")
  await expect(runtime.getByRole("button", { name: "Move the session" })).toBeDisabled()
  await expect(runtime.getByRole("checkbox")).toHaveCount(0)
  await runtime.getByRole("button", { name: "Cancel" }).click()
  await expect(runtime).toHaveCount(0)
  await panel.locator(".host").click()
  // Arithmetic, so what is matched is the shell's answer and not the echo.
  await page.keyboard.type("echo AT=$(pwd -P) N=$((40+2))\n")
  const real = execSync("pwd -P", { cwd: work }).toString().trim()
  await expect(panel.locator(".xterm-rows")).toContainText(`AT=${real} N=42`, { timeout: 15_000 })
  // Ended from inside, the one way left to end it, and put away.
  await page.keyboard.type("exit\n")
  await expect(foot.locator(".state.ended")).toHaveCount(1, { timeout: 15_000 })
  await terminal.click()
  await expect(panel).toHaveCount(0)
  await expect(terminal).not.toHaveClass(/\bon\b/)

  expect(errors, "the page logged errors").toEqual([])
})

/**
 * The pencil renames, in place: a file, its open tab following it; a name
 * that is taken, refused where it was typed with nothing moved; ESC putting
 * the name back; and a folder, staying open under its new name. The one
 * write the tree makes — see `RECORD/2026-10-21.the-tree-renames.completed.md`.
 */
test("the pencil renames a file or a folder in place", async ({ page }) => {
  const work = join(scratch, "work")
  mkdirSync(join(work, "names/inner"), { recursive: true })
  writeFileSync(join(work, "names/hello.txt"), "hello\n")
  writeFileSync(join(work, "names/taken.txt"), "taken\n")
  writeFileSync(join(work, "names/inner/deep.txt"), "deep\n")
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  const picker = page.locator("dialog.modal").first()
  await picker.waitFor({ state: "visible", timeout: 5_000 }).catch(() => {})
  if (await picker.isVisible()) await picker.locator('button:has-text("Use this folder")').click()
  await page.click('.inspector .tabs button:has-text("Files")')
  const tree = page.locator(".inspector .tree")
  const node = name => tree.locator(".node", { has: page.locator(`.name:text-is("${name}")`) })
  await node("names").locator("button.row").click()
  await node("hello.txt").locator("button.row").click()
  await expect(page.locator(".content .code-rows li").first()).toContainText("hello")

  // The field starts on the name, with what is before the extension chosen.
  await node("hello.txt").hover()
  await node("hello.txt").getByRole("button", { name: "Rename hello.txt" }).click()
  const field = tree.getByRole("textbox", { name: "New name for hello.txt" })
  await expect(field).toBeFocused()
  expect(await field.evaluate(input => [input.selectionStart, input.selectionEnd])).toEqual([0, 5])

  // Taken: said under the row, the field still open, nothing moved.
  await field.fill("taken.txt")
  await field.press("Enter")
  await expect(tree.getByRole("alert")).toContainText("already exists")
  await expect(field).toBeVisible()
  expect(readFileSync(join(work, "names/taken.txt"), "utf8")).toBe("taken\n")

  await field.fill("hi.txt")
  await field.press("Enter")
  await expect(node("hi.txt")).toBeVisible()
  await expect(node("hello.txt")).toHaveCount(0)
  expect(readFileSync(join(work, "names/hi.txt"), "utf8")).toBe("hello\n")
  // The open tab followed the file.
  await expect(page.locator(".content .col-head")).toContainText("hi.txt")

  // F2 on a row starts it, and ESC puts the name back.
  await node("hi.txt").locator("button.row").focus()
  await page.keyboard.press("F2")
  await expect(tree.getByRole("textbox", { name: "New name for hi.txt" })).toBeFocused()
  await page.keyboard.type("nope")
  await page.keyboard.press("Escape")
  await expect(tree.locator(".node.renaming")).toHaveCount(0)
  expect(existsSync(join(work, "names/hi.txt"))).toBe(true)

  // A folder, open: it stays open under its new name, with what is in it.
  await node("inner").locator("button.row").click()
  await expect(node("deep.txt")).toBeVisible()
  await node("inner").hover()
  await node("inner").getByRole("button", { name: "Rename inner" }).click()
  const folder = tree.getByRole("textbox", { name: "New name for inner" })
  await folder.fill("outer")
  await folder.press("Enter")
  await expect(node("outer")).toBeVisible()
  await expect(node("deep.txt").locator("button.row")).toHaveAttribute("title", "names/outer/deep.txt")
  expect(existsSync(join(work, "names/outer/deep.txt"))).toBe(true)

  expect(errors, "the page threw").toEqual([])
})
