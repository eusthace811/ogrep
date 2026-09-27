#!/bin/bash
# quick.sh BIN INDEX PATTERN [flags...] -> prints best-of-5 wall ms and row count
BIN=$1; IDX=$2; PAT=$3; shift 3
C=/home/eusth/projects/Experiments/tgrep/build/corpora-pinned/rust
cd $C
best=999999
for i in 1 2 3 4 5 6; do
  s=$(date +%s%N); n=$($BIN -l --color never --no-config -j 4 "$@" --index-path $IDX -- "$PAT" . | wc -l); e=$(date +%s%N)
  t=$(( (e-s)/1000 )); [ $i -gt 1 ] && [ $t -lt $best ] && best=$t
done
printf "%8.1f ms  %6d rows  %s\n" $(echo "$best/1000" | bc -l) $n "$PAT"
