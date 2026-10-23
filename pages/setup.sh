#!/bin/sh
# Install, reinstall or uninstall luu on Linux and macOS.
#
#   curl -fsSL https://jgermade.github.io/luu/setup.sh | sh
#   curl -fsSL https://jgermade.github.io/luu/setup.sh | sh -s -- reinstall
#   curl -fsSL https://jgermade.github.io/luu/setup.sh | sh -s -- uninstall
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
# See RECORD/2026-10-23.an-installer-and-a-landing.completed.md.

set -eu

REPO="jgermade/luu"
SETUP_URL="https://jgermade.github.io/luu/setup.sh"
TMP=""

usage() {
	cat <<EOF
luu setup: install, reinstall or uninstall luu on Linux and macOS.

Usage:
  curl -fsSL $SETUP_URL | sh -s -- [command] [options]
  sh setup.sh [command] [options]

Commands:
  install      Install the latest release (the default). Does nothing when
               that version is already there; replaces an older one.
  reinstall    Download and replace the binary, even if it is the same version.
  uninstall    Remove the binary. The state directory is kept unless --purge.
  help         Show this text.

Options:
  --dir DIR        Where the binary goes. Default: ~/.local/bin
                   (or LUU_INSTALL_DIR)
  --version TAG    A release tag, such as v0.1.1. Default: the latest one.
                   (or LUU_VERSION)
  --purge          With uninstall: also delete luu's state directory, which
                   holds sessions, keys, config.toml, engines and models.
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

# fetch URL FILE: false on any failure, a 404 included.
fetch() {
	curl -fsSL --retry 3 --connect-timeout 20 -o "$2" "$1"
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
		latest=$(curl -fsSLI --retry 3 --connect-timeout 20 -o /dev/null -w '%{url_effective}' \
			"https://github.com/$REPO/releases/latest") ||
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
	say "To download it again: curl -fsSL $SETUP_URL | sh -s -- reinstall"
	path_hint
}

install_luu() {
	force=$1
	need curl || die "curl is needed to download luu"
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

	mkdir -p "$DIR" || die "could not create $DIR"
	# Copied beside the old one and renamed over it: nobody ever runs half a
	# binary, and a `luu serve` already running keeps the file it started from.
	if ! { cp "$new" "$DIR/.luu.$$" && mv -f "$DIR/.luu.$$" "$DIR/luu"; }; then
		rm -f "$DIR/.luu.$$"
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
	say ""
	say "Next:"
	say "  luu chat \"hola\"    one turn against the mock backend, no model needed"
	say "  luu serve          the web UI, on http://127.0.0.1:7878"
	say "  luu --help"
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

uninstall_luu() {
	if [ -e "$DIR/luu" ] || [ -L "$DIR/luu" ]; then
		rm -f "$DIR/luu" || die "could not remove $DIR/luu"
		say "Removed $DIR/luu."
	else
		other=$(command -v luu 2>/dev/null || true)
		if [ -n "$other" ]; then
			say "There is no luu in $DIR. The one on PATH, $other, is left alone: pass --dir $(dirname "$other") to remove that one."
		else
			say "luu is not installed in $DIR."
		fi
	fi

	dirs=$(state_dirs)
	if [ -z "$dirs" ]; then
		return
	fi
	if [ "$PURGE" = 0 ]; then
		say "Kept luu's state (sessions, keys, config.toml, engines) in:"
		printf '%s\n' "$dirs" | while IFS= read -r d; do say "  $d"; done
		say "To delete it as well: curl -fsSL $SETUP_URL | sh -s -- uninstall --purge"
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
	COMMAND=install
	DIR=${LUU_INSTALL_DIR:-}
	VERSION=${LUU_VERSION:-}
	PURGE=0
	YES=0
	while [ $# -gt 0 ]; do
		case "$1" in
		install | reinstall | uninstall) COMMAND=$1 ;;
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

	trap '[ -z "$TMP" ] || rm -rf "$TMP"' EXIT
	trap 'exit 130' INT
	trap 'exit 143' TERM

	case "$COMMAND" in
	install) install_luu 0 ;;
	reinstall) install_luu 1 ;;
	uninstall) uninstall_luu ;;
	esac
}

main "$@"
