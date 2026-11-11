#!/bin/sh
# First start: a `config.toml` that says how this server is reached — users
# where LUU_ORIGIN names it, else the token it names — written once, and then
# the person's to edit. Every later start leaves both alone. Arguments go to
# `luu serve`.
set -eu
state="${LUU_HOME:-/state}"

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

# No policy file: a session is made from the lists in config.toml (Settings →
# Policies), chosen when it starts, and from none it asks.
exec luu serve "$@"
