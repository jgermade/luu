// @ts-check
/// Brand marks: which one a provider or a model wears, and where it is drawn
/// from — the pack `[ui] brand-icons` names, then the page's own few in
/// `web/brands/`. Nothing found is no mark, never a wrong one. See
/// `crate::brands` and `RECORD/2026-10-26.brand-icons.completed.md`.
import { $reactive } from "@web/vendor/jq79.js"
import { apiHeaders } from "./store.js"

/// The page's own, by slug: the files in `web/brands/`.
const BUILTIN = new Set([
  "anthropic", "claude", "deepseek", "gemini", "google", "huggingface", "kimi",
  "llamacpp", "lmstudio", "meta", "mistral", "mlx", "ollama", "qwen", "vllm",
])
export const builtinCount = BUILTIN.size

/// `view` is what `/api/brands` answered — the pack's `slugs` and `revision`,
/// and Settings' half — or `null` until it has.
export const brands = $reactive({ view: null })

let asked = null
/// Asked once per page, by whoever draws a mark first; again after a write.
export function loadBrands() {
  asked ??= fetch("./api/brands", { headers: apiHeaders() })
    .then(res => (res.ok ? res.json() : null))
    // A static twin has no server to ask, and draws the page's own.
    .catch(() => null)
    .then(view => { brands.view = view; return view })
  return asked
}

async function answered(res) {
  if (!res.ok) return { ok: false, error: (await res.text()) || `${res.status}` }
  const view = await res.json()
  brands.view = view
  asked = Promise.resolve(view)
  return { ok: true, view }
}

/// Draws this pack instead of the page's own, or the page's own with `null`.
export async function saveBrands(path) {
  return answered(await fetch("./api/brands", {
    method: "PUT",
    headers: { ...apiHeaders(), "Content-Type": "application/json" },
    body: JSON.stringify({ "brand-icons": path }),
  }))
}

/// A picked folder or zip of marks, as `themeFromFiles` makes one — `{ path,
/// blob }` under one folder — imported into luu's folder and chosen.
export async function importBrandFiles(entries) {
  const form = new FormData()
  for (const { path, blob } of entries) form.append("file", blob, path)
  return answered(await fetch("./api/brands/import", { method: "POST", headers: apiHeaders(), body: form }))
}

/// The URL of the first of `slugs` there is a mark for: the pack's, then the
/// page's own. `null` when neither has one.
export function brandSrc(slugs, view = brands.view) {
  for (const slug of slugs || []) {
    if (view?.slugs?.includes(slug)) return `./api/brands/icon/${slug}.svg?r=${view.revision || 0}`
    if (BUILTIN.has(slug)) return `./brands/${slug}.svg`
  }
  return null
}

/// An engine's marks, by its kind: llama.cpp's, mlx-serve's and Ollama's own.
const ENGINES = { llama: ["llamacpp", "ggml"], mlx: ["mlx"], ollama: ["ollama"] }
export const engineBrand = kind => ENGINES[kind] || []

/// Hosts that say whose API a URL is. A local server says nothing: an
/// OpenAI-compatible `127.0.0.1` is a `llama-server` far more often than it
/// is OpenAI, and a port is a flag anybody passes.
const HOSTS = [
  [/(^|\.)anthropic\.com$/, ["anthropic"]],
  [/(^|\.)openai\.com$/, ["openai"]],
  [/(^|\.)deepseek\.com$/, ["deepseek"]],
  [/(^|\.)minimaxi?\.(io|com|chat)$/, ["minimax"]],
  [/(^|\.)(moonshot\.(cn|ai)|kimi\.(com|ai))$/, ["kimi", "moonshot"]],
  [/(^|\.)(bigmodel\.cn|z\.ai)$/, ["zhipu"]],
  [/(^|\.)mistral\.ai$/, ["mistral"]],
  [/(^|\.)googleapis\.com$/, ["gemini", "google"]],
  [/(^|\.)huggingface\.co$/, ["huggingface"]],
  [/(^|\.)openrouter\.ai$/, ["openrouter"]],
  [/(^|\.)groq\.com$/, ["groq"]],
  [/(^|\.)together\.(ai|xyz)$/, ["together"]],
]

/// A provider's marks, best first: ollama's backend is ollama's, anything else
/// by its URL's host.
export function providerBrand(profile) {
  if (!profile) return []
  const backend = profile.backend || "ollama"
  if (backend === "ollama") return ["ollama"]
  if (backend === "mock") return []
  let host = ""
  try { host = new URL(profile.url || "").hostname.toLowerCase() } catch { /* no URL */ }
  return HOSTS.find(([pattern]) => pattern.test(host))?.[1] || []
}

/// A model's family, by its name. In order: a DeepSeek distilled on Qwen is
/// DeepSeek's, and a model named for two families is the first one's.
const MODELS = [
  [/^claude/, ["claude", "anthropic"]],
  [/^(gpt-|chatgpt|o[1-9](-|$)|text-embedding)/, ["openai"]],
  [/deepseek/, ["deepseek"]],
  [/qwen|qwq/, ["qwen"]],
  [/gemma/, ["gemma", "google"]],
  [/gemini/, ["gemini", "google"]],
  [/llama/, ["meta"]],
  [/mistral|mixtral|codestral|devstral|ministral|magistral|pixtral/, ["mistral"]],
  [/kimi|moonshot/, ["kimi", "moonshot"]],
  [/minimax|abab/, ["minimax"]],
  [/glm/, ["zhipu", "chatglm"]],
  [/^phi[-\d]/, ["microsoft"]],
]
const SOURCES = ["ollama", "llama.cpp", "huggingface", "luu"]

/// A model's marks, best first, by its name — after the catalog's
/// `ollama:` or `huggingface:<org>/` prefix, the org included, so
/// `huggingface:mlx-community/Qwen3-4B-4bit` is Qwen's.
export function modelBrand(name) {
  let text = String(name || "").toLowerCase()
  const at = text.indexOf(":")
  if (at > 0 && SOURCES.includes(text.slice(0, at))) text = text.slice(at + 1)
  const base = text.split("/").pop() || ""
  return MODELS.find(([pattern]) => pattern.test(base))?.[1]
    || MODELS.find(([pattern]) => pattern.test(text))?.[1]
    || []
}
