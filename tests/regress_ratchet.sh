#!/bin/bash
# The Postgres regression run (tests/regress_expected.sh) against the floor it has reached, tests/regress_floor.tsv:
# no file may match fewer of its expected lines than the floor says (less SLACK points, for a psql or a Postgres
# of another minor version; none for a file at 100%), nor may fewer files match exactly, nor may a file have more
# places where both sides succeed with different output (wrong answers, `tools/regress_split.py`) than
# tests/regress_wrong.tsv says. A run that does better says which lines of the floor to raise; `--write` raises
# them (never lowers one, and keeps a file's row where the run compared it with another of its expected files), and
# writes the wrong-answer floor from the run.
#
#   OUT=/tmp/chronos-regress tests/regress_ratchet.sh [--write]
set -u
OUT=${OUT:-/tmp/chronos-regress}
FLOOR=${FLOOR:-$(dirname "$0")/regress_floor.tsv}
SLACK=${SLACK:-1}
WRONG=${WRONG:-$(dirname "$0")/regress_wrong.tsv}
SPLIT="$(dirname "$0")/../tools/regress_split.py"
[ -s "$OUT/summary.tsv" ] || { echo "no $OUT/summary.tsv: run tests/regress_expected.sh first"; exit 2; }
# the files the run compared with another of their expected files than the floor's (a locale test skipped where the
# locale is missing: fewer lines): neither floor is theirs to judge or rewrite
variants() {
  [ -s "$FLOOR" ] || return 0
  awk -F'\t' 'NR == FNR { lines[$1] = $4; next } ($1 in lines) && lines[$1] != $4 { print $1 }' "$FLOOR" "$OUT/summary.tsv"
}
# a wrong-answer table without the variants' rows (`drop`), or with only theirs (`keep`)
rows() { awk -F'\t' -v mode="$1" 'FILENAME == ARGV[1] { v[$1] = 1; next } (mode == "keep") == ($1 in v)' "$OUT/variants.txt" "$2"; }
variants >"$OUT/variants.txt"
if [ "${1:-}" = "--write" ]; then
  # (both made first, then both put in place: a split that fails leaves the floors as they were)
  # (a file compared with another of its expected files than the floor's, as a locale test skipped where the locale
  # is missing, keeps the floor's row: the two aren't the same measure)
  { [ -s "$FLOOR" ] && cat "$FLOOR"; } | awk -F'\t' -v OFS='\t' '
    FILENAME == "-" { old[$1] = $0; lines[$1] = $4; pct[$1] = $3; next }
    # (and the floor only rises: a file within its slack below the floor keeps the floor)
    { print (($1 in old) && (lines[$1] != $4 || pct[$1] > $3)) ? old[$1] : $0 }' - "$OUT/summary.tsv" | sort >"$FLOOR.new" || exit 1
  python3 -I "$SPLIT" "$OUT" --wrong-tsv "$OUT/wrong.tsv" >/dev/null || { rm -f "$FLOOR.new"; echo "the split failed: floors unchanged"; exit 1; }
  # (a variant keeps its wrong-answer floor too, as its line floor)
  { rows drop "$OUT/wrong.tsv"; [ -f "$WRONG" ] && rows keep "$WRONG"; } | sort >"$WRONG.new"
  mv "$FLOOR.new" "$FLOOR" && mv "$WRONG.new" "$WRONG" || exit 1
  echo "floor written: $(grep -c PASS "$FLOOR") exact of $(wc -l <"$FLOOR" | tr -d ' '); wrong-answer floor: $WRONG"
  exit 0
fi
[ -s "$FLOOR" ] || { echo "no floor at $FLOOR: write one with --write"; exit 2; }
awk -F'\t' -v slack="$SLACK" '
  NR == FNR { floor[$1] = $3; was[$1] = $2; lines[$1] = $4; next }
  {
    seen[$1] = 1
    if (!($1 in floor)) { print "new:    " $1 " " $2 " " $3 "%"; next }
    # (compared with another of its expected files than the floor was, a skipped locale test: not the same measure)
    if (lines[$1] != $4) { print "other expected file: " $1 " (" lines[$1] " lines, now " $4 "), not compared"; if (was[$1] == "PASS") exact_was++; if ($2 == "PASS") exact++; next }
    s = floor[$1] >= 100 ? 0 : slack
    if ($3 + s < floor[$1]) { print "worse:  " $1 " " floor[$1] "% -> " $3 "%"; bad++ }
    else if ($3 > floor[$1]) { print "better: " $1 " " floor[$1] "% -> " $3 "%"; better++ }
    if (was[$1] == "PASS") { exact_was++; if ($2 != "PASS") { print "no longer exact: " $1; bad++ } }
    if ($2 == "PASS") exact++
  }
  END {
    for (t in floor) if (!(t in seen)) { print "missing: " t; bad++ }
    printf "exact: %d (floor %d)   worse: %d   better: %d\n", exact, exact_was, bad, better
    if (better && !bad) print "raise the floor: OUT=... tests/regress_ratchet.sh --write, and commit tests/regress_floor.tsv"
    exit bad ? 1 : 0
  }' "$FLOOR" "$OUT/summary.tsv"
lines=$?
# (wrong answers: no file may have more places where both sides succeed and the output differs than its floor)
wrong=0
# (an empty floor is none allowed anywhere, not no check)
if [ -f "$WRONG" ]; then
  python3 -I "$SPLIT" "$OUT" --wrong-tsv "$OUT/wrong.tsv" >/dev/null || exit 2
  rows drop "$OUT/wrong.tsv" >"$OUT/wrong.judged.tsv"
  awk -F'\t' '
    FILENAME == ARGV[1] { floor[$1] = $2; next }
    { if ($2 > floor[$1] + 0) { print "more wrong answers: " $1 " " floor[$1] + 0 " -> " $2; bad++ } }
    END { exit bad ? 1 : 0 }' "$WRONG" "$OUT/wrong.judged.tsv" || wrong=1
fi
exit $(( lines || wrong ))
