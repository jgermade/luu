#!/bin/sh
# run-one.sh <probe> <email>: the scripted model on that probe, then the probe as that user.
set -eu
cd /home/user/luu
python3 -I .tmp/isolation/probe.py replies "$1" > .tmp/isolation/replies.json
pkill -f fake-model.py || true
sleep 0.5
setsid nohup python3 -I .tmp/isolation/fake-model.py 8090 .tmp/isolation/replies.json > .tmp/isolation/fake-model.log 2>&1 < /dev/null &
sleep 1
docker restart luu-pub >/dev/null; sleep 3
code=$(docker exec luu-pub luu user code "$2" | tail -1)
timeout 240 .tmp/venv/bin/python -I .tmp/isolation/probe.py run "$2" "$code" > ".tmp/isolation/$1.json" 2> ".tmp/isolation/$1.err" || { tail -5 ".tmp/isolation/$1.err"; exit 1; }
python3 -c "
import json,sys; d=json.load(open(sys.argv[1]))
print('verdict:', d['verdict'].get('allowed'), d['verdict'].get('enforced_by'), '| held:', len(d['held']))
c=d['command']; print('exit', c.get('exit_code')); print(c.get('stdout')); print(c.get('stderr') or '')
" ".tmp/isolation/$1.json"
