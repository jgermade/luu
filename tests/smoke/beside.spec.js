// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { chmodSync, closeSync, existsSync, mkdirSync, mkdtempSync, openSync, rmSync, writeFileSync, writeSync, ftruncateSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * A second local model is said before the click: the new session's dialog
 * names what is loaded, what this one would load, and the weights together.
 *
 * No real engine: `llama-server` here is a script that sleeps, which is
 * enough, because an engine holds its model for as long as its process lives
 * (`Supervisor::model_of`), and the models are sparse files that start with
 * `GGUF` and take no disk. See
 * `RECORD/2026-11-09.a-second-local-model-warned.completed.md` and row 73 of
 * `ROADMAP/2026-11-08`.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7911
const BASE = `http://127.0.0.1:${PORT}`
const GB = 1e9

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

/** A GGUF by its magic, as long as asked, and sparse. */
function gguf(path, bytes) {
  const fd = openSync(path, "w")
  writeSync(fd, "GGUF")
  ftruncateSync(fd, bytes)
  closeSync(fd)
}

/** @type {import("node:child_process").ChildProcess | null} */
let server = null
/** @type {string} */
let home

test.beforeAll(async () => {
  home = mkdtempSync(join(tmpdir(), "luu-beside-"))
  mkdirSync(join(home, "bin"))
  mkdirSync(join(home, "models"))
  const fake = join(home, "bin", "llama-server")
  writeFileSync(fake, "#!/bin/sh\nexec sleep 600\n")
  chmodSync(fake, 0o755)
  gguf(join(home, "models", "first.gguf"), 4 * GB)
  gguf(join(home, "models", "second.gguf"), 5 * GB)
  const engine = (name, port) =>
    `[engine.${name}]\nkind = "llama"\nbinary = "${fake}"\nport = ${port}\nmodel = "luu:${name}.gguf"\n\n`
  const provider = (name, engine, port) =>
    `[provider.${name}]\nbackend = "openai"\nurl = "http://127.0.0.1:${port}/v1"\n` +
    `model = "luu:${engine}.gguf"\nengine = "${engine}"\n\n`
  writeFileSync(
    join(home, "config.toml"),
    '[provider.here]\nbackend = "mock"\nmodel = "mock"\n\n' +
      engine("first", 18101) + engine("second", 18102) +
      provider("near", "first", 18101) + provider("far", "second", 18102),
  )
  server = spawn(
    binary(),
    ["serve", "--bind", `127.0.0.1:${PORT}`, "--no-store", "--mock-delay-ms", "0"],
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
  const started = await fetch(`${BASE}/api/engines/first/start`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: "{}",
  })
  expect(started.ok, await started.clone().text()).toBe(true)
})

test.afterAll(async () => {
  await fetch(`${BASE}/api/engines/first/stop`, {
    method: "POST", headers: { "content-type": "application/json" }, body: "{}",
  }).catch(() => {})
  server?.kill("SIGTERM")
  rmSync(home, { recursive: true, force: true })
})

const beside = async query => (await fetch(`${BASE}/api/engines/beside?${new URLSearchParams(query)}`)).json()

test("the server says what a session would load, and what is loaded", async () => {
  const far = await beside({ provider: "far" })
  expect(far.loads).toMatchObject({ engine: "second", model: "luu:second.gguf", size: 5 * GB })
  expect(far.loaded).toEqual([
    expect.objectContaining({ engine: "first", model: "luu:first.gguf", size: 4 * GB, sessions: 0 }),
  ])
  // The same engine on the same model: what is loaded is what it needs.
  const near = await beside({ provider: "near" })
  expect(near.loads).toMatchObject({ engine: "first", model: "luu:first.gguf" })
  // A provider with no engine loads nothing.
  expect((await beside({ provider: "here" })).loads).toBeNull()
})

test("the new session's dialog warns before a second local model", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  const picker = page.locator("dialog.modal").first()
  await picker.waitFor({ state: "visible", timeout: 5_000 }).catch(() => {})
  if (await picker.isVisible()) await picker.locator('button:has-text("Use this folder")').click()

  await page.click('.chat .acts button[title*="New session"]')
  const starter = page.getByRole("dialog", { name: "Sessions" })
  await expect(starter).toBeVisible()
  const sends = starter.locator(".sends")
  const warning = starter.locator("p.loads")
  // On the mock: nothing local, nothing said.
  await sends.locator(".prov strong", { hasText: /^here$/ }).click()
  await sends.locator(".models button.model").first().click()
  await expect(sends.locator(".sends-head")).toContainText("here ·")
  await expect(warning).toHaveCount(0)

  await sends.locator(".prov strong", { hasText: /^far$/ }).click()
  await sends.locator(".models button.model").first().click()
  await expect(sends.locator(".sends-head")).toContainText("far ·")
  await expect(warning).toContainText("Already loaded: first (luu) (4.0 GB) on first")
  await expect(warning).toContainText("loads second (luu) (5.0 GB) beside it")
  await expect(warning).toContainText("about 9.0 GB of weights together")

  // The engine already on it: shared, and nothing to say.
  await sends.locator(".prov strong", { hasText: /^near$/ }).click()
  await sends.locator(".models button.model").first().click()
  await expect(sends.locator(".sends-head")).toContainText("near ·")
  await expect(warning).toHaveCount(0)
})
