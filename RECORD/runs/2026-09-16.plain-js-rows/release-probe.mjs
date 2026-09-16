// The shipped viewer, timed against the release binary — the build the
// AGENTS.md numbers and the design doc quote. Only one arm: `rust_embed` bakes
// the UI into a release build, so the arms cannot be swapped here, and the
// before-column stays the 271 ms already on record.
import { chromium } from "@playwright/test"
import { spawn } from "node:child_process"
import { setTimeout as sleep } from "node:timers/promises"
const REPO = new URL("../../../", import.meta.url).pathname.replace(/\/$/, "")
const BIN = process.argv[2] ?? `${REPO}/target/release/luu`
const PORT = Number(process.argv[3] ?? 7986), ORIGIN = `http://127.0.0.1:${PORT}`, TRIALS = 9
const server = spawn(BIN, ["serve","--bind",`127.0.0.1:${PORT}`,"--no-store"],
  { cwd: REPO, env: { ...process.env, LUU_HOME: `${REPO}/.tmp/plain-js-rows/home` }, stdio: "ignore" })
for (let n=0;n<200;n++){ try{ if((await fetch(ORIGIN)).ok) break }catch{} await sleep(100) }
const browser = await chromium.launch({ args: ["--enable-precise-memory-info"] })
const page = await browser.newPage({ viewport: { width: 1600, height: 900 } })
await page.goto(ORIGIN)
const base = await page.evaluate(async () => { const { workspace } = await import("./workspace.js")
  for (let n=0;n<100&&!workspace.base;n++) await new Promise(r=>setTimeout(r,50)); return workspace.base })
await page.evaluate(b => localStorage.setItem(`luu.root:${b}`, ""), base)
await page.reload(); await page.waitForSelector(".col-main")

const measure = async ({ path, expected }) => {
  const { showFile } = await import("./workspace.js")
  const rowsNow = () => document.querySelectorAll("ol.code > li").length
  const column = () => document.querySelector("ol.code")?.closest(".col-main") ?? null
  return await new Promise(resolve => {
    let first = null
    const done = () => { seen.disconnect(); const col = column()
      resolve({ first, whole: performance.now() - t0, rows: rowsNow(),
                nodes: col ? col.querySelectorAll("*").length : -1,
                scrollHeight: col ? col.scrollHeight : -1 }) }
    const seen = new MutationObserver(() => {
      const rows = rowsNow(); if (!rows) return
      if (first === null) first = performance.now() - t0
      if (rows >= expected) requestAnimationFrame(() => requestAnimationFrame(done))
    })
    seen.observe(document.body, { childList: true, subtree: true })
    setTimeout(done, 15000)
    var t0 = performance.now()
    showFile(path)
  })
}
const median = xs => [...xs].sort((a,b)=>a-b)[Math.floor(xs.length/2)]
const out = {}
for (const path of ["crates/luu/src/serve.rs", "crates/luu/ui/store.js"]) {
  const total = (await (await fetch(`${ORIGIN}/api/workspace/file?path=${encodeURIComponent(path)}`)).json()).lines.length
  const runs = []
  for (let n=0;n<=TRIALS;n++) {
    const got = await page.evaluate(measure, { path, expected: total })
    if (n>0) runs.push(got)
    await page.evaluate(async () => { const { closeTab, workspace } = await import("./workspace.js")
      if (workspace.active) closeTab(workspace.active) })
    await sleep(60)
  }
  const heap = await page.evaluate(() => performance.memory?.usedJSHeapSize ?? null)
  out[path] = { lines: total, first: Math.round(median(runs.map(r=>r.first))),
                whole: Math.round(median(runs.map(r=>r.whole))), nodes: runs[0].nodes,
                scrollHeight: runs[0].scrollHeight,
                heapMB: heap===null?null:+(heap/1048576).toFixed(1) }
}
console.log(JSON.stringify(out, null, 2))
await browser.close(); server.kill("SIGKILL")
