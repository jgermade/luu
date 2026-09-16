// Just the builders, with the fetch and the server taken out of the way.
//
// The end-to-end probe puts `insertAdjacentHTML` and `createElement` within a
// millisecond of each other on the big file, which is the sort of tie that is
// usually an instrument problem rather than a result. This times the two
// construction paths alone, over a payload that is already in memory, into an
// `<ol>` that is really in the document — so layout is charged, because layout
// is what a person waits for.
//
// Scratch; see `.tmp/` in AGENTS.md.

import { chromium } from "@playwright/test"
import { spawn } from "node:child_process"
import { setTimeout as sleep } from "node:timers/promises"

const REPO = new URL("../../../", import.meta.url).pathname.replace(/\/$/, "")
const PORT = 7982
const ORIGIN = `http://127.0.0.1:${PORT}`
const RUNS = 15

const server = spawn(`${REPO}/target/debug/luu`, ["serve", "--bind", `127.0.0.1:${PORT}`, "--no-store"],
  { cwd: REPO, env: { ...process.env, LUU_HOME: `${REPO}/.tmp/plain-js-rows/home` }, stdio: "ignore" })
for (let n = 0; n < 100; n++) { try { if ((await fetch(ORIGIN)).ok) break } catch {} await sleep(100) }

const browser = await chromium.launch({ args: ["--enable-precise-memory-info", "--js-flags=--expose-gc"] })
const page = await browser.newPage({ viewport: { width: 1600, height: 900 } })
await page.goto(ORIGIN)

const out = await page.evaluate(async ({ paths, runs }) => {
  const ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;" }
  const NEEDS = /[&<>]/
  const esc = t => (NEEDS.test(t) ? t.replace(/[&<>]/g, c => ESCAPES[c]) : t)

  const builders = {
    // One string, one parse.
    html(ol, lines) {
      let html = ""
      for (let at = 0; at < lines.length; at++) {
        const line = lines[at]
        html += "<li>"
        for (let n = 0; n < line.length; n++) {
          const c = line[n]
          html += c.kind ? `<code class="hl-${c.kind}">${esc(c.text)}</code>` : `<code>${esc(c.text)}</code>`
        }
        html += "</li>"
      }
      ol.insertAdjacentHTML("beforeend", html)
    },
    // The same shape built node by node into a fragment. No escaping:
    // `textContent` is safe by construction, which is half of what this arm is
    // being priced against.
    dom(ol, lines) {
      const frag = document.createDocumentFragment()
      for (let at = 0; at < lines.length; at++) {
        const line = lines[at]
        const li = document.createElement("li")
        for (let n = 0; n < line.length; n++) {
          const c = line[n]
          const code = document.createElement("code")
          if (c.kind) code.className = `hl-${c.kind}`
          code.textContent = c.text
          li.appendChild(code)
        }
        frag.appendChild(li)
      }
      ol.appendChild(frag)
    },
    // The string build alone, escaped and not. Neither touches the DOM, so the
    // pair prices the escape pass without confounding it with a parse of
    // different bytes — which is what an unescaped arm would really be
    // measuring on a file full of `Vec<T>` and `&str`.
    "string only"(ol, lines) {
      let html = ""
      for (let at = 0; at < lines.length; at++) {
        const line = lines[at]
        html += "<li>"
        for (let n = 0; n < line.length; n++) {
          const c = line[n]
          html += c.kind ? `<code class="hl-${c.kind}">${esc(c.text)}</code>` : `<code>${esc(c.text)}</code>`
        }
        html += "</li>"
      }
      sink = html.length
    },
    "string only (no escape)"(ol, lines) {
      let html = ""
      for (let at = 0; at < lines.length; at++) {
        const line = lines[at]
        html += "<li>"
        for (let n = 0; n < line.length; n++) {
          const c = line[n]
          html += c.kind ? `<code class="hl-${c.kind}">${c.text}</code>` : `<code>${c.text}</code>`
        }
        html += "</li>"
      }
      sink = html.length
    },
  }

  let sink = 0
  const host = document.createElement("div")
  host.style.cssText = "position:absolute;top:0;left:0;width:1200px;height:800px;overflow:auto;contain:strict"
  document.body.appendChild(host)

  const median = xs => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)]
  const results = {}

  for (const path of paths) {
    const file = await (await fetch(`./api/workspace/file?path=${encodeURIComponent(path)}`)).json()
    const chunks = file.lines.reduce((n, l) => n + l.length, 0)
    const bytes = file.lines.reduce((n, l) => n + l.reduce((m, c) => m + c.text.length, 0), 0)
    results[path] = { lines: file.lines.length, chunks, chars: bytes, arms: {} }

    const names = Object.keys(builders)
    const times = Object.fromEntries(names.map(n => [n, []]))
    const heaps = {}
    for (let n = 0; n < runs; n++) {
      // Round-robin, rotated each pass, so no arm is always first or last.
      for (let k = 0; k < names.length; k++) {
        const name = names[(k + n) % names.length]
        const ol = document.createElement("ol")
        ol.className = "code"
        host.replaceChildren(ol)
        // A frame between runs, so the previous one's layout and its garbage
        // are not charged to this one.
        await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))
        const t0 = performance.now()
        builders[name](ol, file.lines)
        // Forces layout now rather than letting it land on whatever runs next.
        void ol.offsetHeight
        times[name].push(performance.now() - t0)
        heaps[name] = performance.memory?.usedJSHeapSize ?? null
      }
    }
    // The first pass of each arm is warm-up, not a sample.
    for (const name of names) {
      const kept = times[name].slice(1)
      results[path].arms[name] = {
        median: +median(kept).toFixed(1),
        min: +Math.min(...kept).toFixed(1),
        heapMB: heaps[name] === null ? null : +(heaps[name] / 1048576).toFixed(1),
      }
    }
  }
  host.remove()
  return results
}, { paths: ["crates/luu/src/serve.rs", "crates/luu/ui/store.js"], runs: RUNS })

console.log(JSON.stringify(out, null, 2))
await browser.close()
server.kill("SIGKILL")
