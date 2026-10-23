// @ts-check
/// A reply's prose as HTML: the Markdown a model writes — headings, lists,
/// quotes, tables, rules, emphasis, code spans and links — drawn, rather than
/// shown as its marks. Runs on the text parts `snippets.js` leaves, so a
/// fenced block never reaches it: that is a snippet, drawn as code.
///
/// **Everything is escaped first.** The text is the model's, and the model's
/// text is not the page's markup: an `<img onerror>` in a reply is shown as
/// those characters. What this adds is a fixed set of elements, and a link only
/// when its address is `http:`, `https:` or `mailto:`. Any other address leaves
/// its label as text.
///
/// **Only what jq79's `:html` keeps.** Its sanitiser lets through `p`, `br`,
/// `strong`, `em`, `code`, `a` (its `href`, given `rel="noopener noreferrer"`),
/// the lists, `blockquote`, the headings, `span` and `div` — each with a
/// `class` and no other attribute. So what Markdown has beyond those is drawn
/// with them: a table is `div`s and `span`s laid out as one by `app.css`, a rule
/// is an empty `div.md-hr`, struck text a `span.md-del`, a task's box a
/// `span.md-box`, and a list that starts past 1 carries its numbers as text.
/// The transcript opens a link in a new tab itself, since `target` is not kept.
///
/// **A line break is a line break**, as it was while the reply was plain text:
/// a model that breaks a line means it, and joining the lines would change
/// what it wrote.
///
/// **Partial while it streams**: a `**` whose partner has not come is two
/// asterisks until it comes, and a table is a table once its second line says
/// so. A reply is drawn again on every token, so what came back is kept, by
/// text, for the rows that did not change.
///
/// See `RECORD/2026-10-22.replies-drawn-as-markdown.completed.md`.

const escape = (/** @type {string} */ text) => text
  .replace(/&/g, "&amp;")
  .replace(/</g, "&lt;")
  .replace(/>/g, "&gt;")
  .replace(/"/g, "&quot;")

const SAFE = /^(https?:|mailto:)/i
const opening = (/** @type {string} */ href) => `<a href="${escape(href)}">`

// ---- inline -----------------------------------------------------------------

/// A line's spans. What must not be read again — a code span, an escaped mark,
/// a link's tags — is held out of the text as a numbered mark and put back at
/// the end, so the emphasis that follows cannot reach into it. A link's label
/// stays in the text, and is read with the rest: `[**this**](…)` is bold.
export function inline(/** @type {string} */ text) {
  /** @type {string[]} */
  const held = []
  const hold = (/** @type {string} */ html) => `\u0000${held.push(html) - 1}\u0000`
  let out = text
    // A code span is never anything else: its text is shown as it is.
    .replace(/(`+)([\s\S]*?[^`])\1(?!`)/g, (_, _ticks, code) =>
      hold(`<code>${escape(code.replace(/^ ([\s\S]*) $/, "$1"))}</code>`))
    // `\*` is an asterisk.
    .replace(/\\([\\`*_{}[\]()#+\-.!|~<>])/g, (_, mark) => hold(escape(mark)))
    // [label](address "title"). A label that is itself an address is held
    // whole, so the bare addresses below do not make a link inside the link.
    .replace(/\[([^\]\n]+)\]\(\s*<?([^\s<>()]+(?:\([^\s()]*\))?)>?(?:\s+"[^"\n]*")?\s*\)/g, (_, label, href) => {
      const text = /https?:\/\//i.test(label) ? hold(escape(label)) : label
      return SAFE.test(href) ? hold(opening(href)) + text + hold("</a>") : text
    })
    // <https://…>, and an address written bare.
    .replace(/<(https?:\/\/[^\s<>]+|mailto:[^\s<>]+)>/g, (_, href) => hold(opening(href) + escape(href) + "</a>"))
    .replace(/\bhttps?:\/\/[^\s<>"'`\u0000]*[^\s<>"'`\u0000.,:;!?)\]]/g, href => hold(opening(href) + escape(href) + "</a>"))
  out = escape(out)
    .replace(/\*\*(?=\S)([\s\S]*?\S)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^\w])__(?=\S)([\s\S]*?\S)__(?!\w)/g, "$1<strong>$2</strong>")
    .replace(/~~(?=\S)([\s\S]*?\S)~~/g, '<span class="md-del">$1</span>')
    .replace(/(^|[^*])\*(?=[^\s*])([^*]*?[^\s*])\*(?!\*)/g, "$1<em>$2</em>")
    .replace(/(^|[^\w])_(?=[^\s_])([^_]*?[^\s_])_(?!\w)/g, "$1<em>$2</em>")
  return out.replace(/\u0000(\d+)\u0000/g, (_, n) => held[Number(n)])
}

// ---- blocks -----------------------------------------------------------------

const HEADING = /^ {0,3}(#{1,6})\s+(.*?)(?:\s+#+)?\s*$/
const RULE = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/
const QUOTE = /^ {0,3}> ?/
const ITEM = /^( *)([-*+]|\d{1,9}[.)])(?:[ \t]+(.*))?$/
const ROW_RULE = /^ {0,3}\|?[ \t]*:?-+:?[ \t]*(\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$/
const blank = (/** @type {string} */ line) => !line.trim()
const indentOf = (/** @type {string} */ line) => /^ */.exec(line)?.[0].length ?? 0

/// What a line opens, if it opens something other than a paragraph.
const opens = (/** @type {string[]} */ lines, /** @type {number} */ i) => {
  const line = lines[i]
  return HEADING.test(line) || RULE.test(line) || QUOTE.test(line) || ITEM.test(line) ||
    (line.includes("|") && i + 1 < lines.length && ROW_RULE.test(lines[i + 1]) && lines[i + 1].includes("-"))
}

const cells = (/** @type {string} */ line) => line.trim()
  .replace(/^\|/, "")
  .replace(/(?<!\\)\|$/, "")
  .split(/(?<!\\)\|/)
  .map(cell => cell.trim())

/// A table as `div.md-table` > `div.md-row` > `span.md-cell`, the head's row
/// `.md-head`, a column's alignment a class on its cells; `app.css` lays them
/// out as the table they are, in a box that scrolls sideways when it is wider
/// than the column.
function table(/** @type {string[]} */ lines, /** @type {number} */ i) {
  const head = cells(lines[i])
  const align = cells(lines[i + 1]).map(rule =>
    rule.startsWith(":") && rule.endsWith(":") ? " md-center" : rule.endsWith(":") ? " md-right" : "")
  const row = (/** @type {string[]} */ own, kind = "") =>
    `<div class="md-row${kind}">` + head.map((_, c) => `<span class="md-cell${align[c] ?? ""}">${inline(own[c] ?? "")}</span>`).join("") + "</div>"
  let html = row(head, " md-head")
  let next = i + 2
  while (next < lines.length && !blank(lines[next]) && lines[next].includes("|")) html += row(cells(lines[next++]))
  return { html: `<div class="md-table-box"><div class="md-table">${html}</div></div>`, next }
}

function list(/** @type {string[]} */ lines, /** @type {number} */ i) {
  const first = /** @type {RegExpExecArray} */ (ITEM.exec(lines[i]))
  const base = first[1].length
  const ordered = /\d/.test(first[2])
  const start = ordered ? parseInt(first[2], 10) : 1
  /** @type {{ lines: string[], pad: number }[]} */
  const items = []
  let loose = false
  let next = i
  while (next < lines.length) {
    const line = lines[next]
    const item = ITEM.exec(line)
    if (item && item[1].length === base && /\d/.test(item[2]) === ordered) {
      items.push({ lines: [item[3] ?? ""], pad: base + item[2].length + 1 })
      next++
      continue
    }
    if (blank(line)) {
      // A blank line inside a list keeps it going only if the list does.
      let ahead = next + 1
      while (ahead < lines.length && blank(lines[ahead])) ahead++
      const after = lines[ahead]
      const more = after !== undefined && (indentOf(after) > base ||
        (ITEM.test(after) && indentOf(after) === base && /\d/.test(/** @type {RegExpExecArray} */ (ITEM.exec(after))[2]) === ordered))
      if (!more) break
      loose = loose || indentOf(after) <= base
      items[items.length - 1].lines.push("")
      next++
      continue
    }
    const current = items[items.length - 1]
    if (indentOf(line) > base) {
      current.lines.push(line.slice(Math.min(indentOf(line), current.pad)))
      next++
      continue
    }
    // A line that carries on the item's sentence with no indent of its own.
    if (!item && !blank(current.lines[current.lines.length - 1]) && !opens(lines, next)) {
      current.lines.push(line.trim())
      next++
      continue
    }
    break
  }
  const tag = ordered ? "ol" : "ul"
  // `start` is not kept, so a list that goes on after a code block — `2.` on —
  // writes its numbers.
  const counted = ordered && start !== 1
  const html = items.map(({ lines: own }, n) => {
    const box = /^\[([ xX])\][ \t]+/.exec(own[0])
    if (box) own = [own[0].slice(box[0].length), ...own.slice(1)]
    const mark = box ? `<span class="md-box${box[1] === " " ? "" : " md-on"}"></span>`
      : counted ? `<span class="md-n">${start + n}.</span>` : ""
    return `<li${box ? ' class="md-task"' : ""}>${mark}${blocks(own, !loose && !own.some(blank))}</li>`
  }).join("")
  return { html: `<${tag}${counted ? ' class="md-counted"' : ""}>${html}</${tag}>`, next }
}

/// The blocks of some lines. `tight` is a list item's: its paragraph is not
/// wrapped in a `<p>`, so a short list does not stand apart like an essay.
function blocks(/** @type {string[]} */ lines, tight = false) {
  let html = ""
  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    if (blank(line)) { i++; continue }
    const heading = HEADING.exec(line)
    if (heading) {
      const level = heading[1].length
      html += `<h${level}>${inline(heading[2])}</h${level}>`
      i++
      continue
    }
    // Before a list: `- - -` and `* * *` are rules.
    if (RULE.test(line)) {
      html += '<div class="md-hr"></div>'
      i++
      continue
    }
    if (QUOTE.test(line)) {
      const quoted = []
      while (i < lines.length && QUOTE.test(lines[i])) quoted.push(lines[i++].replace(QUOTE, ""))
      html += `<blockquote>${blocks(quoted)}</blockquote>`
      continue
    }
    if (line.includes("|") && i + 1 < lines.length && ROW_RULE.test(lines[i + 1]) && lines[i + 1].includes("-")) {
      const made = table(lines, i)
      html += made.html
      i = made.next
      continue
    }
    if (ITEM.test(line)) {
      const made = list(lines, i)
      html += made.html
      i = made.next
      continue
    }
    const paragraph = [line.trim()]
    i++
    while (i < lines.length && !blank(lines[i]) && !opens(lines, i)) paragraph.push(lines[i++].trim())
    const text = inline(paragraph.join("\n")).replace(/\n/g, "<br>")
    html += tight ? text : `<p>${text}</p>`
  }
  return html
}

const kept = new Map()

/// The reply's prose as HTML for `:html`.
export function markdown(/** @type {string} */ text) {
  const known = kept.get(text)
  if (known !== undefined) return known
  const html = blocks(text.replace(/\r\n?/g, "\n").replace(/\u0000/g, "").split("\n"))
  if (kept.size > 400) kept.clear()
  kept.set(text, html)
  return html
}
