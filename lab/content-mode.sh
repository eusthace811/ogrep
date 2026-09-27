#!/bin/bash
# content-mode.sh MODE(local|server) : normal line output (-n), zgrep vs ogrep, byte-identical check + best-of-7 timing
L=/home/eusth/projects/Experiments/tgrep/ogrep/lab; C=/home/eusth/projects/Experiments/tgrep/build/corpora-pinned/rust
MODE=$1; OG=${2:-ogrep-r4}
if [ $MODE = server ]; then $L/servers.sh start zgrep-0.3.0 $OG; IZ=$L/work/srv/zgrep-0.3.0; IO=$L/work/srv/$OG; else IZ=$L/work/zgrep/index; IO=$L/work/zgrep/index; fi
cd $C
run() { "$@" > /home/eusth/projects/Experiments/tgrep/ogrep/lab/work/tmp/cm.out 2>/dev/null; }
while IFS='|' read -r pat flags; do
  for eng in zgrep-0.3.0 $OG; do
    idx=$IZ; [ $eng = $OG ] && idx=$IO
    best=999999999
    for i in 1 2 3 4 5 6 7 8; do s=$(date +%s%N); $L/bin/$eng -n --color never --no-config -j 4 $flags --index-path $idx -- "$pat" . > /home/eusth/projects/Experiments/tgrep/ogrep/lab/work/tmp/cm-$eng.out; e=$(date +%s%N); t=$(( (e-s)/1000 )); [ $i -gt 1 ] && [ $t -lt $best ] && best=$t; done
    eval "t_${eng//[-.]/_}=$best"
  done
  same=$(cmp -s /home/eusth/projects/Experiments/tgrep/ogrep/lab/work/tmp/cm-zgrep-0.3.0.out /home/eusth/projects/Experiments/tgrep/ogrep/lab/work/tmp/cm-$OG.out && echo identical || echo DIFFERENT)
  z=$t_zgrep_0_3_0; o=$(eval echo \$t_${OG//[-.]/_})
  printf "%-26s %8.1f ms %8.1f ms  %5.2fx  %6d lines  %s\n" "$pat" $(echo "$z/1000"|bc -l) $(echo "$o/1000"|bc -l) $(echo "$z/$o"|bc -l) $(wc -l < /home/eusth/projects/Experiments/tgrep/ogrep/lab/work/tmp/cm-$OG.out) $same
done <<'Q'
PhantomData|-t rust -F
Vec<|-t rust -F
unwrap()|-t rust -F
fn main|-t rust -F
impl\s+\w+\s+for|-t rust
^use std::|-t rust
TypeVisitableExt|-t rust -F
Q
[ $MODE = server ] && $L/servers.sh stop zgrep-0.3.0 $OG
true
