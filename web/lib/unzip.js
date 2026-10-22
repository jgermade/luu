// @ts-check
/// A zip, opened in the browser: its files as `{ path, blob }`, in order.
///
/// For an icon theme added as a zip (Settings → Appearance → Icons pack). The
/// server takes a theme as the files of one folder, the way a directory picker
/// sends them, so a zip is read here and sent the same way — no zip reader on
/// the server, and no dependency here: the central directory is a few fixed
/// headers, and deflate is the browser's own `DecompressionStream`. See
/// `RECORD/2026-10-21.the-design-system-applied.completed.md`.
///
/// Stored and deflated entries; folders and macOS's `__MACOSX/` are skipped.
/// ZIP64 — an archive past 4 GiB or 65 535 entries — is refused in so many
/// words: no icon theme is one.

const END = 0x06054b50
const CENTRAL = 0x02014b50
const LOCAL = 0x04034b50

/** @param {Blob} file */
export async function unzip(file) {
  const bytes = new DataView(await file.arrayBuffer())
  // The end-of-central-directory record is the last 22 bytes, unless the
  // archive has a comment after it — up to 64 KiB of one.
  let end = -1
  for (let at = bytes.byteLength - 22; at >= Math.max(0, bytes.byteLength - 22 - 0xffff); at--) {
    if (bytes.getUint32(at, true) === END) {
      end = at
      break
    }
  }
  if (end < 0) throw new Error("it is not a zip")
  const count = bytes.getUint16(end + 10, true)
  let at = bytes.getUint32(end + 16, true)
  if (count === 0xffff || at === 0xffffffff) throw new Error("it is a ZIP64 archive, which no icon theme needs")

  const names = new TextDecoder()
  const files = []
  for (let n = 0; n < count; n++) {
    if (bytes.getUint32(at, true) !== CENTRAL) throw new Error("its directory is damaged")
    const method = bytes.getUint16(at + 10, true)
    const size = bytes.getUint32(at + 20, true)
    const nameLength = bytes.getUint16(at + 28, true)
    const extraLength = bytes.getUint16(at + 30, true)
    const commentLength = bytes.getUint16(at + 32, true)
    const local = bytes.getUint32(at + 42, true)
    const path = names.decode(new Uint8Array(bytes.buffer, at + 46, nameLength))
    at += 46 + nameLength + extraLength + commentLength

    if (path.endsWith("/") || path.startsWith("__MACOSX/") || path.split("/").pop() === ".DS_Store") continue
    if (bytes.getUint32(local, true) !== LOCAL) throw new Error(`${path}: its entry is damaged`)
    // The local header's own name and extra lengths, which may differ from the
    // directory's.
    const start = local + 30 + bytes.getUint16(local + 26, true) + bytes.getUint16(local + 28, true)
    const packed = new Blob([new Uint8Array(bytes.buffer, start, size)])
    if (method === 0) {
      files.push({ path, blob: packed })
    } else if (method === 8) {
      const inflated = packed.stream().pipeThrough(new DecompressionStream("deflate-raw"))
      files.push({ path, blob: await new Response(inflated).blob() })
    } else {
      throw new Error(`${path}: compressed in a way this does not read (method ${method})`)
    }
  }
  return files
}
