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
    // A store, in the home above: a session made from lists comes back made
    // from them.
    ["serve", "--bind", `127.0.0.1:${PORT}`, "--provider", "here", "--mock-delay-ms", "0"],
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
 * The new session's dialog: where it runs and which list of each axis, always
 * — no preset to start from, and a list not chosen is the empty one, which
 * reads the folder and asks the rest. The session runs under what was chosen
 * and is resumed made from it again; the next one is offered the same. An
 * environment still takes another list of the file in place of one of its
 * own, through the API. Before the test below, which renames and deletes
 * lists.
 */
test("a session is made from a runtime and three lists, and comes back made from them", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)
  const dialog = page.getByRole("dialog", { name: "Sessions" })
  const select = name => dialog.locator(`select.${name}`)
  const lists = dialog.locator(".row[data-row='lists']")
  const settings = async () => (await fetch(`${BASE}/api/settings`)).json()
  const say = async text => {
    const done = page.locator(".transcript article.assistant button.meta")
    const before = await done.count()
    await page.locator(".composer input").fill(text)
    await page.click('.composer button[type="submit"]')
    await expect(done).toHaveCount(before + 1)
  }

  // Nothing to start from: where it runs, and one list of each axis, none
  // chosen yet — the empty one.
  await page.click('.chat .acts button[title^="New session"]')
  await expect(dialog.locator("select.preset")).toHaveCount(0)
  await expect(select("runtime")).toHaveValue("host|")
  await expect(select("list-shell").locator("option")).toHaveText(["empty", "tools"])
  await expect(select("list-network").locator("option")).toHaveText(["empty", "off", "registries"])
  for (const kind of ["shell", "network", "files"]) await expect(select(`list-${kind}`)).toHaveValue("")
  await expect(lists).toContainText("runs nothing without asking")
  await expect(lists).toContainText("reads the folder, asks to write")

  await select("list-shell").selectOption("tools")
  await expect(lists).toContainText("allow 2 · deny 1")
  await select("list-network").selectOption("registries")
  await dialog.getByRole("button", { name: "Start session" }).click()
  await expect(dialog).toHaveCount(0)

  // It runs under them, the empty one where none was chosen.
  const made = await settings()
  expect(made.lists.composed).toBe(true)
  expect(made.lists.shell.name).toBe("tools")
  expect(made.lists.network.name).toBe("registries")
  expect(made.lists.network.allow).toEqual(["crates.io"])
  expect(made.lists.files.name).toBe("(empty)")
  await say("hola")

  // The next one is offered what this one was made from; started empty.
  await page.click('.chat .acts button[title^="New session"]')
  await expect(select("list-shell")).toHaveValue("tools")
  await expect(select("list-network")).toHaveValue("registries")
  await expect(select("list-files")).toHaveValue("")
  await select("list-shell").selectOption("")
  await select("list-network").selectOption("")
  await dialog.getByRole("button", { name: "Start session" }).click()
  await expect(dialog).toHaveCount(0)
  expect((await settings()).lists.environment).toBe("empty")

  // Picked from the history, the first is resumed made from its lists again.
  await page.click('button[title="Earlier sessions"]')
  await dialog.locator("ul.sessions li:not(.on) .pick").first().click()
  await expect(dialog).toHaveCount(0)
  await expect.poll(async () => (await settings()).lists.shell.name).toBe("tools")
  expect((await settings()).lists.network.name).toBe("registries")

  // What the server refuses, and says why.
  const refused = async (body, text) => {
    const answer = await fetch(`${BASE}/api/sessions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    })
    expect(answer.status).toBe(400)
    expect(await answer.text()).toContain(text)
  }
  await refused({ lists: { network: "nope" } }, "there is no [network.nope]")
  await refused({ runtime: "docker", lists: {} }, "no posture or environment in config.toml names one")
  await refused({ runtime: "host", image: "luu-worker:dev", lists: {} }, "runs no container")

  // An environment takes another list of the file in place of its own.
  const swapped = await fetch(`${BASE}/api/sessions`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ posture: "asking", lists: { network: "registries" } }),
  })
  expect(swapped.ok).toBe(true)
  const there = await settings()
  expect(there.lists.environment).toBe("asking")
  expect(there.lists.composed ?? false).toBe(false)
  expect(there.lists.network.name).toBe("registries")

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

  // Two zones: a path is only ever allowed — one denied inside the folder a
  // session reads could not be held for a command yet.
  const tree = pane.locator(".kind[data-kind='files'] .list[data-list='tree']")
  await expect(tree.locator(".zone")).toHaveCount(1)
  await expect(zone(tree, "allow")).toBeVisible()

  // A write the loader refuses — every program allowed, which a sandbox
  // exists to stop — is not written, and says why.
  await zone(rust, "allow").locator("input").fill("*")
  await zone(rust, "allow").locator("input").press("Enter")
  const before = file()
  await save.click()
  await expect(pane.locator(".section-actions .error")).toContainText("write the programs")
  expect(file()).toBe(before)
  await pane.getByRole("button", { name: "Discard" }).click()
  await expect(zone(rust, "allow").locator(".entry")).toHaveText(["ls×", "git×", "rg×"])
  await expect(pane.locator(".section-actions .error")).toHaveCount(0)

  // The refusal asked for above, and nothing else: a browser logs every
  // response that is not a success.
  expect(errors).toEqual([
    "console: Failed to load resource: the server responded with a status of 422 (Unprocessable Entity)",
  ])
})
