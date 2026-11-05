#!/bin/sh
# First start: a `config.toml` that says how this server is reached — users
# where LUU_ORIGIN names it, else the token it names — written once, and then
# the person's to edit. Every later start leaves both alone. Arguments go to
# `luu serve`.
set -eu
state="${LUU_HOME:-/state}"

# The image's CARGO_HOME, for `docker exec … cargo`. Sessions' commands no
# longer use it (deploy/luu.toml sends theirs to the session's `.tmp`), but a
# mounted policy from before may still grant it, and a granted path has to
# exist: on an empty volume it does not until something makes it.
mkdir -p "${CARGO_HOME:-$state/cargo}"

if [ ! -f "$state/config.toml" ]; then
  if [ -n "${LUU_ORIGIN:-}" ]; then
    # A name a browser types: users, and the page asks for the setup key
    # `serve` prints below until there is an owner.
    luu config server --origin "$LUU_ORIGIN" --bind 0.0.0.0:7878
  else
    cat > "$state/config.toml" <<TOML
# Written by the container on its first start. Yours to edit: see
# RECORD/2026-10-07.a-public-luu.completed.md for every key.
[server]
exposure = "private"
bind = "0.0.0.0:7878"
token-file = "$state/token"
TOML
  fi
fi

# Made here rather than baked into the image, so no two containers share one.
# Read it with `cat $state/token`; it is never printed.
luu token "$state/token" --if-missing

exec luu serve --sandbox /etc/luu/luu.toml "$@"
