#!/usr/bin/env bash
# The installer, walked against a build: `pages/setup.sh` installs, finds it
# already there, reinstalls, refuses a download whose checksum is wrong,
# uninstalls without touching the state directory, and purges it. The release
# is a directory standing in for GitHub (`LUU_DOWNLOAD_URL=file://…`) and HOME
# is a temporary one, so nothing on the machine running it is touched.
#
# CI runs it after packaging each target its runner can execute; a person runs
# the same file:
#
#   scripts/setup-check.sh dist/luu-x86_64-unknown-linux-gnu.tar.gz
#
# The script is run with `sh`, which is dash on Ubuntu and bash in POSIX mode on
# macOS — the two shells `curl … | sh` lands in.
set -euo pipefail

archive="${1:?usage: setup-check.sh <luu-TARGET.tar.gz>}"
setup="$(cd "$(dirname "$0")/.." && pwd)/pages/setup.sh"
name="$(basename "$archive")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

release="$work/release"
mkdir -p "$release" "$work/home"
cp "$archive" "$release/"
sum() { if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }
(cd "$release" && sum "$name" >SHA256SUMS)

export HOME="$work/home" LUU_DOWNLOAD_URL="file://$release"
unset LUU_HOME XDG_CONFIG_HOME LUU_INSTALL_DIR LUU_VERSION
bin="$HOME/.local/bin/luu"

step() { printf '\n== %s\n' "$*"; }
fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

step "install"
# No command typed: with nobody to ask the script installs, which is what CI
# exercises; on a terminal it would show the menu, so ask it nothing here.
if (: </dev/tty) 2>/dev/null; then
  sh "$setup" install | tee "$work/out"
else
  sh "$setup" | tee "$work/out"
fi
grep -q '^Installed luu' "$work/out" || fail "install did not say it installed"
"$bin" --version

step "install again, which has nothing to do"
sh "$setup" install | tee "$work/out"
grep -q 'already installed' "$work/out" || fail "a second install replaced the binary"

step "reinstall"
sh "$setup" reinstall | tee "$work/out"
grep -q '^Reinstalled luu' "$work/out" || fail "reinstall did not replace the binary"

step "a download that does not match its checksum"
cp "$release/SHA256SUMS" "$work/sums"
printf '%064d  %s\n' 0 "$name" >"$release/SHA256SUMS"
if sh "$setup" reinstall; then fail "a download that does not match its checksum was installed"; fi
cp "$work/sums" "$release/SHA256SUMS"
"$bin" --version # the one already there is untouched, and still runs

step "a directory with a space in it"
sh "$setup" install --dir "$work/my bin"
"$work/my bin/luu" --version
sh "$setup" uninstall --dir "$work/my bin"
[ ! -e "$work/my bin/luu" ] || fail "uninstall --dir left the binary"

step "uninstall keeps the state directory"
mkdir -p "$HOME/.luu" && touch "$HOME/.luu/sessions.db"
sh "$setup" uninstall
[ ! -e "$bin" ] || fail "uninstall left $bin"
[ -e "$HOME/.luu/sessions.db" ] || fail "uninstall deleted the state directory without --purge"

# Only where nobody could answer: on a terminal, the script would ask.
if ! (: </dev/tty) 2>/dev/null; then
  step "--purge with nobody to ask deletes nothing"
  if sh "$setup" uninstall --purge </dev/null; then fail "--purge went ahead with no terminal and no --yes"; fi
  [ -e "$HOME/.luu/sessions.db" ] || fail "--purge deleted without asking"
fi

step "uninstall --purge --yes"
sh "$setup" uninstall --purge --yes
[ ! -e "$HOME/.luu" ] || fail "--purge --yes left $HOME/.luu"

# Online for real, where it can be: Linux with systemd running and a sudo
# that asks nothing — a GitHub runner, not a laptop. It makes the user luu,
# /var/lib/luu, /etc/luu and luu.service, so it removes them again whatever
# happens. See RECORD/2026-11-03.local-or-online.WIP.md.
if [ "$(uname -s)" = Linux ] && [ -d /run/systemd/system ] && sudo -n true 2>/dev/null; then
  step "online: a service, a user, a policy and a first owner"
  trap 'sh "$setup" uninstall --purge --yes </dev/null >/dev/null 2>&1 || true; rm -rf "$work"' EXIT
  sh "$setup" install --mode online --origin http://luu.test:7878 --owner Ci@Example.com </dev/null |
    tee "$work/out"
  grep -q '^code: ' "$work/out" || fail "online printed no code"
  answered=""
  for _ in $(seq 1 30); do
    if curl -fsS http://127.0.0.1:7878/api/auth/me.json >"$work/me" 2>/dev/null; then
      answered=1
      break
    fi
    sleep 1
  done
  if [ -z "$answered" ] || ! grep -q '"users":true' "$work/me"; then
    sudo systemctl status luu --no-pager || true
    sudo journalctl -u luu --no-pager | tail -30 || true
    fail "luu.service did not answer as a server with users"
  fi
  [ "$(sudo stat -c %a /var/lib/luu/auth.db)" = 600 ] || fail "auth.db is not 0600"
  [ "$(sudo stat -c %U /var/lib/luu/auth.db)" = luu ] || fail "auth.db is not the user luu's"

  step "online reinstall restarts the service"
  sh "$setup" reinstall </dev/null | tee "$work/out"
  grep -q 'Restarted luu.service' "$work/out" || fail "reinstall did not restart luu.service"

  step "online uninstall --purge --yes"
  sh "$setup" uninstall --purge --yes </dev/null | tee "$work/out"
  [ ! -e /etc/systemd/system/luu.service ] || fail "uninstall left the unit"
  [ ! -e /usr/local/bin/luu ] || fail "uninstall left /usr/local/bin/luu"
  if id luu >/dev/null 2>&1; then fail "--purge left the user luu"; fi
  sudo test ! -e /var/lib/luu || fail "--purge left /var/lib/luu"
else
  step "online: skipped, it needs Linux with systemd and a sudo that asks nothing"
fi

printf '\nsetup.sh: every check passed for %s\n' "$name"
