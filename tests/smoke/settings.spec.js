// @ts-check
import { expect, test } from "@playwright/test"
import { spawn } from "node:child_process"
import { createServer } from "node:http"
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

/**
 * Settings → Resend, driven from the browser against a live `luu serve`.
 *
 * The three rules that decide how much of the history a turn pays for again
 * were built, measured, tested and reachable only by typing a flag and
 * restarting the server. This is the surface that changes that, so this is the
 * test that opens it and clicks it — for the reason `gate.spec.js` exists at
 * all: two bugs in two days were found by a person opening this page and none
 * by a test. See
 * `RECORD/2026-09-18.the-window-rules-are-a-session-fact.completed.md` part 4.
 *
 * What it is really here for is the half a unit test cannot reach: **what the
 * page says after a save that did only part of what it was asked.** Turning
 * rule B off does not move a running session, and the person who clicked it has
 * to be told — by the server, reported by the page, and visible on screen.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const PORT = 7896
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
/** @type {string} */
let home

test.beforeAll(async () => {
  // Its own state directory: this suite *writes* `config.toml`, so running it
  // against the config of the person running it would edit their machine.
  home = mkdtempSync(join(tmpdir(), "luu-settings-"))
  writeFileSync(
    join(home, "config.toml"),
    '[provider.here]\nbackend = "mock"\nmodel = "mock"\n',
  )
  // Three models, one per kind of store the catalog reads, and an MLX one
  // llama.cpp cannot load. Four bytes of GGUF magic is all a listing reads.
  const manifest = (dir, layers) => {
    mkdirSync(dirname(dir), { recursive: true })
    writeFileSync(dir, JSON.stringify({ layers }))
  }
  const ollama = join(home, "stores/ollama")
  manifest(join(ollama, "manifests/registry.ollama.ai/library/tiny/1b"), [
    { mediaType: "application/vnd.ollama.image.model", digest: "sha256:aaa", size: 4 },
  ])
  manifest(join(ollama, "manifests/registry.ollama.ai/library/big/27b-mlx"), [
    { mediaType: "application/vnd.ollama.image.tensor", digest: "sha256:bbb", size: 4 },
  ])
  mkdirSync(join(ollama, "blobs"), { recursive: true })
  writeFileSync(join(ollama, "blobs/sha256-aaa"), "GGUF")
  const snapshot = join(home, "stores/hub/models--org--small-GGUF/snapshots/abc")
  mkdirSync(snapshot, { recursive: true })
  writeFileSync(join(snapshot, "small-Q4_K_M.gguf"), "GGUF")

  // A stand-in for Homebrew, first on the server's PATH, so installing a
  // runtime from Settings is exercised without installing one on the machine
  // that runs the suite. Its `install podman` leaves a `podman` beside it that
  // answers `info` and has no images.
  const brewBin = join(home, "brew-bin")
  mkdirSync(brewBin, { recursive: true })
  writeFileSync(
    join(brewBin, "brew"),
    '#!/bin/sh\necho "==> Installing $2 (stand-in)"\n' +
      'printf \'#!/bin/sh\\ncase "$1" in info) exit 0;; *) exit 1;; esac\\n\' > "$(dirname "$0")/$2"\n' +
      'chmod +x "$(dirname "$0")/$2"\n',
  )
  chmodSync(join(brewBin, "brew"), 0o755)
  // And no podman but the one it installs. The Runtimes test needs a machine
  // without one, and luu finds runtimes on PATH alone, so every directory
  // holding a real one is left out rather than the machine assumed bare — it
  // stopped being bare the evening somebody installed podman on it.
  const path = (process.env.PATH || "")
    .split(":")
    .filter(dir => dir && !existsSync(join(dir, "podman")))
    .join(":")

  server = spawn(
    binary(),
    [
      "serve",
      "--bind", `127.0.0.1:${PORT}`,
      "--mock-delay-ms", "0",
    ],
    {
      cwd: root,
      // The model stores too, each where the tool's own variable points: the
      // catalog would otherwise list the models of whoever runs the suite.
      env: {
        ...process.env,
        LUU_HOME: home,
        PATH: `${brewBin}:${path}`,
        OLLAMA_MODELS: join(home, "stores/ollama"),
        HF_HUB_CACHE: join(home, "stores/hub"),
        LLAMA_CACHE: join(home, "stores/llama.cpp"),
      },
      stdio: "pipe",
    },
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

/**
 * A stand-in `/v1/models` on a port of its own, refusing every key but `key`
 * when there is one — the way a hosted API does, so what the page shows of a
 * refusal is the server's own. Closed by the caller.
 */
async function fakeOpenAi(models, key = null) {
  const fake = createServer((req, res) => {
    if (req.url !== "/v1/models") return res.writeHead(404).end()
    if (key && req.headers.authorization !== `Bearer ${key}`) {
      return res.writeHead(401, { "content-type": "application/json" }).end('{"error":"invalid api key"}')
    }
    res.writeHead(200, { "content-type": "application/json" })
    res.end(JSON.stringify({ data: models.map(id => ({ id })) }))
  })
  await new Promise(done => fake.listen(0, "127.0.0.1", done))
  const at = `http://127.0.0.1:${/** @type {import("node:net").AddressInfo} */ (fake.address()).port}/v1`
  return { at, close: () => fake.close() }
}

async function chooseFolder(page) {
  const picker = page.locator("dialog.modal").first()
  await picker.waitFor({ state: "visible", timeout: 5_000 }).catch(() => {})
  if (await picker.isVisible()) {
    await picker.locator('button:has-text("Use this folder")').click()
    await expect(picker).toBeHidden()
  }
}

/**
 * Settings → General names two directories: the one `serve` was started in,
 * as a path, and luu's own, which is the one every path of luu's is written
 * under as `:luu`.
 */
test("General names the working dir and the config dir, and only the second is :luu", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("General")')
  const general = page.locator(".general-pane dl.settings")
  const row = name => general.locator(`dt:text-is("${name}") + dd`)
  await expect(row("Working dir")).not.toContainText("(:")
  await expect(row("Config dir")).toContainText(home)
  await expect(row("Config dir")).toContainText("(:luu)")
  await page.locator(".modal-head button.close", { hasText: "close" }).click()
})

test("the resend rules are chosen from the page, and a save says what it did not do", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await expect(page.locator(".col.inspector .logo")).toHaveAccessibleName("luu")
  await chooseFolder(page)

  // The third section, which the modal's own comment said its shape made free.
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Sessions")')
  // Resend and Authority are one section now, in this order, and the rail
  // puts Engines before Models; Appearance is General's other half since
  // 2026-10-21.
  await expect(page.locator(".modal .rail button")).toHaveText(["General", "Appearance", "Engines", "Models", "Runtimes", "Sessions"])
  await expect(page.locator("#sessions-resend h2")).toHaveText("Resend")
  await expect(page.locator("#sessions-authority h2")).toHaveText("Authority")

  const running = page.locator("#sessions-resend .settings").first()
  await expect(running).toBeVisible()
  // What the server was started under with no flags: rule A on since
  // 2026-09-19 and the other two off, because A is the only one of the three
  // whose saving has been measured against a model.
  await expect(running.locator("dd").nth(0)).toContainText("once")
  await expect(running.locator("dd").nth(1)).toContainText("never")
  await expect(running.locator("dd").nth(2)).toContainText("kept")

  // The editor below it: this machine's default, which says nothing yet. Unset
  // is not off — it is the code's default, which for this rule is now `once`.
  const editor = page.locator("#sessions-resend .settings").nth(1)
  await expect(editor.locator(".seg").first().locator("button.on")).toHaveText("unset")

  // Rule A off, which since the flip is the direction that is a change. It
  // stores nothing, so it is the one rule that moves a running session in both
  // directions, and this is the half that used to be unreachable.
  await editor.locator('button:has-text("always")').click()
  await expect(page.locator("#sessions-resend button.save .count")).toHaveText("1")
  await page.locator("#sessions-resend button.save").click()
  // A save that landed says so on the button, until the next change.
  await expect(page.locator("#sessions-resend button.save.saved")).toBeVisible()

  await expect(running.locator("dd").nth(0)).toContainText("always")
  const off = await (await fetch(`${BASE}/api/resend`)).json()
  expect(off.file.repeat).toBe("always")
  expect(off.running.repeat).toBe("always")

  // And back on, which is the other direction and the one the default takes.
  await editor.locator('button:has-text("once")').click()
  await page.locator("#sessions-resend button.save").click()

  await expect(running.locator("dd").nth(0)).toContainText("once")
  const written = await (await fetch(`${BASE}/api/resend`)).json()
  expect(written.file.repeat).toBe("once")
  expect(written.running.repeat).toBe("once")
  // And in the file, beside the provider it must not have deleted.
  const onDisk = readFileSync(join(home, "config.toml"), "utf8")
  expect(onDisk).toContain("[resend]")
  expect(onDisk).toContain("[provider.here]")

  // Rule B on, which is clean: the prune line starts moving.
  await editor.locator('button:has-text("behind")').click()
  await page.locator("#sessions-resend button.save").click()
  await expect(running.locator("dd").nth(1)).toContainText("behind")

  // And rule B off again, which is the case this whole spec is for. The line is
  // a ratchet, so the running session keeps pruning; the file says never, and
  // the page has to say both rather than reporting a change that did not
  // happen.
  await editor.locator('button:has-text("never")').click()
  await page.locator("#sessions-resend button.save").click()

  const waiting = page.locator("#sessions-resend .waiting")
  await expect(waiting).toBeVisible()
  await expect(waiting).toContainText("ratchet")
  // The file moved and the session did not, and the panel above says so on the
  // row it is about.
  await expect(running.locator("dd").nth(1)).toContainText("behind")
  await expect(running.locator("dd").nth(1)).toContainText("the file says never")

  // Rule C is three values, not two, and the middle one is the point of part 5:
  // the 32 752 tokens it was justified by came entirely from `read_file` calls
  // and from no command output at all. Matched on an exact label, because
  // `has-text` is a substring and "cited" is one of "cited_reads".
  await editor.locator("button", { hasText: /^cited_reads$/ }).click()
  await page.locator("#sessions-resend button.save").click()

  await expect(running.locator("dd").nth(2)).toContainText("cited_reads")
  const cited = await (await fetch(`${BASE}/api/resend`)).json()
  expect(cited.file.results).toBe("cited_reads")
  expect(cited.running.results).toBe("cited_reads")

  // And stepping back down from it waits, for the same reason turning prune off
  // does: what it hands back has to fit somewhere, and what pays is the floor.
  await editor.locator("button", { hasText: /^kept$/ }).click()
  await page.locator("#sessions-resend button.save").click()
  await expect(page.locator("#sessions-resend .waiting")).toContainText("results:")
  await expect(running.locator("dd").nth(2)).toContainText("cited_reads")

  await page.locator(".modal-head button.close", { hasText: "close" }).click()
  await expect(page.locator("dialog.modal")).toHaveCount(0)

  expect(errors).toEqual([])
})

/**
 * Settings → Authority, [`the resend rules are chosen from the page`]'s reason
 * one section along: a person opening a page and clicking it has found bugs
 * a unit test did not. See
 * `RECORD/2026-09-22.an-authority-a-model-is-told.completed.md`.
 *
 * No `waiting` message to test here — that is the point of §"Applied live in
 * full" in `put_authority`'s own doc comment: a note has no ratchet a save
 * could be unsafe to move, so what is saved is what runs.
 */
test("an authority note is written from the page and reaches the live session", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await expect(page.locator(".col.inspector .logo")).toHaveAccessibleName("luu")
  await chooseFolder(page)

  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Sessions")')

  const running = page.locator("#sessions-authority .settings").first()
  await expect(running).toBeVisible()
  // Nothing set yet, on either authority.
  await expect(running.locator("dd").nth(0)).toContainText("nothing sent")
  await expect(running.locator("dd").nth(1)).toContainText("nothing sent")

  const editor = page.locator("#sessions-authority .settings").nth(1)
  await editor
    .locator("textarea")
    .first()
    .fill("You can read, but writes are refused until a plan is approved.")
  await page.locator("#sessions-authority button.save").click()

  await expect(running.locator("dd").nth(0)).toContainText(
    "You can read, but writes are refused",
  )
  // `system` is the default and is never sent as the empty string the
  // "unset" button writes for a rule — it renders as the word itself once a
  // note exists to have a position at all.
  await expect(running.locator("dd").nth(0)).toContainText("system")

  const written = await (await fetch(`${BASE}/api/authority`)).json()
  expect(written.file.draft.text).toContain("writes are refused")
  expect(written.running.draft.text).toContain("writes are refused")
  expect(written.running.draft.position).toBe("system")
  // The plan table is untouched.
  expect(written.file.plan).toBeUndefined()
  expect(written.running.plan).toBeNull()
  const onDisk = readFileSync(join(home, "config.toml"), "utf8")
  expect(onDisk).toContain("[authority.draft]")
  expect(onDisk).toContain("[provider.here]")

  // `prompt`, which repeats the note every turn instead of riding the cached
  // prefix once.
  await editor.locator('button:has-text("prompt")').first().click()
  await page.locator("#sessions-authority button.save").click()
  await expect(running.locator("dd").nth(0)).toContainText("prompt")
  const moved = await (await fetch(`${BASE}/api/authority`)).json()
  expect(moved.running.draft.position).toBe("prompt")

  await page.locator(".modal-head button.close", { hasText: "close" }).click()
  await expect(page.locator("dialog.modal")).toHaveCount(0)

  expect(errors).toEqual([])
})

/**
 * Settings → General → Files: an icon theme imported with the browser's folder
 * picker, written under `$LUU_HOME/icon-themes/`, chosen, and drawn by the tree
 * without a restart. See
 * `RECORD/2026-09-28.an-icon-theme-from-settings.completed.md`.
 */
test("an icon theme is imported from a picked folder and drawn at once", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  // A theme as small as one can be: an extension folder with one icon.
  const picked = join(mkdtempSync(join(tmpdir(), "luu-theme-")), "tiny-icons")
  mkdirSync(join(picked, "icons"), { recursive: true })
  writeFileSync(join(picked, "icons", "rust.svg"), '<svg xmlns="http://www.w3.org/2000/svg"/>')
  writeFileSync(
    join(picked, "theme.json"),
    JSON.stringify({
      iconDefinitions: { _rust: { iconPath: "./icons/rust.svg" } },
      fileExtensions: { rs: "_rust", toml: "_rust", md: "_rust" },
      file: "_rust",
    }),
  )
  writeFileSync(
    join(picked, "package.json"),
    JSON.stringify({
      name: "tiny-icons",
      contributes: { iconThemes: [{ id: "tiny", label: "Tiny Icons", path: "./theme.json" }] },
    }),
  )

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)
  await page.click('.inspector button:has-text("Files")')
  await expect(page.locator(".inspector img")).toHaveCount(0)

  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Appearance")')
  await page.locator(".modal input.pick-folder").setInputFiles(picked)

  // Chosen, under *Installed in :luu*, and drawn in the preview with its own icons.
  const row = page.getByRole("radio", { name: /Tiny Icons/ })
  await expect(row).toHaveAttribute("aria-checked", "true")
  const preview = page.locator(".modal .theme-preview")
  await expect(preview).toContainText("Tiny Icons")
  await expect(preview.locator(".ds-badge")).not.toHaveClass(/unseen/)
  await expect(preview.locator(".big-tree img").first()).toHaveAttribute("src", /^data:image\/svg\+xml;base64,/)
  // The tree's own shapes, previewed under the pointer, without choosing them.
  await page.getByRole("radio", { name: /The tree's own/ }).hover()
  await expect(preview).toContainText("The tree's own")
  await expect(preview.locator(".big-tree img")).toHaveCount(0)
  // Not in use, so its badge is held but not shown — the head does not move.
  await expect(preview.locator(".ds-badge")).toHaveClass(/unseen/)
  await expect(row).toHaveAttribute("aria-checked", "true")
  const onDisk = readFileSync(join(home, "config.toml"), "utf8")
  expect(onDisk).toContain("icon-themes/tiny-icons")
  expect(onDisk).toContain("[provider.here]")

  await page.locator(".modal-head button.close", { hasText: "close" }).click()
  // Drawn by the tree at once, under the theme's new revision.
  await expect(page.locator(".inspector img").first()).toHaveAttribute("src", /\?r=1$/)

  rmSync(dirname(picked), { recursive: true, force: true })
  expect(errors).toEqual([])
})

/**
 * Settings → Models: the built-in mock is a row and shows as chosen when the
 * server fell back to it, and a provider is added through a form of its own
 * rather than an empty row in the table.
 */
test("the mock a server fell back to is chosen, and a provider is added in a modal", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await expect(page.locator(".col.inspector .logo")).toHaveAccessibleName("luu")
  await chooseFolder(page)

  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Models")')

  // No default in the file: the mock is what this server runs, and the table
  // says so instead of showing nothing chosen.
  const builtin = page.locator(".modal ul.providers li.builtin")
  await expect(builtin.locator(".tag.default")).toHaveCount(1)
  await expect(builtin.locator(".tag:not(.default)")).toHaveText("running")

  await page.locator('.modal button.add:has-text("+ provider")').click()
  const form = page.getByRole("dialog", { name: "Add a provider" })
  await expect(form).toBeVisible()
  // A child of Settings: no head of its own, hung from the bottom of Settings'
  // head, and ESC closes it and nothing under it.
  await expect(form.locator(".modal-head")).toHaveCount(0)
  await expect(async () => {
    const head = await page.locator("dialog.modal:not(.child) > .modal-head").boundingBox()
    const box = await form.boundingBox()
    expect(Math.abs(box.y - (head.y + head.height))).toBeLessThan(1)
  }).toPass()
  await page.keyboard.press("Escape")
  await expect(form).toHaveCount(0)
  await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible()
  // And the parent's head still works over its child: its close button
  // closes both, where a second modal would have made it inert.
  await page.locator('.modal button.add:has-text("+ provider")').click()
  await expect(form).toBeVisible()
  // The rest of Settings is inert under it, the side menu included.
  await expect.poll(() => page.locator(".modal .rail").first().evaluate(el => !!el.closest("[inert]"))).toBe(true)
  await page.getByRole("dialog", { name: "Settings" }).locator(".modal-head button.close").click()
  await expect(page.locator("dialog.modal")).toHaveCount(0)
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Models")')
  await page.locator('.modal button.add:has-text("+ provider")').click()
  // A name the table already has is refused before it is sent.
  await form.locator("dd input").first().fill("here")
  await expect(form.locator(".warn")).toContainText("already a provider")
  await expect(form.getByRole("button", { name: "Add provider" })).toBeDisabled()
  // No *Default* in it: a provider is made the default from the list.
  await expect(form.getByText("what every run that names no provider")).toHaveCount(0)

  // Connected first: the models are chosen from what it serves, not typed —
  // on a tab of their own, shut until the server has answered.
  const server = await fakeOpenAi(["tiny", "tiny-q4"])
  await form.locator("dd input").first().fill("local")
  await form.locator("select.backend").selectOption("openai")
  await form.locator('input[placeholder="http://127.0.0.1:8080/v1"]').fill(server.at)
  await expect(form.getByRole("tab", { name: /Models/ })).toBeDisabled()
  await form.getByRole("button", { name: "Connect" }).click()
  await expect(form.getByRole("tab", { name: /Models/ })).toBeEnabled()
  await expect(form.getByRole("tab", { name: /Models/ })).toContainText("2")
  await form.getByRole("tab", { name: /Models/ }).click()
  await form.locator('label.pick:has-text("tiny") input').first().click()
  await form.getByRole("button", { name: "Add provider" }).click()
  await expect(form).toHaveCount(0)

  // Written on its own, and in the list without a second save.
  const onDisk = readFileSync(join(home, "config.toml"), "utf8")
  expect(onDisk).toContain("[provider.local]")
  expect(onDisk).toContain(`url = "${server.at}"`)
  expect(onDisk).toContain('model = "tiny"')
  expect(onDisk).toContain("[provider.here]")
  const names = page.locator(".modal ul.providers li.profile .name")
  await expect(names).toHaveText(["here", "local"])
  // The list is read-only: no inputs in it, and nothing to save under it.
  await expect(page.locator(".modal ul.providers input")).toHaveCount(0)
  await expect(page.locator(".modal .section.on button.save")).toHaveCount(0)
  // Not asked to be the default, so the mock still is.
  await expect(builtin.locator(".tag.default")).toHaveCount(1)

  // Edited in its own form, which opens on what the file has and asks the
  // provider what it serves: its Models tab is open on the answer.
  const row = name => page.locator(".modal ul.providers li.profile", { has: page.locator(`.name:text-is("${name}")`) })
  await row("local").locator("button.edit").click()
  const edit = page.getByRole("dialog", { name: "Provider local" })
  await edit.getByRole("tab", { name: /Models/ }).click()
  const pick = name => edit.locator("li.model", { has: page.locator(`.mname:text-is("${name}")`) }).locator('input[type="radio"]')
  await expect(pick("tiny")).toBeChecked()
  await pick("tiny-q4").click()
  await edit.getByRole("button", { name: "Save" }).click()
  await expect(edit).toHaveCount(0)
  server.close()
  expect(readFileSync(join(home, "config.toml"), "utf8")).toContain('model = "tiny-q4"')
  await expect(row("local")).toContainText("tiny-q4")

  // Renamed, and the old name is gone rather than left beside it; then
  // removed, asked twice.
  await page.locator('.modal button.add:has-text("+ provider")').click()
  const scratch = page.getByRole("dialog", { name: "Add a provider" })
  await scratch.locator("dd input").first().fill("scratch")
  await scratch.locator("select.backend").selectOption("mock")
  await scratch.getByRole("button", { name: "Add provider" }).click()
  await expect(scratch).toHaveCount(0)
  await row("scratch").locator("button.edit").click()
  const renaming = page.getByRole("dialog", { name: "Provider scratch" })
  await renaming.locator("dd input").first().fill("scratch2")
  await renaming.getByRole("button", { name: "Save" }).click()
  await expect(names).toHaveText(["here", "local", "scratch2"])
  await row("scratch2").locator("button.edit").click()
  const removing = page.getByRole("dialog", { name: "Provider scratch2" })
  await removing.getByRole("button", { name: "Remove", exact: true }).click()
  await removing.getByRole("button", { name: "Remove scratch2?" }).click()
  await expect(removing).toHaveCount(0)
  await expect(names).toHaveText(["here", "local"])
  expect(readFileSync(join(home, "config.toml"), "utf8")).not.toContain("scratch")

  await page.locator(".modal-head button.close", { hasText: "close" }).click()
  await expect(page.locator("dialog.modal")).toHaveCount(0)

  expect(errors).toEqual([])
})

/**
 * Settings → Models: **Connect** asks the destination as typed, before
 * anything about it is saved, and a wrong key comes back as the server's own
 * refusal, at the key. See
 * `RECORD/2026-10-21.connecting-a-provider.completed.md`.
 */
test("a destination is connected to before it is saved, and connecting writes nothing", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  const server = await fakeOpenAi(["abab-7", "abab-8"], "sk-right")
  const before = readFileSync(join(home, "config.toml"), "utf8")

  try {
    await page.setViewportSize({ width: 1440, height: 900 })
    await page.goto(`${BASE}/index.html`)
    await chooseFolder(page)
    await page.click('.inspector .col-foot button[title="Settings"]')
    await page.click('.modal .rail button:has-text("Models")')
    await page.locator('.modal button.add:has-text("+ provider")').click()
    const form = page.getByRole("dialog", { name: "Add a provider" })
    await form.locator("dd input").first().fill("hosted")
    await form.locator("select.backend").selectOption("openai")
    await form.locator('input[placeholder="http://127.0.0.1:8080/v1"]').fill(server.at)

    // The wrong key: the server's 401, the form still open, the key in focus.
    const key = form.locator('input[type="password"]')
    // No *Key file*: luu places the key's file itself.
    await expect(form.getByText("Key file", { exact: true })).toHaveCount(0)
    await key.fill("sk-wrong")
    await form.getByRole("button", { name: "Connect" }).click()
    await expect(form.locator("p.error")).toContainText("401")
    await expect(form.getByRole("tab", { name: /Models/ })).toBeDisabled()
    await expect(key).toBeFocused()

    // The right one: the Models tab opens with what it serves, and nothing
    // is chosen for them. The connection stays where it was, unfolded.
    await key.fill("sk-right")
    await form.getByRole("button", { name: "Connect" }).click()
    const models = form.getByRole("tab", { name: /Models/ })
    await expect(models).toBeEnabled()
    await expect(models).toContainText("2")
    await expect(form.locator('input[placeholder="http://127.0.0.1:8080/v1"]')).toBeVisible()
    await models.click()
    await expect(form.locator("li.model .mname")).toHaveText(["abab-7", "abab-8"])
    await expect(form.locator('li.model input[type="radio"]:checked')).toHaveCount(0)
    // Nothing said about tools by this server: one group, *not said*.
    await expect(form.locator("li.head")).toHaveText(["Not said"])

    // A field the answer was asked with changes: the answer goes, and the
    // Models tab shuts with it.
    await form.getByRole("tab", { name: "Connection" }).click()
    await form.locator('input[placeholder="http://127.0.0.1:8080/v1"]').fill(`${server.at}/`)
    await expect(models).toBeDisabled()
    await expect(form.locator("li.model")).toHaveCount(0)
    // Cancel on the left, alone; Connect beside the save, a step back from it.
    const foot = form.locator("footer button")
    await expect(foot).toHaveText(["Cancel", "Connect", "Add provider"])
    const [cancel, connect] = await Promise.all([
      form.getByRole("button", { name: "Cancel" }).boundingBox(),
      form.getByRole("button", { name: "Connect" }).boundingBox(),
    ])
    expect((connect?.x ?? 0) - ((cancel?.x ?? 0) + (cancel?.width ?? 0))).toBeGreaterThan(100)

    // Two connections, and the file is as it was.
    expect(readFileSync(join(home, "config.toml"), "utf8")).toBe(before)
    await form.locator("button", { hasText: "Cancel" }).click()
    await expect(form).toHaveCount(0)
    await page.locator(".modal-head button.close", { hasText: "close" }).click()
  } finally {
    server.close()
  }

  expect(errors).toEqual([])
})

/**
 * The models half: grouped by what the server says of their tools, a window
 * each that may not pass what the server says it holds, and the file written
 * as exceptions — the one that differs, the one left out — and nothing for
 * the rest. Against a stand-in Ollama whose `/api/show` answers per model.
 */
test("the models are grouped by their tools and written as exceptions", async ({ page }) => {
  const SHOW = {
    "coder:7b": { capabilities: ["completion", "tools"], model_info: { "qwen2.context_length": 32768 } },
    "old:3b": { capabilities: ["completion"], model_info: { "llama.context_length": 8192 } },
    mystery: { model_info: {} },
  }
  const fake = createServer((req, res) => {
    let body = ""
    req.on("data", chunk => (body += chunk))
    req.on("end", () => {
      res.setHeader("content-type", "application/json")
      if (req.url === "/api/tags") return res.end(JSON.stringify({ models: Object.keys(SHOW).map(name => ({ name })) }))
      if (req.url === "/api/show") return res.end(JSON.stringify(SHOW[JSON.parse(body).model] || {}))
      res.writeHead(404).end()
    })
  })
  await new Promise(done => fake.listen(0, "127.0.0.1", done))
  const at = `http://127.0.0.1:${/** @type {import("node:net").AddressInfo} */ (fake.address()).port}`

  try {
    await page.setViewportSize({ width: 1440, height: 1000 })
    await page.goto(`${BASE}/index.html`)
    await chooseFolder(page)
    await page.click('.inspector .col-foot button[title="Settings"]')
    await page.click('.modal .rail button:has-text("Models")')
    await page.locator('.modal button.add:has-text("+ provider")').click()
    const form = page.getByRole("dialog", { name: "Add a provider" })
    await form.locator("dd input").first().fill("grouped")
    await form.locator('input[placeholder="http://127.0.0.1:11434"]').fill(at)
    await form.getByRole("button", { name: "Connect" }).click()
    await form.getByRole("tab", { name: /Models/ }).click()

    // Three groups, in this order, and *not said* is not *no*.
    await expect(form.locator("li.head")).toHaveText(["Tools", "Not said", "No tools"])
    const row = name => form.locator("li.model", { has: page.locator(`.mname:text-is("${name}")`) })
    await expect(row("coder:7b").getByRole("radiogroup")).toHaveCount(0)
    const calls = row("old:3b").getByRole("radiogroup", { name: "Tool calls of old:3b" })
    await expect(calls.getByRole("radio", { name: "fenced" })).toHaveAttribute("aria-checked", "true")
    // A choice by hand is the model's own, and says what it costs.
    await calls.getByRole("radio", { name: "native" }).click()
    await expect(calls.getByRole("radio", { name: "native" })).toHaveAttribute("aria-checked", "true")
    await expect(row("old:3b")).toContainText("it will likely refuse the turn")
    await calls.getByRole("radio", { name: "fenced" }).click()
    await expect(row("old:3b")).toContainText("up to 8192")

    // Past what the model holds is refused before it is sent.
    const add = form.getByRole("button", { name: "Add provider" })
    await row("old:3b").locator("input.win").fill("9000")
    await expect(add).toBeDisabled()
    await row("old:3b").locator("input.win").fill("4096")
    await expect(add).toBeEnabled()

    await form.locator(".leave label").click()
    await expect(row("old:3b")).toContainText("left out")
    await add.click()
    await expect(form).toHaveCount(0)

    const written = readFileSync(join(home, "config.toml"), "utf8")
    const table = written.slice(written.indexOf("[provider.grouped]"))
    // Native for the profile, since its models with tools need nothing written.
    expect(table).toContain('tool-calls = "native"')
    expect(table).toContain('[provider.grouped.models."old:3b"]')
    expect(table).toContain("context-limit = 4096")
    expect(table).toContain("enabled = false")
    expect(table).not.toContain('models."coder:7b"')
    expect(table).not.toContain("models.mystery")

    // Left out of the pickers, and still in the form.
    const offered = await (await fetch(`${BASE}/api/providers/grouped/models`)).json()
    expect(offered.models).toEqual(["coder:7b", "mystery"])
    const all = await (await fetch(`${BASE}/api/providers/grouped/models?facts=1`)).json()
    expect(all.models).toHaveLength(3)
    expect(all.facts["old:3b"]).toEqual({ window: 8192, window_is: "trained", tools: false })

    // Removed again, so the tests after this one see the file they expect.
    const now = await (await fetch(`${BASE}/api/providers`)).json()
    delete now.providers.grouped
    const put = await fetch(`${BASE}/api/providers`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ default: now.default ?? null, providers: now.providers }),
    })
    expect(put.ok).toBe(true)
    await page.locator(".modal-head button.close", { hasText: "close" }).click()
  } finally {
    fake.close()
  }
})

/**
 * An Anthropic-compatible provider: the key goes as `x-api-key`, the listing's
 * `max_input_tokens` caps each window, a Claude model is one that takes tools,
 * and what the listing says of the default model — its thinking, its effort,
 * its `max_tokens` — is what the three placeholders say. The Messages API's own
 * keys are written to the profile, a model that takes less than the cap gets
 * its own, and native calls, the backend's default, are not written. See
 * `RECORD/2026-10-22.an-anthropic-compatible-backend.completed.md`.
 */
test("an anthropic provider connects with its key and writes its thinking", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })
  const LEVELS = Object.fromEntries(["low", "medium", "high", "xhigh", "max"].map(l => [l, { supported: true }]))
  const MODELS = [
    {
      id: "claude-opus-5-5", max_input_tokens: 1000000, max_tokens: 128000,
      capabilities: {
        thinking: { supported: true, types: { adaptive: { supported: true }, enabled: { supported: false } } },
        effort: { supported: true, ...LEVELS },
      },
    },
    {
      id: "claude-haiku-4-5", max_input_tokens: 200000, max_tokens: 8192,
      capabilities: {
        thinking: { supported: true, types: { adaptive: { supported: false }, enabled: { supported: true } } },
        effort: { supported: false },
      },
    },
  ]
  const fake = createServer((req, res) => {
    if (!req.url.startsWith("/v1/models")) return res.writeHead(404).end()
    if (req.headers["x-api-key"] !== "sk-ant-right" || !req.headers["anthropic-version"]) {
      return res.writeHead(401, { "content-type": "application/json" })
        .end('{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}')
    }
    res.writeHead(200, { "content-type": "application/json" })
    res.end(JSON.stringify({ data: MODELS, has_more: false }))
  })
  await new Promise(done => fake.listen(0, "127.0.0.1", done))
  const at = `http://127.0.0.1:${/** @type {import("node:net").AddressInfo} */ (fake.address()).port}`

  try {
    await page.setViewportSize({ width: 1440, height: 1000 })
    await page.goto(`${BASE}/index.html`)
    await chooseFolder(page)
    await page.click('.inspector .col-foot button[title="Settings"]')
    await page.click('.modal .rail button:has-text("Models")')
    await page.locator('.modal button.add:has-text("+ provider")').click()
    const form = page.getByRole("dialog", { name: "Add a provider" })
    await form.locator("dd input").first().fill("claude")
    await form.locator("select.backend").selectOption("anthropic")
    await form.locator('input[placeholder="http://127.0.0.1:8080"]').fill(at)

    const key = form.locator('input[type="password"]')
    await key.fill("sk-ant-wrong")
    await form.getByRole("button", { name: "Connect" }).click()
    await expect(form.locator("p.error")).toContainText("401")
    await expect(key).toBeFocused()

    // Before anything is said of a model, the placeholders are luu's own.
    const firstThinking = form.locator("select.thinking option").first()
    await expect(firstThinking).toHaveText("the server's default")
    await expect(form.locator("input.max-tokens")).toHaveAttribute("placeholder", "16384")

    await key.fill("sk-ant-right")
    await form.getByRole("button", { name: "Connect" }).click()
    const modelsTab = form.getByRole("tab", { name: /Models/ })
    await expect(modelsTab).toContainText("2")
    // None chosen, so the first listed answers for the placeholders.
    await expect(firstThinking).toHaveText("the server's default — claude-opus-5-5 takes adaptive")
    await expect(form.locator("select.effort option").first())
      .toHaveText("the server's default — claude-opus-5-5: low · medium · high · xhigh · max")
    await expect(form.locator("input.max-tokens")).toHaveAttribute("placeholder", "16384")
    await expect(form.locator(".max-tokens + .dim")).toContainText("takes up to 128000")

    await modelsTab.click()
    await expect(form.locator("li.head")).toHaveText(["Tools"])
    const row = name => form.locator("li.model", { has: page.locator(`.mname:text-is("${name}")`) })
    await expect(row("claude-haiku-4-5")).toContainText("up to 200000")
    // Haiku as the default: the placeholders follow it.
    await row("claude-haiku-4-5").locator('input[type="radio"]').click()
    await form.getByRole("tab", { name: "Connection" }).click()
    await expect(firstThinking).toHaveText("the server's default — claude-haiku-4-5 takes a budget")
    await expect(form.locator("select.effort option").first()).toHaveText("the server's default — claude-haiku-4-5 takes none")
    await expect(form.locator("input.max-tokens")).toHaveAttribute("placeholder", "8192")

    // A budget that does not fit under a model's cap is refused before it is
    // sent — Haiku's 8192, whatever the profile's.
    const add = form.getByRole("button", { name: "Add provider" })
    await form.locator("select.thinking").selectOption("budget")
    await form.locator("input.budget").fill("9000")
    await expect(add).toBeDisabled()
    await form.locator("select.thinking").selectOption("adaptive")
    await form.locator("select.effort").selectOption("high")
    // Past what the default model takes is refused too.
    await form.locator("input.max-tokens").fill("32000")
    await expect(add).toBeDisabled()
    await modelsTab.click()
    await row("claude-opus-5-5").locator('input[type="radio"]').click()
    await expect(add).toBeEnabled()
    await add.click()
    await expect(form).toHaveCount(0)

    const written = readFileSync(join(home, "config.toml"), "utf8")
    const table = written.slice(written.indexOf("[provider.claude]"))
    expect(table).toContain('backend = "anthropic"')
    expect(table).toContain('model = "claude-opus-5-5"')
    expect(table).toContain('thinking = "adaptive"')
    expect(table).toContain('effort = "high"')
    expect(table).toContain("max-tokens = 32000")
    expect(table).toContain("key-file = ")
    // The model that takes less than the profile's cap gets its own.
    expect(table).toMatch(/\[provider\.claude\.models\."?claude-haiku-4-5"?\]\nmax-tokens = 8192\n/)
    // Native is what an anthropic profile resolves to: nothing to write.
    expect(table).not.toContain("tool-calls")

    const all = await (await fetch(`${BASE}/api/providers/claude/models?facts=1`)).json()
    expect(all.facts["claude-opus-5-5"]).toEqual({
      window: 1000000, window_is: "limit", tools: true,
      max_tokens: 128000, thinking: ["adaptive"], effort: ["low", "medium", "high", "xhigh", "max"],
    })

    const now = await (await fetch(`${BASE}/api/providers`)).json()
    delete now.providers.claude
    const put = await fetch(`${BASE}/api/providers`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ default: now.default ?? null, providers: now.providers }),
    })
    expect(put.ok).toBe(true)
    await page.locator(".modal-head button.close", { hasText: "close" }).click()
  } finally {
    fake.close()
  }

  expect(errors).toEqual([])
})

/**
 * Settings → Engines: a model server luu starts. No download here — the suite
 * does not reach the network — so the binary is a stand-in named
 * `llama-server` that serves HTTP on the port luu hands it, which is enough to
 * see it written, started, heard from and stopped. See
 * `RECORD/2026-09-28.a-model-server-luu-starts.completed.md`.
 */
test("an engine is added with its provider, started, and stopped from Settings", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  // `--host 127.0.0.1 --port <port>` is what luu passes first, so the port is $4.
  const bin = join(home, "bin")
  mkdirSync(bin, { recursive: true })
  const fake = join(bin, "llama-server")
  writeFileSync(fake, '#!/bin/sh\necho "stand-in on $4"\nexec python3 -m http.server --bind 127.0.0.1 "$4"\n')
  chmodSync(fake, 0o755)

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await expect(page.locator(".col.inspector .logo")).toHaveAccessibleName("luu")
  await chooseFolder(page)

  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Engines")')
  await expect(page.locator(".engines-pane")).toContainText("None yet")

  await page.locator('.engines-pane button.add:has-text("+ engine")').click()
  const form = page.getByRole("dialog", { name: "Add an engine" })
  await expect(form).toBeVisible()
  await form.locator("dd input").first().fill("fake")
  await form.locator("select").nth(1).selectOption("custom")

  // Only the kind's own binary, from here.
  const path = form.locator('input[placeholder$="/llama-server"]')
  await path.fill("/bin/sh")
  await expect(form.locator(".warn", { hasText: "must be a file named" })).toBeVisible()
  await expect(form.locator("button.save")).toBeDisabled()
  await path.fill(fake)
  await form.locator('input[placeholder="8080"]').fill("8097")
  await form.locator("button.save").click()
  await expect(form).toHaveCount(0)

  const onDisk = readFileSync(join(home, "config.toml"), "utf8")
  expect(onDisk).toContain("[engine.fake]")
  expect(onDisk).toContain("port = 8097")
  expect(onDisk).toContain("[provider.fake]")
  expect(onDisk).toContain('url = "http://127.0.0.1:8097/v1"')
  expect(onDisk).toContain('engine = "fake"')

  const card = page.locator(".engines-pane section.engine", { hasText: "fake" })
  await expect(card.locator(".state")).toHaveText("stopped")
  await expect(card).toContainText(fake)

  await card.locator('button:has-text("start")').click()
  await expect(card.locator(".state")).toHaveText(/starting|running/)
  await expect(card.locator("pre.log")).toContainText("stand-in on 8097", { timeout: 10_000 })
  await card.locator('button:has-text("stop")').click()
  await expect(card.locator(".state")).toHaveText("exited")
  await expect(card).toContainText("stopped from luu")

  // And Models shows the provider as one that starts an engine.
  await page.click('.modal .rail button:has-text("Models")')
  await expect(page.locator(".modal ul.providers .tag", { hasText: "engine" })).toHaveCount(1)

  await page.locator(".modal-head button.close", { hasText: "close" }).click()
  await expect(page.locator("dialog.modal")).toHaveCount(0)
  expect(errors).toEqual([])
})

/**
 * Changing where a session sends needs no restart: a new `default` is taken up
 * by an empty live session nobody pointed anywhere, and the chat's own picker
 * lists every provider with the models it serves and moves the session there.
 */
test("a new default and the chat's picker both move the session without a restart", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await expect(page.locator(".col.inspector .logo")).toHaveAccessibleName("luu")
  await chooseFolder(page)

  // This server was started with no -p and no default: it follows the file.
  const before = await (await fetch(`${BASE}/api/settings`)).json()
  expect(before.follows_default).toBe(true)
  expect(before.profile).toBeNull()

  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Models")')
  // The default is the one thing changed from the list, and written at once.
  const here = page.locator(".modal ul.providers li.profile", { has: page.locator('.name:text-is("here")') })
  await here.locator('button:has-text("make default")').click()
  await expect(here.locator(".tag.default")).toHaveCount(1)
  await expect(page.locator(".modal")).toContainText("the live session is on here")
  const after = await (await fetch(`${BASE}/api/settings`)).json()
  expect(after.profile).toBe("here")
  expect(after.follows_default).toBe(true)
  await page.locator(".modal-head button.close", { hasText: "close" }).click()
  await expect(page.locator("dialog.modal")).toHaveCount(0)

  // The picker, in two columns: it opens on the session's own provider, with
  // that provider's models beside it; another provider in focus shows its own.
  await page.click(".options .dest")
  const menu = page.locator(".options .menu")
  const models = menu.locator(".models")
  await expect(menu.locator(".prov.on")).toContainText("here")
  await expect(models.locator("button.model.on")).toHaveText(/mock/)
  await menu.locator(".prov", { hasText: "local" }).hover()
  await expect(menu.locator(".prov.on")).toContainText("local")
  await expect(models.locator("button.model")).toHaveText(/tiny/)
  await models.locator("button.model", { hasText: "tiny" }).click()
  await expect(menu).toHaveCount(0)
  await expect(page.locator(".options .dest")).toContainText("local")
  await expect(page.locator(".options .dest")).toContainText("tiny")
  const moved = await (await fetch(`${BASE}/api/settings`)).json()
  expect(moved.profile).toBe("local")
  // Chosen, so it no longer follows the file.
  expect(moved.follows_default).toBe(false)

  expect(errors).toEqual([])
})

/**
 * The models already on the machine, named by where they came from, in the
 * engine form and in the chat's picker for a provider whose engine luu
 * starts. The stores are the suite's own fixtures — see `beforeAll`.
 */
test("the models on this machine are listed by where they came from", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })

  const catalog = await (await fetch(`${BASE}/api/models`)).json()
  expect(catalog.models.map(m => m.label).sort()).toEqual([
    "big:27b-mlx (ollama, mlx)",
    "small-Q4_K_M (huggingface)",
    "tiny:1b (ollama)",
  ])
  expect(catalog.models.find(m => m.label === "tiny:1b (ollama)").reference).toBe("ollama:tiny:1b")
  // And which engine cannot load which, in the server's words.
  const big = catalog.models.find(m => m.label.startsWith("big:27b-mlx"))
  expect(Object.keys(big.cannot).sort()).toEqual(["llama", "mlx"])

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await expect(page.locator(".col.inspector .logo")).toHaveAccessibleName("luu")
  await chooseFolder(page)

  // The engine form offers the two llama.cpp can load, and says why not the third.
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Engines")')
  await page.locator(".engines-pane section.engine", { hasText: "fake" }).locator('button:has-text("edit")').click()
  const form = page.getByRole("dialog", { name: "Engine fake" })
  const models = form.locator("select").nth(2)
  await expect(models.locator("option")).toHaveText([
    "none — named in the arguments below",
    /tiny:1b \(ollama\)/,
    /small-Q4_K_M \(huggingface\)/,
    /⚠ big:27b-mlx \(ollama, mlx\)/,
  ])
  await expect(models.locator("option").last()).toBeDisabled()
  await expect(models.locator("option").last()).toHaveAttribute("title", /only ollama/)
  await expect(form).toContainText("1 on this machine this engine cannot load")
  await models.selectOption("ollama:tiny:1b")
  await form.locator("button.save").click()
  await expect(form).toHaveCount(0)
  expect(readFileSync(join(home, "config.toml"), "utf8")).toContain('model = "ollama:tiny:1b"')
  await expect(page.locator(".engines-pane section.engine", { hasText: "fake" })).toContainText("tiny:1b (ollama)")
  await page.locator(".modal-head button.close", { hasText: "close" }).click()

  // And the chat's picker lists them under the provider that starts it.
  await page.click(".options .dest")
  await page.locator(".options .menu .prov", { hasText: "fake" }).click()
  const group = page.locator(".options .menu .models")
  await expect(group.locator("button.model .name")).toHaveText([
    "tiny:1b (ollama)",
    "small-Q4_K_M (huggingface)",
  ])
  // The one it cannot load is there, greyed, with a warning and why.
  const off = group.locator(".model.off")
  await expect(off).toHaveCount(1)
  await expect(off).toContainText("⚠")
  await expect(off).toContainText("big:27b-mlx (ollama, mlx)")
  await expect(off).toHaveAttribute("aria-disabled", "true")
  await expect(off).toHaveAttribute("title", /llama.cpp loads GGUF only/)
  await page.keyboard.press("Escape")

  expect(errors).toEqual([])
})

/**
 * Who answers the gate, as a dropup: the choice on a button in the chat's
 * foot, and each answer with a line about what it means.
 */
test("the gate's mode is chosen from a dropup", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await expect(page.locator(".col.inspector .logo")).toHaveAccessibleName("luu")
  await chooseFolder(page)

  const face = page.locator(".options .dropup .face")
  await expect(face).toContainText("Manual")
  await face.click()
  const list = page.locator(".options .dropup .list")
  await expect(list.locator(".opt")).toHaveCount(2)
  await expect(list.locator(".opt.on")).toContainText("Every job waits for you")
  await list.locator(".opt", { hasText: "Auto" }).click()
  await expect(list).toHaveCount(0)
  await expect(face).toContainText("Auto")
  // Kept, like every General preference, and put back for the next test.
  expect(await page.evaluate(() => localStorage.getItem("luu.confirm"))).toBe("auto")
  await face.click()
  await page.keyboard.press("Escape")
  await expect(list).toHaveCount(0)
  await face.click()
  await list.locator(".opt", { hasText: "Manual" }).click()
  await expect(face).toContainText("Manual")
})

/**
 * Settings → Runtimes: every container runtime probed, a posture added and
 * removed by naming a policy file out of the list, and the host shell's rule
 * written — all of it into this suite's own `config.toml`. And the one thing
 * this page may not do: build an image no file here names. See
 * `RECORD/2026-10-01.runtimes-in-settings.completed.md`.
 */
test("runtimes, postures and the host shell are configured from Settings", async ({ page }) => {
  const errors = []
  page.on("pageerror", error => errors.push(`uncaught: ${error.message}`))
  page.on("console", message => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`)
  })
  await page.setViewportSize({ width: 1440, height: 1000 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Runtimes")')
  const pane = page.locator(".runtimes-pane")
  await expect(pane.locator(".runtime")).toHaveCount(5, { timeout: 15_000 })
  for (const runtime of ["docker", "podman", "nerdctl", "colima", "container"]) {
    await expect(pane.locator(".runtime header strong", { hasText: new RegExp(`^${runtime}$`) })).toHaveCount(1)
  }
  const config = () => readFileSync(join(home, "config.toml"), "utf8")

  // A posture, named by a policy file out of the list — this checkout's.
  await pane.locator('button.add:has-text("+ posture")').click()
  await pane.locator('.adding input[placeholder="name"]').fill("boxed")
  await pane.locator(".adding select").selectOption("luu.container.toml")
  await pane.locator('.adding button:has-text("add")').click()
  await expect(pane.locator(".postures dt", { hasText: "boxed" })).toHaveCount(1)
  expect(config()).toContain("[posture.boxed]")
  expect(config()).toContain('policy = "luu.container.toml"')
  // And offered at once, without a restart.
  const offered = await (await fetch(`${BASE}/api/postures`)).json()
  expect(Object.keys(offered.postures)).toContain("boxed")
  await pane.locator(".postures .row", { hasText: "boxed" }).locator('button:has-text("remove")').click()
  await expect(pane.locator(".postures dt", { hasText: "boxed" })).toHaveCount(0)
  expect(config()).not.toContain("[posture.boxed]")

  // A path out of the list is refused, from the route itself.
  const typed = await fetch(`${BASE}/api/postures`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ postures: { wide: { policy: "/etc/anything.toml" } } }),
  })
  expect(typed.status).toBe(422)
  // And so is an image no policy file here names.
  const built = await fetch(`${BASE}/api/runtimes/docker/build`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ image: "somebody/else:latest" }),
  })
  expect(built.status).toBe(400)
  expect(await built.text()).toContain("no policy file here names")

  // Installed from here, where Homebrew can: macOS only, and with the
  // stand-in above rather than the real thing.
  if (process.platform === "darwin") {
    const podman = pane.locator(".runtime", { has: page.locator("header strong", { hasText: /^podman$/ }) })
    await expect(podman.locator(".hint code")).toContainText("brew-bin/brew install podman")
    // The three that need an administrator say so, with no button.
    const docker = pane.locator(".runtime", { has: page.locator("header strong", { hasText: /^container$/ }) })
    await expect(docker.locator('button:has-text("install")')).toHaveCount(0)
    await expect(docker).toContainText("not from here: its package needs an administrator")
    await podman.locator('button:has-text("install")').click()
    // When the task ends the runtimes are asked again: installed, answering,
    // and with the image the policy files name not built yet.
    await expect(podman.locator(".state").first()).toHaveText("answering", { timeout: 15_000 })
    await expect(podman.locator(".images")).toContainText("not built")
    await expect(podman.locator('.images button:has-text("build")')).toHaveCount(1)
  }

  // The host shell, closed and opened again.
  await pane.locator('.hosts label:has-text("Nobody") input').check()
  await expect.poll(config).toContain('host = "never"')
  const shut = await (await fetch(`${BASE}/api/terminal`)).json()
  expect(shut.available).toBe(false)
  expect(shut.reason).toContain("never")
  await pane.locator('.hosts label:has-text("This machine") input').check()
  // The default is written as no table at all.
  await expect.poll(config).not.toContain("[terminal]")

  expect(errors, "the page logged errors").toEqual([])
})

/**
 * General's previews are drawn, not left as bare markup: the editor and the
 * snippets side by side, the mini editor a numbered-by-hand list in its tone,
 * the layout's miniatures and the icon themes' cards laid out. Written after
 * a cleanup of one rule took the others with it and the page showed `1. 1 fn
 * main()` — every one of these is a style that was missing then. See
 * `RECORD/2026-10-21.the-design-system-applied.completed.md`.
 */
test("Appearance draws its previews: editor, snippets, layout and the files theme", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Appearance")')

  const pane = page.locator(".appearance-pane")
  const style = (selector, property) =>
    pane.locator(selector).first().evaluate((el, p) => getComputedStyle(el)[p], property)
  await expect(pane.locator(".code-group .duo")).toBeVisible()
  expect(await style(".code-group .duo", "display")).toBe("grid")
  expect((await style(".code-group .duo", "gridTemplateColumns")).split(" ")).toHaveLength(2)
  expect(await style(".mini-lines", "listStyleType")).toBe("none")
  expect(await style(".mini-editor", "borderTopLeftRadius")).toBe("8px")
  expect(await style(".layout-preview-grid .mini-app", "display")).toBe("flex")
  expect(await style(".files-duo", "display")).toBe("grid")
  expect(await style(".big-tree", "listStyleType")).toBe("none")

  // The mini editor is in the tone the theme choice says.
  const editor = pane.locator(".mini-editor")
  await pane.getByRole("radiogroup", { name: "Editor theme" }).getByRole("radio", { name: "Light" }).click()
  await expect(editor).toHaveAttribute("data-tone", "light")
  expect(await editor.evaluate(el => getComputedStyle(el).backgroundColor)).toBe("rgb(255, 255, 255)")
  await pane.getByRole("radiogroup", { name: "Editor theme" }).getByRole("radio", { name: "Dark" }).click()
  await expect(editor).toHaveAttribute("data-tone", "dark")

  // The sizes: each on its own, applied to the page and kept for the next visit.
  const rootVar = name => page.evaluate(n => document.documentElement.style.getPropertyValue(n), name)
  await pane.getByRole("radiogroup", { name: "Editor text size" }).getByRole("radio", { name: "Large" }).click()
  await pane.getByRole("radiogroup", { name: "Chat text size" }).getByRole("radio", { name: "Small" }).click()
  const treeHeight = () => page.locator(".big-tree").evaluate(t => t.getBoundingClientRect().height)
  const before = await treeHeight()
  await pane.getByRole("radiogroup", { name: "Files size" }).getByRole("radio", { name: "Large" }).click()
  // The rows grow and the list does not: it scrolls instead.
  expect(await treeHeight()).toBe(before)
  await expect.poll(() => page.locator(".big-tree").evaluate(t => t.scrollHeight > t.clientHeight)).toBe(true)
  await expect(pane.getByRole("heading", { name: "Icons pack" })).toBeVisible()
  expect(await rootVar("--luu-editor-font")).toBe("14px")
  expect(await rootVar("--luu-chat-zoom")).toBe("0.9")
  expect(await rootVar("--luu-tree-zoom")).toBe("1.15")
  expect(await page.evaluate(() => localStorage.getItem("luu.size.editor"))).toBe("l")
  await page.evaluate(() => { for (const k of ["luu.size.tree", "luu.size.editor", "luu.size.chat"]) localStorage.removeItem(k) })

  // A server started before the previews existed answers 404: the page says
  // so, and the preview's tree is still a tree — its folders are the page's
  // to know.
  await page.route("**/api/icon-theme/previews", route => route.fulfill({ status: 404, body: "not found" }))
  await page.reload()
  await chooseFolder(page)
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Appearance")')
  await expect(page.locator(".appearance-pane")).toContainText("Restart luu serve")
  await expect(page.locator(".theme-preview .big-tree .caret")).toHaveCount(2)
  await expect(page.locator(".theme-preview .big-tree img")).toHaveCount(0)
})

/**
 * The editors' icon packs are looked for behind the answer, never in it: the
 * section is usable at once — what was found last listed, the default and
 * *+ Add a theme* live — and the page asks again until the look is done, when
 * what it found joins the list. The look is kept in the state directory, so
 * the next server starts with it. See
 * `RECORD/2026-09-28.an-icon-theme-from-settings.completed.md`, appended
 * 2026-10-22.
 */
test("the editors' icon packs are looked for in the background and kept", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 })
  // The real server first: opening Appearance starts a look, and its answer
  // is written beside config.toml.
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Appearance")')
  await expect.poll(() => existsSync(join(home, "icon-themes.detected.json")), { timeout: 15_000 }).toBe(true)
  const kept = JSON.parse(readFileSync(join(home, "icon-themes.detected.json"), "utf8"))
  expect(Array.isArray(kept.found)).toBe(true)
  expect(typeof kept.at).toBe("number")
  await page.locator(".modal-head button.close", { hasText: "close" }).click()

  // Then a server that is still looking, and then has found one.
  let asked = 0
  await page.route("**/api/icon-theme", async route => {
    if (route.request().method() !== "GET") return route.fallback()
    asked++
    const found = asked < 3 ? [] : [{ path: "~/.cursor/extensions/pub.icons-1.0.0", name: "Pub icons", source: "Cursor" }]
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({ path: null, editable: true, named: null, loaded: null, folder: null, found, looking: asked < 3 }),
    })
  })
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Appearance")')
  const list = page.locator(".theme-list")
  await expect(list).toContainText("looking in the editors' extensions…")
  // Nothing waits on the look.
  await expect(list.getByRole("radio", { name: /The tree's own/ })).toBeEnabled()
  await expect(list.getByRole("button", { name: /Add a theme/ })).toBeEnabled()
  // Asked again until it is done, and what it found joins the list.
  await expect(list.getByRole("radio", { name: /Pub icons/ })).toBeVisible({ timeout: 10_000 })
  await expect(list).not.toContainText("looking in the editors' extensions…")
  expect(asked).toBe(3)
  await page.unroute("**/api/icon-theme")
  await page.locator(".modal-head button.close", { hasText: "close" }).click()
})

/** A zip of `files` (path → text), stored rather than deflated: what a person
 *  makes of a theme folder, without a library to make it here. */
function zipOf(files) {
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    return c >>> 0
  })
  const crc32 = bytes => {
    let c = 0xffffffff
    for (const b of bytes) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8)
    return (c ^ 0xffffffff) >>> 0
  }
  const locals = []
  const centrals = []
  let offset = 0
  for (const [path, text] of Object.entries(files)) {
    const name = Buffer.from(path)
    const data = Buffer.from(text)
    const crc = crc32(data)
    const local = Buffer.alloc(30)
    local.writeUInt32LE(0x04034b50, 0); local.writeUInt16LE(20, 4); local.writeUInt32LE(crc, 14)
    local.writeUInt32LE(data.length, 18); local.writeUInt32LE(data.length, 22); local.writeUInt16LE(name.length, 26)
    const central = Buffer.alloc(46)
    central.writeUInt32LE(0x02014b50, 0); central.writeUInt16LE(20, 4); central.writeUInt16LE(20, 6)
    central.writeUInt32LE(crc, 16); central.writeUInt32LE(data.length, 20); central.writeUInt32LE(data.length, 24)
    central.writeUInt16LE(name.length, 28); central.writeUInt32LE(offset, 42)
    locals.push(local, name, data)
    centrals.push(central, name)
    offset += 30 + name.length + data.length
  }
  const directory = Buffer.concat(centrals)
  const end = Buffer.alloc(22)
  const count = Object.keys(files).length
  end.writeUInt32LE(0x06054b50, 0); end.writeUInt16LE(count, 8); end.writeUInt16LE(count, 10)
  end.writeUInt32LE(directory.length, 12); end.writeUInt32LE(offset, 16)
  return Buffer.concat([...locals, directory, end])
}

const themeFiles = label => ({
  "package.json": JSON.stringify({ name: label.toLowerCase().replace(/ /g, "-"), contributes: { iconThemes: [{ id: "t", label, path: "./theme.json" }] } }),
  "theme.json": JSON.stringify({ iconDefinitions: { _rust: { iconPath: "./icons/rust.svg" } }, fileExtensions: { rs: "_rust" }, file: "_rust" }),
  "icons/rust.svg": "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><rect width='16' height='16' fill='#c33'/></svg>",
})

/**
 * *+ Add a theme* takes a folder, a zip or a theme JSON, and the frame takes
 * any of them dropped on it. A zip is opened in the page (`lib/unzip.js`) and
 * sent as the folder it holds; one whose files are at its top is put under its
 * own name; and a JSON on its own, which names icons it does not bring, is
 * refused with what to do instead. See
 * `RECORD/2026-10-21.the-design-system-applied.completed.md`.
 */
test("a theme is added as a zip, dropped on the frame, and a bare JSON is refused", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${BASE}/index.html`)
  await chooseFolder(page)
  await page.click('.inspector .col-foot button[title="Settings"]')
  await page.click('.modal .rail button:has-text("Appearance")')

  // A zip with the theme in a folder, picked.
  const inFolder = Object.fromEntries(Object.entries(themeFiles("Zipped Icons")).map(([p, t]) => [`zipped/${p}`, t]))
  await page.locator(".modal input.pick-file").setInputFiles({ name: "zipped.zip", mimeType: "application/zip", buffer: zipOf(inFolder) })
  await expect(page.getByRole("radio", { name: /Zipped Icons/ })).toHaveAttribute("aria-checked", "true")
  await expect(page.locator(".theme-preview .big-tree img").first()).toHaveAttribute("src", /^data:image\/svg/)

  // A zip of a theme's contents, dropped on the frame: put under its own name.
  const flat = zipOf(themeFiles("Dropped Icons")).toString("base64")
  await page.locator(".files-frame").evaluate(async (frame, b64) => {
    const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0))
    const transfer = new DataTransfer()
    transfer.items.add(new File([bytes], "dropped.zip", { type: "application/zip" }))
    for (const type of ["dragenter", "dragover", "drop"]) {
      frame.dispatchEvent(new DragEvent(type, { dataTransfer: transfer, bubbles: true, cancelable: true }))
    }
  }, flat)
  await expect(page.getByRole("radio", { name: /Dropped Icons/ })).toHaveAttribute("aria-checked", "true")

  // A JSON alone: refused, saying why and what to add instead.
  await page.locator(".modal input.pick-file").setInputFiles({
    name: "bare.json", mimeType: "application/json", buffer: Buffer.from(themeFiles("Bare")["theme.json"]),
  })
  await expect(page.locator(".appearance-pane")).toContainText("icons that were not included")
  await expect(page.getByRole("radio", { name: /Dropped Icons/ })).toHaveAttribute("aria-checked", "true")
})
