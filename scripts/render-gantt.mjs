// Renders the ```mermaid``` blocks of a roadmap revision into one standalone
// HTML page, with the SVG inlined.
//
// The page is generated from the markdown rather than written beside it, for
// the same reason the fixtures are recorded rather than hand-written: two
// copies of one chart is one chart and one lie, and nothing would say which.
// Regenerate it whenever the markdown changes.
//
//   npm i mermaid playwright        # not workspace dependencies; cargo needs neither
//   node scripts/render-gantt.mjs ROADMAP/2026-08-31
//
// Chromium comes from a normal playwright install; set CHROMIUM=<path> to point
// at one that is already on the machine.

import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { join, basename } from "node:path";
import { createRequire } from "node:module";
import { chromium } from "playwright";

const require = createRequire(import.meta.url);
const dir = process.argv[2];
if (!dir) {
  console.error("usage: node scripts/render-gantt.mjs <roadmap-revision-dir>");
  process.exit(1);
}

const blocks = [];
for (const name of readdirSync(dir).filter((f) => f.endsWith(".md")).sort()) {
  const md = readFileSync(join(dir, name), "utf8");
  for (const m of md.matchAll(/```mermaid\n([\s\S]*?)```/g)) {
    const def = m[1].trimEnd();
    const title = (def.match(/^\s*title\s+(.+)$/m) || [, "Untitled"])[1].trim();
    blocks.push({ source: name, title, def });
  }
}
if (blocks.length === 0) {
  console.error(`no mermaid blocks in ${dir}`);
  process.exit(1);
}

const browser = await chromium.launch({ executablePath: process.env.CHROMIUM || undefined });
const page = await browser.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
await page.setContent("<!doctype html><body><div id=g></div>");
await page.addScriptTag({ path: require.resolve("mermaid/dist/mermaid.min.js") });

const rendered = [];
for (const b of blocks) {
  const svg = await page.evaluate(async ([def, id]) => {
    // startOnLoad off: render() is called explicitly, one diagram at a time, so
    // a diagram that fails to parse throws here instead of silently painting an
    // error box into the page.
    window.mermaid.initialize({ startOnLoad: false, theme: "neutral", gantt: { useWidth: 1100 } });
    const { svg } = await window.mermaid.render(id, def);
    return svg;
  }, [b.def, `d${rendered.length}`]);
  rendered.push({ ...b, svg });
  console.log(`rendered  ${b.source}  ${b.title}`);
}
await browser.close();
if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}

const esc = (s) => s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
const revision = basename(dir);
const html = `<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>luu roadmap ${esc(revision)} — Gantt</title>
<style>
  :root { color-scheme: light; }
  body {
    margin: 0; padding: 2.5rem 1.5rem 4rem;
    background: #f6f6f4; color: #1d1d1b;
    font: 15px/1.55 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
  }
  main { max-width: 1180px; margin: 0 auto; }
  h1 { font-size: 1.5rem; margin: 0 0 .35rem; letter-spacing: -.01em; }
  .lede { margin: 0 0 2.5rem; color: #5c5c58; max-width: 62ch; }
  .lede code { background: #e9e9e5; padding: .1em .35em; border-radius: 3px; font-size: .9em; }
  figure {
    margin: 0 0 2rem; padding: 1.25rem 1.25rem .75rem;
    background: #fff; border: 1px solid #e2e2dc; border-radius: 8px;
    overflow-x: auto;
  }
  figcaption {
    margin-bottom: .9rem; font-size: .8rem; text-transform: uppercase;
    letter-spacing: .06em; color: #8a8a83;
  }
  svg { max-width: 100%; height: auto; display: block; }
  footer { margin-top: 3rem; font-size: .8rem; color: #8a8a83; }
  a { color: #1d1d1b; }
</style>
<main>
  <h1>luu roadmap — revision ${esc(revision)}</h1>
  <p class="lede">
    The bars are <strong>sizes, not commitments</strong>: one person working
    evenings, an arbitrary start date, and the only thing worth trusting is the
    shape — what runs in parallel, what waits, and where the decision points
    fall. Generated from the markdown in <code>ROADMAP/${esc(revision)}/</code>
    by <code>scripts/render-gantt.mjs</code>; edit the markdown, not this file.
  </p>
${rendered
  .map((r) => `  <figure>\n    <figcaption>${esc(r.source)} — ${esc(r.title)}</figcaption>\n${r.svg}\n  </figure>`)
  .join("\n")}
  <footer>Mermaid ${esc(require("mermaid/package.json").version)}.</footer>
</main>
`;
const out = join(dir, "gantt.html");
writeFileSync(out, html);
console.log(`\nwrote ${out}`);
