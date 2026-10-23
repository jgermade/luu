// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * Settings → Policies: the shell, network and files lists, seen and edited,
 * and written to `config.toml` whole. An entry added and saved is in the file;
 * a list an environment names cannot be deleted, and renamed it takes the
 * environment with it; a write the loader refuses is not written, and the
 * section says why in the loader's words. See
 * `RECORD/2026-10-22.policies-not-postures.WIP.md`.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7906
const BASE = `http://127.0.0.1:${PORT}`

const CONFIG = `[provider.here]
backend = "mock"
model = "mock"

[shell.tools]
allow = ["ls", "git"]
ask = ["*"]
deny = ["curl"]

[network.off]

[network.registries]
allow = ["crates.io"]

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
  home = mkdtempSync(join(tmpdir(), "luu-policies-"))
  writeFileSync(join(home, "config.toml"), CONFIG)
  server = spawn(
    binary(),
    ["serve", "--bind", `127.0.0.1:${PORT}`, "--provider", "here", "--no-store", "--mock-delay-ms", "0"],
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

const file = () => readFileSync(join(home, "config.toml"), "utf8")

/**
 * The new session's dialog: where it starts from and runs, which list of each
 * axis, where it sends. An environment is a preset: it fills the three lists,
 * one can be swapped for another of the file, and the session runs under the
 * swap — named in what the page reads back. A policy file has no lists to
 * choose. Before the test below, which renames and deletes lists.
 */
test("a session starts from an environment with another list in place of one of its own", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)
  await page.click('.chat .acts button[title^="New session"]')
  const dialog = page.getByRole("dialog", { name: "Sessions" })
  const select = name => dialog.locator(`select.${name}`)

  // The server's own policy file: nothing to choose among its lists.
  await expect(select("preset")).toHaveValue("")
  for (const kind of ["shell", "network", "files"]) await expect(select(`list-${kind}`)).toBeDisabled()

  // An environment fills them, and each can be changed.
  await select("preset").selectOption("asking")
  await expect(select("list-shell")).toHaveValue("tools")
  await expect(select("list-network")).toHaveValue("off")
  await expect(select("list-files")).toHaveValue("tree")
  await expect(select("list-network")).toBeEnabled()
  await select("list-network").selectOption("registries")
  await expect(dialog.locator(".row[data-row='lists']")).toContainText("instead of off")
  await dialog.getByRole("button", { name: "Start session" }).click()
  await expect(dialog).toBeHidden()

  // The session runs under the swap: the environment's other lists, and this one.
  const settings = await (await fetch(`${BASE}/api/settings`)).json()
  expect(settings.lists.environment).toBe("asking")
  expect(settings.lists.shell.name).toBe("tools")
  expect(settings.lists.network.name).toBe("registries")
  expect(settings.lists.network.allow).toEqual(["crates.io"])

  // Lists are chosen on top of an environment, never a policy file's.
  const refused = await fetch(`${BASE}/api/sessions`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ lists: { network: "off" } }),
  })
  expect(refused.status).toBe(400)
  expect(await refused.text()).toContain("a posture's lists are its policy file")

  expect(errors).toEqual([])
})

test("the lists are edited in Settings and written to config.toml whole", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Policies")')
  const pane = page.locator(".policies-pane")
  const tools = pane.locator(".kind[data-kind='shell'] .list[data-list='tools']")
  const zone = (list, name) => list.locator(`.zone[data-zone='${name}']`)
  await expect(zone(tools, "allow").locator(".entry")).toHaveText(["ls×", "git×"])
  await expect(zone(tools, "deny").locator(".entry")).toHaveText(["curl×"])
  await expect(pane.locator(".preset-list li")).toContainText("[environment.asking]")
  const save = pane.locator(".section-actions button.save")
  await expect(save).toBeDisabled()

  // An entry added, and saved: in the file.
  await zone(tools, "allow").locator("input").fill("rg")
  await zone(tools, "allow").locator("input").press("Enter")
  await expect(zone(tools, "allow").locator(".entry")).toHaveText(["ls×", "git×", "rg×"])
  await expect(save).toContainText("1")
  await save.click()
  await expect(save).toBeDisabled()
  // The file as the serialiser lays it out: three entries are one a line.
  expect(file()).toMatch(/\[shell\.tools\]\s*allow = \[\s*"ls",\s*"git",\s*"rg",?\s*\]/)

  // A list an environment names cannot be deleted; renamed, the environment
  // follows it in the same write.
  await expect(tools.getByRole("button", { name: "Delete" })).toBeDisabled()
  await tools.getByRole("button", { name: "Rename" }).click()
  const name = tools.getByRole("textbox", { name: "List name" })
  await name.fill("rust")
  await name.press("Enter")
  const rust = pane.locator(".kind[data-kind='shell'] .list[data-list='rust']")
  await expect(rust).toBeVisible()
  await expect(pane.locator(".preset-list li")).toContainText("rust")
  // One that nothing names goes.
  const registries = pane.locator(".kind[data-kind='network'] .list[data-list='registries']")
  await registries.getByRole("button", { name: "Delete" }).click()
  await expect(registries).toHaveCount(0)
  await save.click()
  await expect(save).toBeDisabled()
  expect(file()).toContain("[shell.rust]")
  expect(file()).not.toContain("[shell.tools]")
  expect(file()).toMatch(/\[environment\.asking\][^[]*shell = "rust"/)
  expect(file()).not.toContain("[network.registries]")
  // And the next session is offered it under the new name.
  const postures = await (await fetch(`${BASE}/api/postures`)).json()
  expect(postures.environments).toContain("asking")

  // A write the loader refuses — a path denied inside one that is allowed,
  // which a command cannot be held to yet — is not written, and says why.
  const tree = pane.locator(".kind[data-kind='files'] .list[data-list='tree']")
  await zone(tree, "deny").locator("input").fill("secrets")
  await zone(tree, "deny").locator("input").press("Enter")
  const before = file()
  await save.click()
  await expect(pane.locator(".section-actions .error")).toContainText("cannot be held")
  expect(file()).toBe(before)
  await pane.getByRole("button", { name: "Discard" }).click()
  await expect(zone(tree, "deny").locator(".entry")).toHaveCount(0)
  await expect(pane.locator(".section-actions .error")).toHaveCount(0)

  // The refusal asked for above, and nothing else: a browser logs every
  // response that is not a success.
  expect(errors).toEqual([
    "console: Failed to load resource: the server responded with a status of 422 (Unprocessable Entity)",
  ])
})
