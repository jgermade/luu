#!/bin/sh
# Install, reinstall or uninstall luu on Linux and macOS, local or online.
#
#   curl -fsSL https://jgermade.github.io/luu/setup.sh | sh
#   curl -fsSL https://jgermade.github.io/luu/setup.sh | sh -s -- reinstall
#   curl -fsSL https://jgermade.github.io/luu/setup.sh | sh -s -- uninstall
#   curl -fsSL https://jgermade.github.io/luu/setup.sh | sh -s -- install --mode online \
#     --origin https://luu.example.com
#   wget -qO- https://jgermade.github.io/luu/setup.sh | sh    # where there is no curl
#
# With no command and a terminal to ask on, the first line asks what to do:
# install, and — when it finds a binary already there — reinstall or uninstall.
# With nobody to ask it installs, the answer this line has always given.
#
# An install then asks how luu will run: **local**, for you on this machine,
# which is everything below; or **online**, for people who log in — Linux with
# systemd: the binary in /usr/local/bin, a system user `luu` whose home
# /var/lib/luu is the state directory, a policy in /etc/luu/luu.toml, `luu
# config server` for the origin, luu.service, and a one-time link to finish
# in the page — mail, and the owner, proved by one code. Each step that
# needs root runs through sudo where the script is not root already. An online
# install is known afterwards by its unit: reinstall restarts it, uninstall
# removes it. See RECORD/2026-11-03.local-or-online.completed.md and
# RECORD/2026-11-03.an-owner-proved-by-email.WIP.md.
#
# One binary, from a GitHub release, into ~/.local/bin (or --dir). No root, and
# nothing outside that directory is written: no shell profile is edited, and
# when the directory is not on PATH the line to add is printed instead.
# `uninstall` removes the binary and keeps the state directory — sessions, keys,
# config.toml, downloaded engines — unless `--purge` says otherwise.
#
# Everything is a function and the last line calls `main`, so a download that
# was cut short runs nothing. POSIX sh, because `| sh` is dash on Debian.
#
# Downloads with curl, or with wget where there is no curl — a minimal Debian
# ships wget alone. See RECORD/2026-10-23.an-installer-and-a-landing.completed.md
# and RECORD/2026-10-30.three-ways-in.WIP.md.

set -eu

REPO="jgermade/luu"
SETUP_URL="https://jgermade.github.io/luu/setup.sh"
# Online: the unit, the service's home (its state directory) and its policy.
UNIT="/etc/systemd/system/luu.service"
SERVICE_HOME="/var/lib/luu"
POLICY="/etc/luu/luu.toml"
TMP=""
# `curl` or `wget`: what downloads, and what the lines this prints say to run.
GET=

usage() {
	cat <<EOF
luu setup: install, reinstall or uninstall luu on Linux and macOS.

Usage:
  curl -fsSL $SETUP_URL | sh -s -- [command] [options]
  wget -qO- $SETUP_URL | sh -s -- [command] [options]
  sh setup.sh [command] [options]

Commands:
  install      Install the latest release. Does nothing when that version is
               already there; replaces an older one.
  reinstall    Download and replace the binary, even if it is the same version.
  uninstall    Remove the binary — and, online, the service. The state
               directory is kept unless --purge.
  help         Show this text.

With no command, the script asks which of these to do, offering reinstall and
uninstall only when it finds a binary in the install directory. With no
terminal to ask on it installs.

Options:
  --mode MODE      local: for you, on this machine (the default). online: for
                   people who log in — a systemd service, Linux only, which
                   needs root. (or LUU_MODE)
  --origin URL     Online: what a browser types, https://luu.example.com.
  --dir DIR        Where the binary goes. Default: ~/.local/bin, or
                   /usr/local/bin online. (or LUU_INSTALL_DIR)
  --version TAG    A release tag, such as v0.1.1. Default: the latest one.
                   (or LUU_VERSION)
  --purge          With uninstall: also delete luu's state directory, which
                   holds sessions, keys, config.toml, engines and models —
                   online, /var/lib/luu, the user luu and /etc/luu.
  -y, --yes        Do not ask before deleting anything.
  -h, --help       Show this text.

Environment:
  LUU_DOWNLOAD_URL   A directory holding luu-<target>.tar.gz and SHA256SUMS,
                     used instead of GitHub: a mirror, or file:// for a build
                     of your own.
EOF
}

say() { printf '%s\n' "$*"; }
warn() { printf 'luu setup: warning: %s\n' "$*" >&2; }
die() {
	printf 'luu setup: error: %s\n' "$*" >&2
	exit 1
}
need() { command -v "$1" >/dev/null 2>&1; }

# as_root CMD...: run as root, through sudo when this is not root already.
as_root() {
	if [ "$(id -u)" = 0 ]; then
		"$@"
	elif need sudo; then
		sudo "$@"
	else
		die "this step needs root and there is no sudo: run the script as root"
	fi
}

# as_luu CMD...: run as the service's user.
as_luu() {
	if [ "$(id -u)" = 0 ] && need runuser; then
		runuser -u luu -- "$@"
	else
		as_root sudo -u luu -- "$@"
	fi
}

# in_dir CMD...: run as whoever may write DIR. Local never asks for root: a
# directory it cannot write is the error it always was. Online asks for it
# where the nearest directory that exists — DIR, or the first parent that
# does — is not this user's to write.
in_dir() {
	if [ "$MODE" != online ]; then
		"$@"
		return
	fi
	d=$DIR
	while [ ! -e "$d" ]; do d=$(dirname "$d"); done
	if [ -w "$d" ]; then
		"$@"
	else
		as_root "$@"
	fi
}

# Whether online can work here: Linux, with systemd running.
online_possible() {
	[ "$(uname -s)" = Linux ] && [ -d /run/systemd/system ] && need systemctl
}

# Which of curl and wget downloads: curl where there is one, nothing where
# there is neither — uninstall needs no download.
pick_getter() {
	if need curl; then
		GET=curl
	elif need wget; then
		GET=wget
	fi
}

# The line that runs this script again, with what downloaded it.
again() {
	case "$GET" in
	wget) printf 'wget -qO- %s | sh -s -- %s' "$SETUP_URL" "$*" ;;
	*) printf 'curl -fsSL %s | sh -s -- %s' "$SETUP_URL" "$*" ;;
	esac
}

# fetch URL FILE: false on any failure, a 404 included. wget is BusyBox's on
# some systems, which has no --tries, so it is tried three times here.
fetch() {
	case "$GET" in
	wget)
		for _ in 1 2 3; do
			wget -q -T 20 -O "$2" "$1" && return 0
		done
		rm -f "$2"
		return 1
		;;
	*) curl -fsSL --retry 3 --connect-timeout 20 -o "$2" "$1" ;;
	esac
}

# The URL a redirect ends at — what `latest` is answered with.
redirected() {
	case "$GET" in
	wget)
		# Every hop's headers, the first `Location` naming a tag is the answer.
		wget -S --spider -T 20 -O /dev/null "$1" 2>&1 |
			awk 'tolower($1) == "location:" && $2 ~ /\/releases\/tag\// { sub(/\r$/, "", $2); print $2; exit }'
		;;
	*) curl -fsSLI --retry 3 --connect-timeout 20 -o /dev/null -w '%{url_effective}' "$1" ;;
	esac
}

# The release target this machine runs: the triple in the asset's name.
detect_target() {
	os=$(uname -s)
	arch=$(uname -m)
	case "$os" in
	Linux) os=unknown-linux-gnu ;;
	Darwin) os=apple-darwin ;;
	*) die "luu is built for Linux and macOS, and this is $os" ;;
	esac
	case "$arch" in
	x86_64 | amd64) arch=x86_64 ;;
	arm64 | aarch64) arch=aarch64 ;;
	*) die "there is no luu build for $arch: build it from source (https://github.com/$REPO)" ;;
	esac
	# A shell under Rosetta says x86_64 on Apple Silicon, and the native build
	# is the one worth running there.
	if [ "$os" = apple-darwin ] && [ "$arch" = x86_64 ] &&
		[ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
		arch=aarch64
	fi
	if [ "$os" = unknown-linux-gnu ] && ldd --version 2>&1 | grep -qi musl; then
		die "the Linux builds link glibc and this system uses musl: build luu from source (https://github.com/$REPO)"
	fi
	TARGET="$arch-$os"
}

# Sets TAG and BASE: which release, and where its files are. With
# LUU_DOWNLOAD_URL there is no tag to ask GitHub about, and the version is
# whatever the downloaded binary says it is.
resolve_release() {
	if [ -n "${LUU_DOWNLOAD_URL:-}" ]; then
		TAG=""
		BASE=${LUU_DOWNLOAD_URL%/}
		return
	fi
	if [ -n "$VERSION" ]; then
		case "$VERSION" in
		v*) TAG=$VERSION ;;
		*) TAG=v$VERSION ;;
		esac
	else
		# The redirect GitHub answers `latest` with names the tag, and costs
		# none of the API's rate limit.
		latest=$(redirected "https://github.com/$REPO/releases/latest") ||
			die "could not reach GitHub to find the latest release"
		TAG=${latest##*/}
		case "$TAG" in
		v[0-9]*) ;;
		*) die "GitHub has no release of luu to install" ;;
		esac
	fi
	BASE="https://github.com/$REPO/releases/download/$TAG"
}

# "0.1.1" for a luu binary, or nothing.
version_of() {
	"$1" --version </dev/null 2>/dev/null | awk '{ print $2; exit }'
}

sha256() {
	if need sha256sum; then
		sha256sum "$1" | awk '{ print $1 }'
	elif need shasum; then
		shasum -a 256 "$1" | awk '{ print $1 }'
	elif need openssl; then
		openssl dgst -sha256 "$1" | awk '{ print $NF }'
	fi
}

# The sum the release publishes for an asset: its SHA256SUMS, or, for a release
# made before it had one, the digest GitHub keeps for every asset — the same
# digest luu checks the engines it downloads against. Nothing when neither is
# there.
expected_sum() {
	if fetch "$BASE/SHA256SUMS" "$TMP/SHA256SUMS" 2>/dev/null; then
		sum=$(awk -v f="$1" '$2 == f || $2 == "*" f { print $1; exit }' "$TMP/SHA256SUMS")
		[ -n "$sum" ] || die "$BASE/SHA256SUMS does not list $1"
		printf '%s\n' "$sum"
		return
	fi
	[ -n "$TAG" ] || return 0
	fetch "https://api.github.com/repos/$REPO/releases/tags/$TAG" "$TMP/release.json" 2>/dev/null || return 0
	awk -v name="\"name\": \"$1\"" '
		index($0, name) { found = 1 }
		found && /"digest": "sha256:/ { sub(/.*"sha256:/, ""); sub(/".*/, ""); print; exit }
		found && /"browser_download_url"/ { exit }
	' "$TMP/release.json"
}

# What to type when the directory is not on PATH, and a warning when another
# luu shadows this one.
path_hint() {
	case ":${PATH:-}:" in
	*":$DIR:"*)
		found=$(command -v luu 2>/dev/null || true)
		if [ -n "$found" ] && [ "$found" != "$DIR/luu" ]; then
			warn "$found comes first on PATH, so \`luu\` runs that one and not $DIR/luu"
		fi
		;;
	*)
		say ""
		say "$DIR is not on your PATH. Add it with:"
		case "${SHELL:-}" in
		*/zsh) say "  echo 'export PATH=\"$DIR:\$PATH\"' >> ~/.zshrc" ;;
		*/bash)
			rc=.bashrc
			if [ "$(uname -s)" = Darwin ]; then rc=.bash_profile; fi
			say "  echo 'export PATH=\"$DIR:\$PATH\"' >> ~/$rc"
			;;
		*/fish) say "  fish_add_path $DIR" ;;
		*) say "  export PATH=\"$DIR:\$PATH\"" ;;
		esac
		say "and open a new terminal."
		;;
	esac
}

already_installed() {
	say "luu $1 is already installed at $DIR/luu."
	say "To download it again: $(again reinstall)"
	path_hint
}

install_luu() {
	force=$1
	[ -n "$GET" ] || die "curl or wget is needed to download luu"
	need tar || die "tar is needed to unpack luu"
	detect_target
	resolve_release

	current=""
	if [ -x "$DIR/luu" ]; then current=$(version_of "$DIR/luu"); fi
	if [ "$force" = 0 ] && [ -n "$current" ] && [ "v$current" = "$TAG" ]; then
		already_installed "$current"
		return
	fi

	TMP=$(mktemp -d "${TMPDIR:-/tmp}/luu-setup.XXXXXX")
	asset="luu-$TARGET.tar.gz"
	say "Downloading luu ${TAG:+$TAG }for $TARGET..."
	if ! fetch "$BASE/$asset" "$TMP/$asset"; then
		die "could not download $BASE/$asset.
${TAG:+Release $TAG may have no build for $TARGET: see https://github.com/$REPO/releases/tag/$TAG
}To build it from source instead:
  cargo install --locked --git https://github.com/$REPO luu"
	fi

	want=$(expected_sum "$asset")
	if [ -z "$want" ]; then
		warn "no checksum is published for $asset, so it is installed unverified"
	else
		got=$(sha256 "$TMP/$asset")
		[ -n "$got" ] || die "no sha256sum, shasum or openssl here to check the download with"
		[ "$got" = "$want" ] || die "$asset does not match its checksum (expected $want, got $got)"
	fi

	tar -xzf "$TMP/$asset" -C "$TMP" || die "could not unpack $asset"
	new="$TMP/luu-$TARGET/luu"
	[ -f "$new" ] || die "$asset has no luu binary in it"
	chmod 755 "$new"
	# Run before it replaces anything: a binary this system cannot load — an
	# older glibc than it was linked against, say — must not take the place of
	# one that works.
	if ! "$new" --version </dev/null >"$TMP/version" 2>"$TMP/error"; then
		die "the downloaded luu does not run on this system:
$(cat "$TMP/error")"
	fi
	version=$(awk '{ print $2; exit }' "$TMP/version")
	if [ "$force" = 0 ] && [ -n "$current" ] && [ "$current" = "$version" ]; then
		already_installed "$current"
		return
	fi

	in_dir mkdir -p "$DIR" || die "could not create $DIR"
	# Copied beside the old one and renamed over it: nobody ever runs half a
	# binary, and a `luu serve` already running keeps the file it started from.
	if ! { in_dir cp "$new" "$DIR/.luu.$$" && in_dir mv -f "$DIR/.luu.$$" "$DIR/luu"; }; then
		in_dir rm -f "$DIR/.luu.$$"
		die "could not write $DIR/luu"
	fi

	if [ -z "$current" ]; then
		say "Installed luu $version at $DIR/luu."
	elif [ "$current" = "$version" ]; then
		say "Reinstalled luu $version at $DIR/luu."
	else
		say "Replaced luu $current with $version at $DIR/luu."
	fi
	path_hint
	if [ "$MODE" = online ]; then return; fi
	say ""
	say "Next:"
	say "  luu chat \"hola\"    one turn against the mock backend, no model needed"
	say "  luu serve          the web UI, on http://127.0.0.1:7878"
	say "  luu --help"
}

# One answer from the terminal, for online's two questions.
ask() {
	if ! (: </dev/tty) 2>/dev/null; then
		die "$2 is needed, and there is no terminal to ask on: pass $2"
	fi
	printf '%s: ' "$1" >/dev/tty
	read -r answer </dev/tty || answer=""
	[ -n "$answer" ] || die "$2 is needed, and nothing was typed"
	printf '%s' "$answer"
}

# Local or online, for an install with no online one here already.
choose_mode() {
	if [ -n "$MODE" ]; then return; fi
	if ! (: </dev/tty) 2>/dev/null || ! online_possible; then
		MODE=local
		return
	fi
	say ""
	say "How will luu run here?"
	say "  1) local   for you, on this machine: luu serve on 127.0.0.1, no login"
	say "  2) online  for people who log in: a service behind your domain, which"
	say "             needs root — the binary in /usr/local/bin, the user luu"
	while :; do
		printf 'Which? [1] ' >/dev/tty
		read -r answer </dev/tty || answer=1
		case "${answer:-1}" in
		1 | local) MODE=local; return ;;
		2 | online) MODE=online; return ;;
		esac
		warn "not an option"
	done
}

# The policy an online luu's sessions run under: each session's own folder,
# and the commands most machines have. Written only where there is none.
policy_text() {
	cat <<'TOML'
# The policy `luu serve` runs under as luu.service: each session's own folder
# (`.`), read and write, and the commands this machine has. Yours to edit;
# `setup.sh` writes it only where there is none.
[sandbox]
enforcement = "kernel"
network = false
commands = ["git", "rg", "ls"]

[[sandbox.paths]]
path = "."
access = "read-write"
TOML
}

unit_text() {
	cat <<UNIT
# luu serve, online. Written by setup.sh: see
# https://github.com/jgermade/luu/blob/main/RECORD/2026-11-03.local-or-online.completed.md
[Unit]
Description=luu serve, online
After=network-online.target
Wants=network-online.target

[Service]
User=luu
Group=luu
Environment=LUU_HOME=$SERVICE_HOME
WorkingDirectory=$SERVICE_HOME/work
ExecStart=$DIR/luu serve --sandbox $POLICY
Restart=on-failure
RestartSec=2
NoNewPrivileges=yes
PrivateTmp=yes
ProtectHome=yes
ProtectSystem=strict
ReadWritePaths=$SERVICE_HOME

[Install]
WantedBy=multi-user.target
UNIT
}

# The online half of an install: the user, the policy, `luu config server`,
# the unit, and the link that finishes it in the page.
go_online() {
	online_possible || die "online needs Linux with systemd running, and this machine has neither"
	[ -n "$ORIGIN" ] || ORIGIN=$(ask "What a browser types to reach luu (https://luu.example.com)" --origin)
	TMP=${TMP:-$(mktemp -d "${TMPDIR:-/tmp}/luu-setup.XXXXXX")}

	if ! id luu >/dev/null 2>&1; then
		say "Creating the system user luu, home $SERVICE_HOME..."
		as_root useradd --system --home-dir "$SERVICE_HOME" --create-home \
			--shell /usr/sbin/nologin luu || die "could not create the user luu"
	fi
	as_root mkdir -p "$SERVICE_HOME/work"
	as_root chown luu:luu "$SERVICE_HOME" "$SERVICE_HOME/work"
	as_root chmod 0750 "$SERVICE_HOME"
	if [ ! -f "$POLICY" ]; then
		policy_text >"$TMP/luu.toml"
		as_root mkdir -p "$(dirname "$POLICY")"
		as_root cp "$TMP/luu.toml" "$POLICY"
		as_root chmod 0644 "$POLICY"
		say "Wrote $POLICY."
	fi

	as_luu env LUU_HOME="$SERVICE_HOME" "$DIR/luu" config server \
		--origin "$ORIGIN" </dev/null >/dev/null || die "luu config server failed"

	unit_text >"$TMP/luu.service"
	as_root cp "$TMP/luu.service" "$UNIT"
	as_root chmod 0644 "$UNIT"
	as_root systemctl daemon-reload
	as_root systemctl enable luu >/dev/null 2>&1 || as_root systemctl enable luu
	as_root systemctl restart luu || die "luu.service did not start: systemctl status luu"
	say "luu is running as luu.service, on what [server] in $SERVICE_HOME/config.toml says."
	say ""
	say "Put TLS in front of it, for instance with Caddy:"
	say "  caddy reverse-proxy --from ${ORIGIN#*://} --to 127.0.0.1:7878"
	say ""
	say "Then finish it — how it sends mail, and you as its owner, proved by one code —"
	say "in the page this link opens (an hour, once; the QR code is the same link):"
	say ""
	as_luu env LUU_HOME="$SERVICE_HOME" "$DIR/luu" config setup-url --qr </dev/null 2>/dev/null ||
		warn "no setup link: run \`sudo -u luu env LUU_HOME=$SERVICE_HOME $DIR/luu config setup-url\`"
	say ""
	say "or in a terminal here:"
	say "  sudo -u luu env LUU_HOME=$SERVICE_HOME $DIR/luu config"
	say ""
	say "  systemctl status luu"
}

# The binary an online install runs, out of its unit.
unit_binary() {
	sed -n 's/^ExecStart=\([^ ]*\)\/luu .*/\1/p' "$UNIT" 2>/dev/null | head -n 1
}

# luu's state directories, resolved the way config.rs resolves them: LUU_HOME,
# ~/.luu and $XDG_CONFIG_HOME/luu. Every one that exists, because a machine can
# have had more than one.
state_dirs() {
	for d in "${LUU_HOME:-}" "$HOME/.luu" "${XDG_CONFIG_HOME:-$HOME/.config}/luu"; do
		if [ -n "$d" ] && [ -d "$d" ]; then printf '%s\n' "${d%/}"; fi
	done | awk '!seen[$0]++'
}

# Yes, from --yes or from a person. Never from the pipe the script came in on:
# with nobody to ask, the answer is no.
confirm() {
	if [ "$YES" = 1 ]; then return 0; fi
	if ! (: </dev/tty) 2>/dev/null; then
		die "there is no terminal to ask on, so nothing was deleted: pass --yes"
	fi
	printf '%s [y/N] ' "$1" >/dev/tty
	read -r answer </dev/tty || answer=""
	case "$answer" in
	[yY] | [yY][eE][sS]) return 0 ;;
	*) return 1 ;;
	esac
}

# No command was typed: ask what to do, offering the destructive options only
# when there is something here to destroy. The answer becomes COMMAND.
choose_command() {
	if ! (: </dev/tty) 2>/dev/null; then
		say "There is no terminal to ask on, so: install. (--help for the commands.)"
		COMMAND=install
		return
	fi
	current=""
	if [ -x "$DIR/luu" ]; then current=$(version_of "$DIR/luu"); fi
	if [ -n "$current" ]; then
		say "luu $current is installed at $DIR/luu."
		say ""
		say "  1) Install the latest release (update if there is one)"
		say "  2) Reinstall $current"
		say "  3) Uninstall — keeps luu's state directory"
		say "  4) Quit"
	else
		say "luu is not installed in $DIR."
		say ""
		say "  1) Install the latest release"
		say "  2) Quit"
	fi
	while :; do
		printf 'What should this do? [1] ' >/dev/tty
		read -r answer </dev/tty || answer=1
		[ -n "$answer" ] || answer=1
		case "$answer" in
		q | Q) answer=quit ;;
		esac
		if [ -n "$current" ]; then
			case "$answer" in
			1) COMMAND=install; return ;;
			2) COMMAND=reinstall; return ;;
			3) COMMAND=uninstall; return ;;
			4 | quit)
				say "Nothing to do."
				exit 0
				;;
			esac
		else
			case "$answer" in
			1) COMMAND=install; return ;;
			2 | quit)
				say "Nothing to do."
				exit 0
				;;
			esac
		fi
		warn "not an option"
	done
}

uninstall_luu() {
	if [ "$MODE" = online ] && [ -f "$UNIT" ]; then
		as_root systemctl disable --now luu >/dev/null 2>&1 || true
		as_root rm -f "$UNIT"
		as_root systemctl daemon-reload || true
		say "Stopped and removed luu.service."
	fi
	if [ -e "$DIR/luu" ] || [ -L "$DIR/luu" ]; then
		in_dir rm -f "$DIR/luu" || die "could not remove $DIR/luu"
		say "Removed $DIR/luu."
	else
		other=$(command -v luu 2>/dev/null || true)
		if [ -n "$other" ]; then
			say "There is no luu in $DIR. The one on PATH, $other, is left alone: pass --dir $(dirname "$other") to remove that one."
		else
			say "luu is not installed in $DIR."
		fi
	fi

	if [ "$MODE" = online ]; then
		if [ "$PURGE" = 0 ]; then
			say "Kept the service's state (sessions, users, config.toml) in $SERVICE_HOME,"
			say "the user luu and $POLICY. To delete them as well: $(again uninstall --purge)"
			return
		fi
		say "--purge deletes $SERVICE_HOME (sessions, users, config.toml), the user luu and /etc/luu."
		if ! confirm "Delete them?"; then
			say "Nothing deleted."
			return
		fi
		as_root rm -rf "$SERVICE_HOME" /etc/luu
		if id luu >/dev/null 2>&1; then as_root userdel luu || warn "could not remove the user luu"; fi
		say "Deleted $SERVICE_HOME, /etc/luu and the user luu."
		return
	fi

	dirs=$(state_dirs)
	if [ -z "$dirs" ]; then
		return
	fi
	if [ "$PURGE" = 0 ]; then
		say "Kept luu's state (sessions, keys, config.toml, engines) in:"
		printf '%s\n' "$dirs" | while IFS= read -r d; do say "  $d"; done
		say "To delete it as well: $(again uninstall --purge)"
		return
	fi

	# Only LUU_HOME can name one of these, and it is checked before anything is
	# listed, let alone asked about.
	printf '%s\n' "$dirs" | while IFS= read -r d; do
		case "$d" in
		/ | "${HOME%/}") die "LUU_HOME is $d, which --purge will not delete" ;;
		esac
	done
	say "--purge deletes luu's state: sessions, keys, config.toml, engines and models."
	printf '%s\n' "$dirs" | while IFS= read -r d; do
		say "  $d ($(du -sh "$d" 2>/dev/null | awk '{ print $1 }'))"
	done
	if ! confirm "Delete them?"; then
		say "Nothing deleted."
		return
	fi
	printf '%s\n' "$dirs" | while IFS= read -r d; do
		rm -rf "$d" || die "could not delete $d"
		say "Deleted $d."
	done
}

main() {
	pick_getter
	COMMAND=install
	EXPLICIT=0
	DIR=${LUU_INSTALL_DIR:-}
	VERSION=${LUU_VERSION:-}
	MODE=${LUU_MODE:-}
	ORIGIN=""
	PURGE=0
	YES=0
	while [ $# -gt 0 ]; do
		case "$1" in
		install | reinstall | uninstall)
			COMMAND=$1
			EXPLICIT=1
			;;
		help | -h | --help)
			usage
			return
			;;
		--dir)
			[ $# -ge 2 ] || die "--dir needs a directory"
			DIR=$2
			shift
			;;
		--dir=*) DIR=${1#*=} ;;
		--version)
			[ $# -ge 2 ] || die "--version needs a tag, such as v0.1.1"
			VERSION=$2
			shift
			;;
		--version=*) VERSION=${1#*=} ;;
		--mode)
			[ $# -ge 2 ] || die "--mode needs local or online"
			MODE=$2
			shift
			;;
		--mode=*) MODE=${1#*=} ;;
		--origin)
			[ $# -ge 2 ] || die "--origin needs a URL"
			ORIGIN=$2
			shift
			;;
		--origin=*) ORIGIN=${1#*=} ;;
		--purge) PURGE=1 ;;
		-y | --yes) YES=1 ;;
		*) die "unknown argument: $1 (see --help)" ;;
		esac
		shift
	done

	[ -n "${HOME:-}" ] || die "HOME is not set"
	if [ "$PURGE" = 1 ] && [ "$COMMAND" != uninstall ]; then
		die "--purge goes with uninstall"
	fi
	case "$MODE" in
	"" | local | online) ;;
	*) die "--mode is local or online, not $MODE" ;;
	esac
	# An online install is known by its unit, and lives where its unit says.
	if [ -f "$UNIT" ] && [ "$MODE" != local ]; then
		MODE=online
		[ -n "$DIR" ] || DIR=$(unit_binary)
	fi
	[ "$MODE" != online ] || [ -n "$DIR" ] || DIR=/usr/local/bin
	[ -n "$DIR" ] || DIR="$HOME/.local/bin"
	# A `--dir=~/bin` reaches here unexpanded: the shell expands a tilde after
	# `=` only in an assignment.
	# shellcheck disable=SC2088
	case "$DIR" in
	"~") DIR=$HOME ;;
	"~/"*) DIR="$HOME/${DIR#"~/"}" ;;
	/*) ;;
	*) DIR="$(pwd)/$DIR" ;;
	esac
	DIR=${DIR%/}
	if [ "$EXPLICIT" = 0 ]; then choose_command; fi
	# A first install asks where it will run; online moves the binary.
	if [ "$COMMAND" = install ] && [ ! -f "$UNIT" ]; then
		choose_mode
		if [ "$MODE" = online ] && [ -z "${LUU_INSTALL_DIR:-}" ] && [ "$DIR" = "$HOME/.local/bin" ]; then
			DIR=/usr/local/bin
		fi
	fi

	trap '[ -z "$TMP" ] || rm -rf "$TMP"' EXIT
	trap 'exit 130' INT
	trap 'exit 143' TERM

	case "$COMMAND" in
	install | reinstall)
		force=0
		[ "$COMMAND" = install ] || force=1
		install_luu "$force"
		if [ "$MODE" = online ] && [ -f "$UNIT" ]; then
			as_root systemctl restart luu && say "Restarted luu.service."
		elif [ "$MODE" = online ]; then
			go_online
		fi
		;;
	uninstall) uninstall_luu ;;
	esac
}

main "$@"
