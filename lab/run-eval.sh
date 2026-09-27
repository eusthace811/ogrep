#!/bin/bash
set -e
cd /home/eusth/projects/Experiments/tgrep/ogrep
B=/home/eusth/projects/Experiments/tgrep/build/corpora-pinned
OUT=lab/results/$1; mkdir -p $OUT; R=${2:-5}
ENG="--engine tgrep=lab/bin/tgrep-1.0.10 --engine zgrep=lab/bin/zgrep-0.3.0 --engine ogrep=lab/bin/$3"
for mode in local server; do
  python3 scripts/evaluate.py $ENG --corpus $B/rust --expected-revision 1a8fa555801329bd0e803d7384b5a21191c61f30 --queries evidence/queries-upstream-rust.json --work lab/work/eval-$1/rust --mode $mode --rounds $R --seed 273337 --output $OUT/rust-broad-$mode.json
  python3 scripts/evaluate.py $ENG --corpus $B/rust --expected-revision 1a8fa555801329bd0e803d7384b5a21191c61f30 --queries evidence/queries-rust.json --work lab/work/eval-$1/rust --mode $mode --rounds $R --seed 273337 --output $OUT/rust-targeted-$mode.json
  python3 scripts/evaluate.py $ENG --corpus $B/cpython --expected-revision b54ae4cdfce4b84b22889b5c5b9ed0ab9bcb3702 --queries evidence/queries-cpython.json --work lab/work/eval-$1/cpython --mode $mode --rounds $R --seed 273337 --output $OUT/cpython-targeted-$mode.json
done
echo DONE
