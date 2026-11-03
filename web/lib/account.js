// Who is logged in, on a server with users — asked once, before the page
// mounts anything. Everywhere else (loopback, a token, a static copy of the
// page) the answer is `{ users: false }` and the page is what it always was.
// See `RECORD/2026-11-03.a-user-a-code-and-a-folder.WIP.md`.

export const account = { users: false, loggedIn: false, email: null, role: null }

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
