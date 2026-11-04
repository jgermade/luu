#!/bin/sh
# phase.sh <image>: fresh volumes, two users, each a secret, bo a project, and
# luu serve in <image> on the scripted provider. The probes run after it.
set -eu
image=$1
docker rm -f luu-pub >/dev/null 2>&1 || true
docker volume rm -f luu-state luu-work >/dev/null
R="docker run --rm -v luu-state:/state -v luu-work:/work --entrypoint sh $image -c"
$R 'cat > /state/config.toml <<TOML
default = "probe"

[server]
exposure = "public"
bind = "0.0.0.0:7878"
origin = "http://127.0.0.1:7979"

[provider.probe]
backend = "openai"
url = "http://172.17.0.1:8090/v1"
model = "probe"
remote = true
TOML
luu user add ana@example.test >/dev/null && luu user add bo@example.test >/dev/null && luu user list'
$R 'a=$(luu user list | awk "/^ana@/{print \$4}"); b=$(luu user list | awk "/^bo@/{print \$4}");
mkdir -p /work/$a /work/$b/hello/src && echo ana-secret > /work/$a/secret && echo bo-secret > /work/$b/secret &&
printf "[package]\nname = \"hello\"\nversion = \"0.1.0\"\nedition = \"2021\"\n" > /work/$b/hello/Cargo.toml &&
printf "fn main() { println!(\"hello from bo\"); }\n" > /work/$b/hello/src/main.rs && echo "$a $b"'
docker run -d --name luu-pub -p 127.0.0.1:7979:7878 -v luu-state:/state -v luu-work:/work "$image" --no-store >/dev/null
sleep 3
docker logs luu-pub 2>&1 | grep -E "env |warning: .*writable|state/cargo" || true
