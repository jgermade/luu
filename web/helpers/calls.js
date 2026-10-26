// @ts-check
/// A tool part, as the chat words it, where that is more than a field: which
/// lines a read read. A part is a `tool` segment out of `segments.js`; the
/// card is `views/chat/tool-call.html`.

/// What `read_file` returns when `max_lines` is not given: `DEFAULT_MAX_LINES`
/// in `agent-core/src/tools/fs.rs`. A read that came back with this many
/// lines may have stopped short of the end.
const READ_CAP = 400

/// Which lines a `read_file` read — `lines 120–180` — where it may not have
/// been the whole file, and `""` where it was, or for any other tool. From
/// `start_line`, and as many lines as came back; before it has come back, as
/// many as it asked for. The tool's output carries no total, so a read that
/// asked for a range is named by it even where the file was shorter.
export function readLines(part) {
  if (part?.name !== "read_file") return ""
  const args = part.arguments || {}
  const whole = value => typeof value === "number" && Number.isInteger(value) && value >= 0
  const start = whole(args.start_line) && args.start_line > 1 ? args.start_line : 1
  const asked = whole(args.max_lines) ? args.max_lines : null
  const call = part.call
  const got = call?.verdict?.allowed && !call.error && typeof call.output === "string"
    ? (call.output === "" ? 0 : call.output.split("\n").length)
    : null
  if (start === 1 && asked === null && (got === null || got < READ_CAP)) return ""
  const count = got ?? asked
  if (count === null) return `from line ${start}`
  if (count === 0) return `nothing from line ${start}`
  return count === 1 ? `line ${start}` : `lines ${start}–${start + count - 1}`
}
