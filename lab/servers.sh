#!/bin/bash
# servers.sh start|stop NAME... : one server per lab/bin/NAME over a private copy of the zgrep index
L=/home/eusth/projects/Experiments/tgrep/ogrep/lab; C=/home/eusth/projects/Experiments/tgrep/build/corpora-pinned/rust
cmd=$1; shift
for b in "$@"; do
  d=$L/work/srv/$b
  if [ -f $d/serve.json ]; then kill $(python3 -c "import json;print(json.load(open('$d/serve.json'))['pid'])") 2>/dev/null; sleep 0.5; fi
  if [ "$cmd" = start ]; then
    rm -rf $d; cp -r $L/work/zgrep/index $d; rm -f $d/serve.json
    (setsid $L/bin/$b serve $C --index-path $d --no-watch -j 4 > $d.log 2>&1 &)
  fi
done
[ "$cmd" = start ] && for b in "$@"; do for i in $(seq 100); do [ -f $L/work/srv/$b/serve.json ] && break; sleep 0.1; done; done
true
