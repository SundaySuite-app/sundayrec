#!/usr/bin/env bash
# `npm run rig:mac` — nattkjøreren for Macen. Kjører de lange, maskinkjørbare
# `#[ignore]`-testene (de som IKKE trenger lydkort, kamera, nettverk eller en
# ekstern fil) og skriver en kort resultattabell til terminalen og til
# rig-results/ (git-ignorert). Den er IKKE en del av CI og ikke av `npm run check`.
#
#   npm run rig:mac             full lengde: 90 min virtuell tid (v1) + 30 s
#                               sanntid + 120 s CPU-last (veggtid: se
#                               docs/RIG-DAY.md «Nattkjøreren»)
#   npm run rig:mac -- --quick  noen få minutter totalt, for å prøve kjøreren
#
# Ordbruk som i core/selftest: Pass / Warn / Fail. Warn = testen kjørte, men
# hoppet over seg selv (SKIP, f.eks. uten ffmpeg-sidecar) og beviser da ingenting.
# Exit-kode 1 hvis NOE feilet (Fail); Warn alene gir 0.
#
# Dette er MASKINBEVIST, ikke menneske-sjekket: avkryssingsboksene i
# docs/RIG-DAY.md krysses aldri av på grunnlag av dette skriptet.
set -uo pipefail
cd "$(dirname "$0")/.."

if [ "$(uname -s)" != "Darwin" ]; then
  echo "rig:mac kjører bare på macOS (dette er $(uname -s)). Riggtestene her er Mac-delen av docs/RIG-DAY.md." >&2
  exit 2
fi

QUICK=0
for a in "$@"; do
  case "$a" in
    --quick) QUICK=1 ;;
    -h|--help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "Ukjent argument: $a (gyldig: --quick, --help)" >&2; exit 2 ;;
  esac
done

# Delt byggecache (ellers fylles disken av hvert arbeidstre).
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/sunday-work/.target-sundayrec}"
# Som ci-local.sh: ingen stille SKIP uten ffmpeg/ffprobe.
export SUNDAYREC_REQUIRE_SIDECAR=1

if [ "$QUICK" = 1 ]; then
  export SUNDAYREC_RIG_MINUTES=3 SUNDAYREC_RIG_REAL_SECS=20 SUNDAYREC_RIG_LOAD_SECS=20
  MODE="quick (3 min virtuell, 20 s sanntid, 20 s last)"
else
  export SUNDAYREC_RIG_MINUTES="${SUNDAYREC_RIG_MINUTES:-90}"
  export SUNDAYREC_RIG_REAL_SECS="${SUNDAYREC_RIG_REAL_SECS:-30}"
  export SUNDAYREC_RIG_LOAD_SECS="${SUNDAYREC_RIG_LOAD_SECS:-120}"
  MODE="full (${SUNDAYREC_RIG_MINUTES} min virtuell, ${SUNDAYREC_RIG_REAL_SECS} s sanntid, ${SUNDAYREC_RIG_LOAD_SECS} s last)"
fi

if [ ! -x src-tauri/binaries/ffprobe-aarch64-apple-darwin ] && ! ls src-tauri/binaries/ffprobe-* >/dev/null 2>&1; then
  echo "Mangler ffmpeg/ffprobe-sidecar. Kjør 'npm run ffmpeg' først." >&2
  exit 2
fi

STAMP="$(date +%Y-%m-%d-%H%M%S)"
mkdir -p rig-results
OUT="rig-results/rig-mac-$STAMP.txt"
LOGDIR="rig-results/rig-mac-$STAMP.logs"
mkdir -p "$LOGDIR"

COMMIT="$(git rev-parse --short=8 HEAD 2>/dev/null || echo ukjent)"
DIRTY=""; [ -n "$(git status --porcelain --untracked-files=no 2>/dev/null)" ] && DIRTY=" (+ lokale endringer)"
MODEL="$(sysctl -n hw.model 2>/dev/null || echo ukjent)"
CHIP="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo ukjent)"
MACOS="$(sw_vers -productVersion 2>/dev/null || echo ukjent)"

# riggpunkt | teststi (relativt til sundayrec-biblioteket) | kort navn
MOD="recorder::longrun::tests::rig_sync"
TESTS=(
  "(v1)|$MOD::the_90_minute_recording_keeps_picture_and_sound_in_step|${SUNDAYREC_RIG_MINUTES} min virtuell bilde/lyd-synk"
  "(v1)|$MOD::the_virtual_driver_agrees_with_the_real_clock|sanntidsklokke, samme driver"
  "(e2/v4)|$MOD::video_under_cpu_load_keeps_30_fps_and_counts_what_it_drops|30 fps under CPU-last"
)

# Bygg først, så byggetiden ikke havner i testtidene.
echo "▶ Bygger testbinæren (ikke med i tidene) …"
if ! cargo test -p sundayrec --lib --no-run 2>"$LOGDIR/build.log"; then
  echo "Bygget feilet, se $LOGDIR/build.log" >&2
  tail -20 "$LOGDIR/build.log" >&2
  exit 2
fi

fmt_dur() { local s=$1; printf '%dm%02ds' $((s / 60)) $((s % 60)); }

ROWS=()
FAILS=0; WARNS=0; PASSES=0
for t in "${TESTS[@]}"; do
  IFS='|' read -r point path label <<<"$t"
  log="$LOGDIR/$(echo "$path" | sed 's/.*:://').log"
  printf '▶ %s %s …\n' "$point" "$label"
  start=$SECONDS
  cargo test -p sundayrec --lib "$path" -- --ignored --exact --nocapture >"$log" 2>&1
  rc=$?
  dur=$((SECONDS - start))

  verdict=Pass
  if [ $rc -ne 0 ]; then
    verdict=Fail
  elif ! grep -q 'test result: ok. 1 passed' "$log"; then
    verdict=Fail   # 0 tester kjørt (feil navn) skal aldri se grønt ut
  elif grep -q '^SKIP:' "$log"; then
    verdict=Warn
  fi

  # Måletallet testen selv skriver ut.
  measure="-"
  case "$path" in
    *the_virtual_driver*|*the_90_minute*)
      measure="$(awk '
        /picture - sound = / {
          s=$0; sub(/.*picture - sound = /,"",s); split(s,a," ms"); off=a[1]+0
          r=$0; sub(/.*right answer /,"",r); split(r,b," ms"); exp_=b[1]+0
          d=off-exp_; if (d<0) d=-d; if (d>mx) mx=d; n++
          o=off; if (o<0) o=-o; if (o>mo) mo=o
        }
        /avg [0-9.]+ fps/ { s=$0; sub(/.*avg /,"",s); split(s,f," "); fps=f[1] }
        END { if (n) printf "%d klapp, størst avvik fra riktig svar %.1f ms, størst bilde-lyd %.1f ms, %s fps", n, mx, mo, fps }
      ' "$log")"
      [ -z "$measure" ] && measure="-" ;;
    *video_under_cpu_load*)
      measure="$(grep -m1 'v4 under load' "$log" | sed -E 's/.*camera offered ([0-9]+), dropped ([0-9]+);.*delivered ([0-9.]+) fps.*slowest camera push (.*)$/tilbudt \1, droppet \2, \3 fps, tregeste push \4/')"
      [ -z "$measure" ] && measure="-" ;;
  esac
  [ "$verdict" = Fail ] && measure="$measure | se $log"
  [ "$verdict" = Warn ] && measure="hoppet over (SKIP)"

  case $verdict in Pass) PASSES=$((PASSES+1));; Warn) WARNS=$((WARNS+1));; Fail) FAILS=$((FAILS+1));; esac
  ROWS+=("$(printf '%-8s %-5s %-7s %-34s %s' "$point" "$verdict" "$(fmt_dur $dur)" "$label" "$measure")")
done

{
  echo "SundayRec rig:mac — maskinbevist, IKKE menneske-sjekket"
  echo "Dato:     $(date '+%Y-%m-%d %H:%M:%S')"
  echo "Commit:   $COMMIT$DIRTY"
  echo "Maskin:   $MODEL ($CHIP)"
  echo "macOS:    $MACOS"
  echo "Modus:    $MODE"
  echo
  printf '%-8s %-5s %-7s %-34s %s\n' Punkt Utfall Tid Test Måletall
  for r in "${ROWS[@]}"; do echo "$r"; done
  echo
  echo "Sum: $PASSES Pass, $WARNS Warn, $FAILS Fail"
  echo "Pass betyr at maskinen kjørte testen grønt. Det krysser IKKE av noe i docs/RIG-DAY.md;"
  echo "et kryss betyr at et menneske har sjekket på ekte maskinvare."
  echo
  echo "Trenger eieren i egen terminal (mikrofon, kamera, nettverk eller ekstern fil; ikke kjørt her):"
  echo "  (a/d) mikrofon, 60 s:           cargo test -p sundayrec --lib test_recording::tests::native_bench_60s_machine_proof -- --ignored --exact --nocapture"
  echo "  (a) mikrofon, soak:             cargo test -p sundayrec --lib soak::tests::soak_native_device -- --ignored --exact --nocapture   (SUNDAYREC_SOAK_ITERATIONS / SUNDAYREC_SOAK_SECS styrer lengden)"
  echo "  (mikrofon) rate-bytte i strøm:  cargo test -p sundayrec --lib recorder::native_capture::stream::tests::rate_change_under_a_running_stream_is_a_stream_fault -- --ignored --exact --nocapture"
  echo "  (v) alle kameraer:              cargo test -p sundayrec --lib camera::avfoundation::tests::captures_from_every_camera_on_this_mac -- --ignored --exact --nocapture"
  echo "  (v) kamera trekkes ut:          cargo test -p sundayrec --lib camera::reconnect::tests::real_camera_comes_back_on_the_same_id -- --ignored --exact --nocapture"
  echo "  (v) forhåndsvisning 4/s:        cargo test -p sundayrec --lib camera::preview::tests::real_camera_preview_holds_four_a_second_through_a_recording -- --ignored --exact --nocapture"
  echo "  (v) kamera drept midt i opptak: cargo test -p sundayrec --lib recorder::longrun::tests::native_video::real_camera_killed_mid_recording_is_recovered_to_an_mp4 -- --ignored --exact --nocapture"
  echo "  (ekstern fil) editor-benker:    SUNDAYREC_BENCH_FILE=<lang opptak> cargo test -p sundayrec --lib editor::tests::ffmpeg_smoke:: -- --ignored --nocapture   (+ SUNDAYREC_B6_BENCH_SOURCE for b6_bench_*)"
  echo "  (nettverk) telemetri mot wrangler dev: SUNDAYREC_TELEMETRY_URL=... SUNDAYREC_TELEMETRY_KEY=... cargo test -p sundayrec --lib telemetry::http_sender::live_endpoint -- --ignored --nocapture"
  echo "Detaljlogger: $LOGDIR/"
} | tee "$OUT"

echo
echo "Skrevet til $OUT"
[ "$FAILS" -eq 0 ]
