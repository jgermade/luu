// Who is logged in, on a server with users — asked once, before the page
// mounts anything. Everywhere else (loopback, a token, a static copy of the
// page) the answer is `{ users: false }` and the page is what it always was.
// See `RECORD/2026-11-03.a-user-a-code-and-a-folder.completed.md`.

// `owner`: whether the server has one yet — none means setup, not a login.
// `mail`: whether a code can come by email.
export const account = { users: false, loggedIn: false, owner: true, mail: false, email: null, role: null }

export async function whoAmI() {
  try {
    // `.json`, the one spelling a static copy of the page can answer too:
    // `luu export` writes `{ "users": false }` there.
    const response = await fetch("./api/auth/me.json", { credentials: "same-origin" })
    if (response.status === 200 || response.status === 401) {
      const said = await response.json()
      Object.assign(account, said, { loggedIn: response.status === 200 })
    }
  } catch {
    // No server behind the page, or one from before users: nothing to ask.
  }
  return account
}

// A code exchanged for a login. `null` when it worked, else what to say.
export async function logIn(email, code) {
  let response
  try {
    response = await fetch("./api/auth/login", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email, code }),
    })
  } catch {
    return "the server did not answer"
  }
  if (response.ok) return null
  if (response.status === 401) return "That code is not valid. Ask for a new one."
  return (await response.text()) || `the server answered ${response.status}`
}

export async function logOut() {
  try {
    await fetch("./api/auth/logout", { method: "POST", credentials: "same-origin" })
  } finally {
    location.reload()
  }
}

async function post(path, body) {
  try {
    const response = await fetch(path, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    })
    return { ok: response.ok, status: response.status, text: await response.text() }
  } catch {
    return { ok: false, status: 0, text: "the server did not answer" }
  }
}

// A code by email. The server answers the same whoever asked, so this says
// only that the request went.
export async function emailCode(email) {
  const sent = await post("./api/auth/code", { email })
  return sent.ok ? null : sent.text || `the server answered ${sent.status}`
}

// Setup in the page: the first owner's code, sent under the mail settings as
// typed — the test of them. `null` when it went, else why not.
export async function setupSend(fields) {
  const sent = await post("./api/setup/send", fields)
  return sent.ok ? null : sent.text || `the server answered ${sent.status}`
}

// The code back: the owner made, logged in, and the mail settings saved.
export async function setupClaim(fields) {
  const sent = await post("./api/setup/claim", fields)
  if (sent.ok) return null
  if (sent.status === 401) return "That code is not valid. Send another."
  return sent.text || `the server answered ${sent.status}`
}

// `{ ok, view }` or `{ ok: false, error }`, for Settings → Users and the
// account's logins. See `RECORD/2026-11-05.roles-in-the-page.completed.md`.
async function call(method, path, body) {
  try {
    const response = await fetch(path, {
      method,
      credentials: "same-origin",
      headers: body ? { "Content-Type": "application/json" } : {},
      body: body ? JSON.stringify(body) : undefined,
    })
    const text = await response.text()
    if (!response.ok) return { ok: false, error: text || `the server answered ${response.status}` }
    return { ok: true, view: text ? JSON.parse(text) : null }
  } catch {
    return { ok: false, error: "the server did not answer" }
  }
}

// Whether this login manages people: an admin or an owner.
export const managesPeople = () => account.role === "admin" || account.role === "owner"

export const listUsers = () => call("GET", "./api/users")
export const addUser = (email, role) => call("POST", "./api/users", { email, role })
export const editUser = (id, edit) => call("PUT", `./api/users/${encodeURIComponent(id)}`, edit)
export const removeUser = id => call("DELETE", `./api/users/${encodeURIComponent(id)}`)

export const listLogins = () => call("GET", "./api/account/logins")
export const endLogin = id => call("POST", `./api/account/logins/${encodeURIComponent(id)}/end`)
export const endOtherLogins = () => call("POST", "./api/account/logins/end-others")

// Passkeys: WebAuthn in the browser, the server's ceremonies at either end.
// The options come and the credential goes as JSON, with every buffer in
// base64url, converted here rather than with the browsers' own
// `parseCreationOptionsFromJSON`, which is not everywhere yet. See
// `RECORD/2026-11-05.passkeys.completed.md`.
export const passkeysHere = () => Boolean(account.passkeys && window.PublicKeyCredential && navigator.credentials)

const fromB64 = text => {
  const plain = text.replace(/-/g, "+").replace(/_/g, "/")
  const padded = plain + "=".repeat((4 - (plain.length % 4)) % 4)
  return Uint8Array.from(atob(padded), c => c.charCodeAt(0)).buffer
}
const toB64 = buffer => btoa(String.fromCharCode(...new Uint8Array(buffer)))
  .replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "")

const creationOptions = ({ publicKey }) => ({
  ...publicKey,
  challenge: fromB64(publicKey.challenge),
  user: { ...publicKey.user, id: fromB64(publicKey.user.id) },
  excludeCredentials: (publicKey.excludeCredentials || []).map(c => ({ ...c, id: fromB64(c.id) })),
})
const requestOptions = ({ publicKey }) => ({
  ...publicKey,
  challenge: fromB64(publicKey.challenge),
  allowCredentials: (publicKey.allowCredentials || []).map(c => ({ ...c, id: fromB64(c.id) })),
})

// What the browser said, as the server reads it. `null` when the person
// turned the browser's dialog down, which is not an error to show.
async function ask(how) {
  try {
    return await how()
  } catch (error) {
    if (error?.name === "NotAllowedError" || error?.name === "AbortError") return null
    throw error
  }
}

// A login with a passkey: `null` when it worked, `""` when the person
// cancelled, else what to say.
export async function logInWithPasskey() {
  const started = await call("POST", "./api/auth/passkey/start")
  if (!started.ok) return started.error
  let credential
  try {
    credential = await ask(() => navigator.credentials.get({ publicKey: requestOptions(started.view.options) }))
  } catch (error) {
    return `${error.message || error}`
  }
  if (!credential) return ""
  const r = credential.response
  const finished = await call("POST", "./api/auth/passkey/finish", {
    ceremony: started.view.ceremony,
    credential: {
      id: credential.id,
      rawId: toB64(credential.rawId),
      type: credential.type,
      response: {
        authenticatorData: toB64(r.authenticatorData),
        clientDataJSON: toB64(r.clientDataJSON),
        signature: toB64(r.signature),
        userHandle: r.userHandle ? toB64(r.userHandle) : null,
      },
      extensions: {},
    },
  })
  if (finished.ok) return null
  return "That passkey did not log you in. Use the code instead."
}

// A passkey for this device, on the account logged in: `null` when it was
// added, `""` when the person cancelled, else what to say.
export async function addPasskey(name) {
  const started = await call("POST", "./api/account/passkeys/start")
  if (!started.ok) return started.error
  let credential
  try {
    credential = await ask(() => navigator.credentials.create({ publicKey: creationOptions(started.view) }))
  } catch (error) {
    return `${error.message || error}`
  }
  if (!credential) return ""
  const r = credential.response
  const finished = await call("POST", "./api/account/passkeys/finish", {
    name,
    credential: {
      id: credential.id,
      rawId: toB64(credential.rawId),
      type: credential.type,
      response: {
        attestationObject: toB64(r.attestationObject),
        clientDataJSON: toB64(r.clientDataJSON),
        transports: r.getTransports ? r.getTransports() : undefined,
      },
      extensions: {},
    },
  })
  return finished.ok ? null : finished.error
}

export const listPasskeys = () => call("GET", "./api/account/passkeys")
export const renamePasskey = (id, name) => call("PUT", `./api/account/passkeys/${encodeURIComponent(id)}`, { name })
export const removePasskey = id => call("DELETE", `./api/account/passkeys/${encodeURIComponent(id)}`)

// A name for a passkey made on this device, from what the browser says it is.
export function deviceName() {
  const ua = navigator.userAgent
  const os = /iPhone|iPad/.test(ua) ? "iPhone" : /Android/.test(ua) ? "Android"
    : /Mac OS X/.test(ua) ? "Mac" : /Windows/.test(ua) ? "Windows" : /Linux/.test(ua) ? "Linux" : "this device"
  const browser = /Edg\//.test(ua) ? "Edge" : /Firefox\//.test(ua) ? "Firefox"
    : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : ""
  return browser ? `${browser} on ${os}` : os
}
