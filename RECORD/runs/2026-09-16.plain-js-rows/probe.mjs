// The file viewer's row-building cost, three ways.
//
// Arms:
//   before  the two nested `:each` groups, as HEAD has them
//   html    rows.js as proposed: one insertAdjacentHTML per block
//   dom     the same, built with createElement into a DocumentFragment
//
// The binary is the debug one deliberately: `rust_embed` reads the UI from disk
// in debug and bakes it in for release, so the arms can be swapped without a
// rebuild — and the server's share is then inflated identically in every
// column, which is how phase 8 measured it too.
//
// Scratch; see `.tmp/` in AGENTS.md.

import { chromium } from "@playwright/test"
import { spawn } from "node:child_process"
import { cp, mkdir, writeFile } from "node:fs/promises"
import { execFileSync } from "node:child_process"
import { setTimeout as sleep } from "node:timers/promises"

const REPO = new URL("../../../", import.meta.url).pathname.replace(/\/$/, "")
const UI = `${REPO}/crates/luu/ui`
const HERE = `${REPO}/.tmp/plain-js-rows`
// The UI as it was before this change. Defaults to the parent of the commit
// that introduced `rows.js`, so the before arm needs nothing checked out.
const BEFORE = process.env.BEFORE_REF ?? "HEAD^"
const SWAPPED = ["content-viewer.html", "workspace.js", "app.css", "rows.js"]

async function stage() {
  await mkdir(`${HERE}/before`, { recursive: true })
  await mkdir(`${HERE}/after`, { recursive: true })
  for (const f of SWAPPED) {
    for (const [ref, into] of [[BEFORE, "before"], ["HEAD", "after"]]) {
      let bytes = ""
      try {
        // stderr ignored: `rows.js` not existing before the change is the
        // expected case below, not a failure worth printing.
        bytes = execFileSync("git", ["show", `${ref}:crates/luu/ui/${f}`],
                             { cwd: REPO, stdio: ["ignore", "pipe", "ignore"] })
      } catch {
        // `rows.js` does not exist before the change, and nothing imports it
        // there; an absent file is the right content for that arm.
        continue
      }
      await writeFile(`${HERE}/${into}/${f}`, bytes)
    }
  }
}
const PORT = 7979
const ORIGIN = `http://127.0.0.1:${PORT}`

const FILES = [
  { path: "crates/luu/src/serve.rs", label: "serve.rs" },
  { path: "crates/luu/ui/store.js", label: "store.js" },
]
const TRIALS = 7

const use = async (which, rows) => {
  for (const f of ["content-viewer.html", "workspace.js", "app.css"])
    await cp(`${HERE}/${which}/${f}`, `${UI}/${f}`)
  if (rows) await cp(rows, `${UI}/rows.js`)
}

const ARMS = {
  // As HEAD^ has it: two nested `:each` groups.
  before: () => use("before", null),
  // The shipped module, with its builder swapped for the string one.
  html: () => use("after", `${new URL("rows.html-arm.js", import.meta.url).pathname}`),
  // The shipped module as it ships.
  dom: () => use("after", `${HERE}/after/rows.js`),
}

function startServer() {
  const child = spawn(
    `${REPO}/target/debug/luu`,
    ["serve", "--bind", `127.0.0.1:${PORT}`, "--no-store"],
    { cwd: REPO, env: { ...process.env, LUU_HOME: `${HERE}/home` }, stdio: "pipe" },
  )
  child.stdout.on("data", () => {})
  child.stderr.on("data", d => process.env.PROBE_VERBOSE && process.stderr.write(d))
  return child
}

async function waitForServer() {
  for (let n = 0; n < 100; n++) {
    try {
      const answer = await fetch(`${ORIGIN}/`)
      if (answer.ok) return
    } catch {}
    await sleep(100)
  }
  throw new Error("serve never came up")
}

/// One file opened, instrumented. Returns the two moments phase 8 reported —
/// the first rows on screen and the last — plus what the result costs to hold.
const measure = async ({ path, expected }) => {
  const { showFile } = await import("./workspace.js")
  // `document.body`, not `.col-main`: there are two of those — the inspector's
  // and the content column's — and observing the first one watches the file
  // tree change while the file it is supposed to be timing renders elsewhere.
  const rowsNow = () => document.querySelectorAll("ol.code > li").length
  const column = () => document.querySelector("ol.code")?.closest(".col-main") ?? null

  return await new Promise(resolve => {
    let first = null
    let marks = 0
    const done = extra => {
      seen.disconnect()
      const col = column()
      resolve({
        first,
        whole: performance.now() - t0,
        rows: rowsNow(),
        nodes: col ? col.querySelectorAll("*").length : -1,
        scrollHeight: col ? col.scrollHeight : -1,
        marks,
        ...extra,
      })
    }
    const seen = new MutationObserver(() => {
      const rows = rowsNow()
      if (!rows) return
      if (first === null) first = performance.now() - t0
      marks++
      // One more frame, so layout for the last block is included rather than
      // charged to whatever runs next.
      if (rows >= expected) requestAnimationFrame(() => requestAnimationFrame(() => done({})))
    })
    seen.observe(document.body, { childList: true, subtree: true })
    setTimeout(() => done({ timedOut: true }), 15000)
    var t0 = performance.now()
    showFile(path)
  })
}

const median = xs => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)]

async function runArm(name, browser) {
  await ARMS[name]()
  const out = {}
  for (const file of FILES) {
    const context = await browser.newContext({ viewport: { width: 1600, height: 900 } })
    const page = await context.newPage()
    page.on("console", m => {
      if (m.type() === "error") console.error(`    [console] ${m.text()}`)
    })
    await page.goto(ORIGIN)
    // The folder picker opens on a first visit; answering it in localStorage is
    // what a returning visitor has, and it is the state being measured.
    const base = await page.evaluate(async () => {
      const { workspace } = await import("./workspace.js")
      for (let n = 0; n < 100 && !workspace.base; n++) await new Promise(r => setTimeout(r, 50))
      return workspace.base
    })
    await page.evaluate(b => localStorage.setItem(`luu.root:${b}`, ""), base)
    await page.reload()
    await page.waitForSelector(".col-main")

    const total = JSON.parse(
      await (await fetch(`${ORIGIN}/api/workspace/file?path=${encodeURIComponent(file.path)}`)).text(),
    ).lines.length

    const runs = []
    for (let n = 0; n <= TRIALS; n++) {
      const got = await page.evaluate(measure, { path: file.path, expected: total })
      if (!got) throw new Error(`arm ${name}, ${file.label}, trial ${n}: measure returned nothing`)
      if (process.env.PROBE_VERBOSE) console.error(`    ${name} ${file.label} #${n} first=${Math.round(got.first)} whole=${Math.round(got.whole)} rows=${got.rows}/${total}`)
      // The first run of each file pays for grammar warm-up and the module
      // graph; it is a warm-up, not a sample.
      if (n > 0) runs.push(got)
      await page.evaluate(async () => {
        const { closeTab, workspace } = await import("./workspace.js")
        if (workspace.active) closeTab(workspace.active)
      })
      await sleep(60)
    }
    const heap = await page.evaluate(() => performance.memory?.usedJSHeapSize ?? null)
    out[file.label] = {
      lines: total,
      first: Math.round(median(runs.map(r => r.first))),
      whole: Math.round(median(runs.map(r => r.whole))),
      nodes: runs[0].nodes,
      rows: runs[0].rows,
      scrollHeight: runs[0].scrollHeight,
      heapMB: heap === null ? null : +(heap / 1048576).toFixed(1),
    }
    await context.close()
  }
  return out
}

const server = startServer()
try {
  await stage()
  await waitForServer()
  const browser = await chromium.launch({ args: ["--enable-precise-memory-info"] })
  const results = {}
  for (const arm of ["before", "html", "dom"]) {
    process.stderr.write(`==> ${arm}\n`)
    results[arm] = await runArm(arm, browser)
  }
  await browser.close()
  console.log(JSON.stringify(results, null, 2))
} finally {
  server.kill("SIGKILL")
  // Leave the tree in the state the work is in, never in an arm's.
  await ARMS.dom()
}
