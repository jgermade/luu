// @ts-check
/// The prompt a turn was sent, cut into the budget's buckets so each piece can
/// wear its bucket's colour.
///
/// The server sends the buckets as *counts* and the prompt as *one string*;
/// nothing on the wire says where one bucket ends and the next begins. So the
/// cut is read back here from what the renderer writes the same way every
/// time — `Context::system_message`, `user_text`, a fold's pair:
///
/// - the system message is the system prompt, then `# Tools…`, then
///   `# Repository map…`, joined with blank lines, and an authority note
///   last when one is set in `system` position;
/// - a closed job is a user message (its objective) and an assistant message
///   opening `[job closed]` or `[draft closed]` — both **summaries**;
/// - every other exchange before the last user message is **history**;
/// - the last user message is this turn's fragments (`// path` blocks, the
///   **code**) and then the **prompt**, which ends with what was asked.
///
/// Where a boundary cannot be found, the text stays in the bucket it was
/// already in rather than being guessed into another: a colour is a claim.
/// See `RECORD/2026-09-30.the-foot-and-the-turn.completed.md`.

/// `blocks` is `[{ role, text }]`, as `context-view.html` cuts the prompt at
/// its role markers. `asked` is what the person typed; `notes` the authority
/// notes' words this session was sent under. Returns the same blocks, each
/// with `parts: [{ bucket, text }]`.
export function paint(blocks, asked = "", notes = []) {
  const lastUser = blocks.map(b => b.role).lastIndexOf("user")
  return blocks.map((block, i) => {
    const next = blocks[i + 1]
    let parts
    if (block.role === "system") parts = system(block.text, notes)
    else if (isFold(block.text) || (block.role === "user" && next && isFold(next.text))) {
      parts = [{ bucket: "summaries", text: block.text }]
    } else if (i === lastUser) parts = question(block.text, asked, notes)
    else if (block.role === "prompt") parts = [{ bucket: "prompt", text: block.text }]
    else parts = [{ bucket: "history", text: block.text }]
    return { ...block, parts: parts.filter(p => p.text) }
  })
}

const isFold = text => /^\[(job|draft) closed\]/.test(text)

function system(text, notes) {
  const at = (header, from) => {
    const found = text.indexOf(`\n\n${header}`, from)
    return found < 0 ? -1 : found + 2
  }
  const tools = at("# Tools", 0)
  const map = at("# Repository map", Math.max(tools, 0))
  // The note renders last and counts with the system bucket, so it is
  // cut off the end before the map or the tools can claim it.
  const note = notes.find(n => n && text.endsWith(`\n\n${n}`))
  const end = note ? text.length - note.length - 2 : text.length
  const cuts = [
    { bucket: "system", from: 0 },
    ...(tools >= 0 ? [{ bucket: "tools", from: tools }] : []),
    ...(map >= 0 ? [{ bucket: "map", from: map }] : []),
  ]
  const parts = cuts.map((cut, k) => ({
    bucket: cut.bucket,
    text: text.slice(cut.from, Math.min(cuts[k + 1]?.from ?? end, end)),
  }))
  if (note) parts.push({ bucket: "system", text: text.slice(end) })
  return parts
}

/// This turn's user message: fragments, then the prompt. Fragments are only
/// looked for when the message opens with one (`// path`), and they end where
/// what was asked begins — found from the end, so a fragment quoting the
/// question does not move the cut. A note in `prompt` position renders just
/// before the question and counts as prompt, so it is moved back over when its
/// words are there.
function question(text, asked, notes) {
  const where = asked ? text.lastIndexOf(asked) : -1
  if (!text.startsWith("// ") || where <= 0) return [{ bucket: "prompt", text }]
  const before = text.slice(0, where)
  const note = notes.find(n => n && before.endsWith(`${n}\n\n`))
  const start = note ? where - note.length - 2 : where
  return [
    { bucket: "code", text: text.slice(0, start) },
    { bucket: "prompt", text: text.slice(start) },
  ]
}

/// The buckets in the order the prompt renders them, which is the legend's.
export const ORDER = ["system", "tools", "map", "summaries", "history", "code", "prompt"]

/// [`paint`]'s blocks, regrouped by bucket: `[{ bucket, entries: [{ role,
/// text }] }]`, one group per bucket that has text, in [`ORDER`]. What the
/// context *is made of* rather than the order it was sent in — the system
/// text, the tools, each earlier exchange under history, this turn's code and
/// its question — which is the question somebody opening it is asking. An
/// entry keeps the role of the message it came from, so history still reads
/// as who said what.
export function group(painted) {
  const groups = new Map()
  for (const block of painted) {
    for (const part of block.parts || []) {
      if (!part.text.trim()) continue
      const entries = groups.get(part.bucket) || []
      const last = entries[entries.length - 1]
      // Two pieces of one message in one bucket read as one entry.
      if (last && last.role === block.role && last.message === block) last.text += part.text
      else entries.push({ role: block.role, text: part.text, message: block })
      groups.set(part.bucket, entries)
    }
  }
  return [...ORDER, ...[...groups.keys()].filter(b => !ORDER.includes(b))]
    .filter(bucket => groups.has(bucket))
    .map(bucket => ({
      bucket,
      entries: groups.get(bucket).map(({ role, text }) => ({ role, text: text.trim() })),
    }))
}

/// [`group`]'s groups with the native transport's tool specs added as the
/// `tools` group, one entry per tool: its name and description, then its
/// parameters. `specs` is the request's `tools` field as the trace sent it —
/// OpenAI's `[{ type: "function", function: { name, description, parameters
/// } }]`. Under the fenced transport the tools are in the prompt and there are
/// no specs, so the groups come back as they were; specs that do not parse are
/// shown as sent rather than dropped.
export function withSpecs(groups, specs) {
  if (!specs || groups.some(g => g.bucket === "tools")) return groups
  let entries
  try {
    entries = JSON.parse(specs).map(spec => {
      const { name, description, parameters } = spec.function || spec
      const head = description ? `${name} — ${description}` : String(name)
      return { role: "tools", text: `${head}\n${JSON.stringify(parameters ?? {}, null, 2)}` }
    })
  } catch {
    entries = [{ role: "tools", text: specs }]
  }
  const tools = { bucket: "tools", entries, native: true }
  const order = ORDER.indexOf("tools")
  const at = groups.findIndex(g => ORDER.indexOf(g.bucket) > order)
  return at < 0 ? [...groups, tools] : [...groups.slice(0, at), tools, ...groups.slice(at)]
}

/// What a bucket holds, one item each, where a bucket is a list of things
/// rather than one text: the tools by name, the map's files, the folded jobs,
/// each earlier message, this turn's fragments by path. The system text and
/// the question are one thing each and are not listed.
///
/// Takes [`group`]'s (or [`withSpecs`]'s) groups and returns `{ [bucket]: {
/// chars, items: [{ label, where, path, tag, title, chars, muted }] } }`. `chars` is
/// an item's part of its bucket's text, so a caller holding the bucket's count
/// can say roughly what each item costs — roughly, because the count is the
/// bucket's and the split is by characters. `where` is what tells two items
/// apart after the label (a directory, a role, a description), and `path` says
/// it is a directory, whose end is the part worth keeping; `muted` is a
/// line that stands for what was left out, and costs nothing of its own.
///
/// Read back from the text the way [`paint`] is, and with its rule: a chunk
/// that does not look like an item is not listed rather than guessed at.
export function contents(groups) {
  const found = {}
  for (const g of groups) {
    const list = LISTS[g.bucket]
    if (!list) continue
    const text = g.entries.map(e => e.text).join("\n\n")
    const items = list(g.entries, text)
    if (items.length) found[g.bucket] = { chars: text.length, items }
  }
  return found
}

const chunks = text => text.split(/\n{2,}/)

/// `crates/x/src/lib.rs:1-15` → `lib.rs:1-15` under `crates/x/src`.
const asPath = path => {
  const cut = path.lastIndexOf("/")
  return cut < 0
    ? { label: path, where: "", path: true }
    : { label: path.slice(cut + 1), where: path.slice(0, cut), path: true }
}

const firstLine = text => text.split("\n").find(line => line.trim()) || ""

/// A fragment's head: `// path` at the start of a message or after a blank
/// line, where the path has a dot or a slash in it — which is what tells it
/// from a comment inside the code it carries. A span cited instead of sent
/// says why after a dash.
const FRAGMENT_HEAD = /(?:^|\n\n)\/\/ (\S*[./]\S*)(?: — \d+ tokens, (superseded|pruned))?(?=\n|$)/g

const LISTS = {
  // `name — description` over its parameters, in the prompt and in the
  // native specs alike. The intro's `tool` fence starts with a backtick and
  // its prose has spaces, so neither reads as a name.
  tools: (_, text) => chunks(text).flatMap(chunk => {
    const [head = "", body = ""] = chunk.split("\n")
    const named = /^([A-Za-z_][\w.:-]*)(?: — (.*))?$/.exec(head)
    return named && body.trimStart().startsWith("{")
      ? [{ label: named[1], where: named[2] || "", title: head, chars: chunk.length }]
      : []
  }),
  // A file is its path on a line of its own, then its definitions indented
  // under it. The `#` lines are the map's header, and one of them says how
  // many files the budget left out.
  map: (_, text) => chunks(text).flatMap(chunk => {
    const [head = "", ...definitions] = chunk.split("\n")
    const more = /^# (\d+) more file/.exec(head)
    if (more) return [{ label: `${more[1]} more left out`, where: "", muted: true, chars: 0 }]
    if (!head || head.startsWith("#") || /^\s/.test(head)) return []
    const n = definitions.length
    return [{ ...asPath(head), title: `${head} — ${n} definition${n === 1 ? "" : "s"}`, chars: chunk.length }]
  }),
  // The objective is on the line the fold opens with, after its tag.
  summaries: entries => entries.flatMap((entry, i) => {
    const tag = /^\[(job|draft) closed\] (.*)/.exec(entry.text)
    if (!tag) return []
    const asked = entries[i - 1]?.role === "user" ? entries[i - 1].text.length : 0
    return [{ label: tag[2], where: tag[1], title: firstLine(entry.text), chars: entry.text.length + asked }]
  }),
  // A message by its first line — except a question that carried files,
  // whose first line is a file's head: it is named by what was asked, which
  // comes after them, and says how many files it carried.
  history: entries => entries.map(entry => {
    const files = entry.role === "user" ? [...entry.text.matchAll(FRAGMENT_HEAD)].length : 0
    const said = files ? entry.text.slice(entry.text.lastIndexOf("\n\n") + 2) : entry.text
    return {
      label: firstLine(said),
      where: files ? `${entry.role} · ${files} file${files === 1 ? "" : "s"}` : entry.role,
      title: entry.text.slice(0, 400),
      chars: entry.text.length,
    }
  }),
  // One item per fragment, from its head to the next one.
  code: (_, text) => {
    const heads = [...text.matchAll(FRAGMENT_HEAD)]
    return heads.map((m, i) => {
      const from = m.index + (m[0].startsWith("\n") ? 2 : 0)
      const to = heads[i + 1]?.index ?? text.length
      return { ...asPath(m[1]), tag: m[2] || "", title: m[1], chars: to - from }
    })
  },
}

/// The prompt as sent, cut at the role markers the renderer wrote
/// (`<|System|>`, `<|User|>`, `<|Assistant|>`). A prompt with no markers —
/// another renderer — is one block.
export function split(text) {
  if (!text) return []
  const found = [...text.matchAll(/<\|(\w+)\|>/g)]
  if (!found.length) return [{ role: "prompt", text }]
  return found.map((m, i) => ({
    role: m[1].toLowerCase(),
    text: text.slice(m.index + m[0].length, found[i + 1]?.index ?? text.length).trim(),
  }))
}
