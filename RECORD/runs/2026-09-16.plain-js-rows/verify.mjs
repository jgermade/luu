// Does the rebuilt viewer actually behave? The things phase 8 checked when it
// argued for two blocks, re-checked now that nothing re-renders:
//
//   - the row rules still apply, now that they live in app.css rather than in
//     a `<style scoped>` that would have sealed them to `[data-jq79=…]`
//   - the scrollbar is the length of the file from the first frame
//   - a selection made in the first screenful survives the tail landing
//   - so does the scroll position
//   - the other two tab kinds still draw
//
// Scratch; see `.tmp/` in AGENTS.md.

import { chromium } from "@playwright/test"
import { spawn } from "node:child_process"
import { setTimeout as sleep } from "node:timers/promises"

const REPO = new URL("../../../", import.meta.url).pathname.replace(/\/$/, "")
const PORT = 7983
const ORIGIN = `http://127.0.0.1:${PORT}`
const FILE = "crates/luu/src/serve.rs"

const server = spawn(`${REPO}/target/debug/luu`, ["serve", "--bind", `127.0.0.1:${PORT}`, "--no-store"],
  { cwd: REPO, env: { ...process.env, LUU_HOME: `${REPO}/.tmp/plain-js-rows/home` }, stdio: "ignore" })
for (let n = 0; n < 100; n++) { try { if ((await fetch(ORIGIN)).ok) break } catch {} await sleep(100) }

const browser = await chromium.launch()
const page = await browser.newPage({ viewport: { width: 1600, height: 900 } })
const problems = []
page.on("pageerror", e => problems.push(`pageerror: ${e.message}`))
page.on("console", m => { if (m.type() === "error") problems.push(`console: ${m.text()}`) })

await page.goto(ORIGIN)
const base = await page.evaluate(async () => {
  const { workspace } = await import("./workspace.js")
  for (let n = 0; n < 100 && !workspace.base; n++) await new Promise(r => setTimeout(r, 50))
  return workspace.base
})
await page.evaluate(b => localStorage.setItem(`luu.root:${b}`, ""), base)
await page.reload()
await page.waitForSelector(".col-main")

// ---- the two-pass checks, taken across the frame the tail lands on ---------
const twoPass = await page.evaluate(async path => {
  const { showFile } = await import("./workspace.js")
  const rows = () => document.querySelectorAll("ol.code > li")
  const column = () => document.querySelector("ol.code")?.closest(".col-main")

  showFile(path)
  // Wait for the head, and stop before the tail: the whole point is what
  // happens across that boundary.
  for (let n = 0; n < 400 && rows().length === 0; n++) await new Promise(r => setTimeout(r, 5))
  const head = rows().length

  const col = column()
  const firstRow = rows()[0]
  const styleOf = el => {
    const s = getComputedStyle(el)
    return { display: s.display, whiteSpace: s.whiteSpace, fontFamily: s.fontFamily.slice(0, 20) }
  }
  // The gutter is a `::before` on the row; if the global rules did not apply it
  // has no width at all.
  const gutter = getComputedStyle(firstRow, "::before").width

  // A selection inside the first screenful, and a scroll into it.
  const target = rows()[4].querySelector("code")
  const range = document.createRange()
  range.selectNodeContents(target)
  const sel = getSelection()
  sel.removeAllRanges()
  sel.addRange(range)
  const selectedBefore = sel.toString()
  col.scrollTop = 900
  const scrolledBefore = col.scrollTop

  const before = {
    head,
    scrollHeight: col.scrollHeight,
    rowStyle: styleOf(firstRow),
    gutter,
    spacerShown: !document.getElementById("code-pending")?.hidden,
    identity: firstRow,
  }

  // Now let the tail land.
  for (let n = 0; n < 400 && rows().length === head; n++) await new Promise(r => setTimeout(r, 5))
  await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))

  return {
    head,
    whole: rows().length,
    scrollHeightBefore: before.scrollHeight,
    scrollHeightAfter: col.scrollHeight,
    rowStyle: before.rowStyle,
    gutter: before.gutter,
    spacerShownBeforeTail: before.spacerShown,
    spacerHiddenAfterTail: !!document.getElementById("code-pending")?.hidden,
    firstRowSurvived: before.identity.isConnected && rows()[0] === before.identity,
    selectedBefore,
    selectedAfter: getSelection().toString(),
    scrolledBefore,
    scrolledAfter: col.scrollTop,
  }
}, FILE)

// ---- the other two tab kinds ----------------------------------------------
const others = await page.evaluate(async () => {
  const { showDebug, showDiff, workspace } = await import("./workspace.js")
  showDebug("probe", "a debug block", "hello from the probe")
  for (let n = 0; n < 200 && !document.querySelector("pre.debug"); n++)
    await new Promise(r => setTimeout(r, 5))
  const debugText = document.querySelector("pre.debug")?.textContent ?? null

  showDiff("crates/luu/ui/rows.js", false)
  for (let n = 0; n < 400 && workspace.loading; n++) await new Promise(r => setTimeout(r, 5))
  await new Promise(r => setTimeout(r, 150))
  return {
    debugText,
    diffKind: workspace.content?.kind ?? null,
    diffHunks: document.querySelectorAll(".diff .hunk").length,
  }
})

await page.evaluate(async p => {
  const { showFile } = await import("./workspace.js")
  showFile(p)
  await new Promise(r => setTimeout(r, 800))
}, FILE)
await page.screenshot({ path: `${REPO}/.tmp/plain-js-rows/viewer.png` })

console.log(JSON.stringify({ twoPass, others, problems }, null, 2))
await browser.close()
server.kill("SIGKILL")
