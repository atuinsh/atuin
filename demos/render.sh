#!/usr/bin/env bash
# Renders the README demos (demos/*.tape) with betamax into demos/out/, once
# per theme in themes/, as out/<demo>-<theme>.webp.
#
# Each render runs in a throwaway HOME seeded from fixtures/, under an empty
# environment. Betamax hands its environment to the tape's shell, so an
# inherited HISTFILE would import and show the real history of whoever ran
# this. The HOME also carries the themes and fonts/, since betamax only looks
# for fonts in system dirs and that HOME.
#
# A demo with a rigs/<demo>.sh gets the rig's rig_up before its render and
# rig_down after (see rigs/threads.sh).
#
# Usage: demos/render.sh [tape...]
#   ATUIN_BIN         the atuin to record (default: target/debug/atuin)
#   ATUIN_SERVER_BIN  the sync server rigs run (default: target/debug/atuin-server)
#   BETAMAX           the betamax to run (default: betamax on PATH)
#   ANTHROPIC_API_KEY, OPENAI_API_KEY  for the agents the threads demo runs
set -euo pipefail

demos=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
absolute() { echo "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; }

atuin_bin=${ATUIN_BIN:-$demos/../target/debug/atuin}
if [ ! -x "$atuin_bin" ]; then
  echo "No atuin at $atuin_bin. Run cargo build -p atuin, or set ATUIN_BIN."
  exit 1
fi
atuin_bin=$(absolute "$atuin_bin")
betamax=$(command -v "${BETAMAX:-betamax}")

# Writes fixtures/history.tsv (`<exit><TAB><duration s><TAB><command>`,
# oldest first) as zsh extended history that ends a few minutes ago, so ages
# read naturally.
seed_history() {
  local spacing_s=3187 now count i=0 exit duration command
  now=$(date +%s)
  count=$(wc -l <"$1")
  while IFS=$'\t' read -r exit duration command; do
    printf ': %d:%d;%s\n' $((now - (count - i) * spacing_s)) "$duration" "$command"
    i=$((i + 1))
  done <"$1"
}

# SQL giving each command of fixtures/history.tsv its exit code, which zsh's
# history doesn't keep.
history_exits() {
  local exit duration command
  while IFS=$'\t' read -r exit duration command; do
    printf "UPDATE history SET exit = %d WHERE command = '%s';\n" \
      "$exit" "${command//\'/\'\'}"
  done <"$1"
}

# A HOME seeded from fixtures/home, plus fixtures/<overlay> when given, with
# atuin on its PATH, the fonts betamax needs, and the fixture history
# imported.
make_home() {
  local home=$1 overlay=${2:-}
  mkdir -p "$home"
  cp -R fixtures/home/. "$home/"
  [ -z "$overlay" ] || cp -R "fixtures/$overlay/." "$home/"
  mkdir -p "$home/bin" "$home/.local/share/fonts" "$home/Library"
  ln -s "$atuin_bin" "$home/bin/atuin"
  cp fonts/*.ttf "$home/.local/share/fonts/"
  ln -s ../.local/share/fonts "$home/Library/Fonts"
  seed_history fixtures/history.tsv >"$home/.zsh_history"
  env -i HOME="$home" PATH="$home/bin:/usr/bin:/bin" ATUIN_HOST_NAME="${overlay:-laptop}" \
    HISTFILE="$home/.zsh_history" \
    atuin import zsh >/dev/null 2>&1
  history_exits fixtures/history.tsv | sqlite3 "$(env -i HOME="$home" \
    PATH="$home/bin:/usr/bin:/bin" atuin config get --resolved db_path)"
}

# Whatever render is underway when this exits, interrupted or failed.
work='' rig=''
cleanup() {
  [ -n "$work" ] || return 0
  ! declare -F rig_down >/dev/null || rig_down "$work"
  # Twice: an agent betamax just killed can write its config after the first.
  rm -rf "$work" 2>/dev/null || true
  sleep 1
  rm -rf "$work"
  work=
}
trap cleanup EXIT

cd "$demos"
mkdir -p out
if [ $# -eq 0 ]; then
  set -- *.tape
fi

for tape in "$@"; do
  demo=$(basename "$tape" .tape)

  rig=
  if [ -f "rigs/$demo.sh" ]; then
    rig=rigs/$demo.sh
  fi

  for theme in themes/*.tape; do
    theme_name=$(basename "$theme" .tape)
    # Short, since rigs put the daemons' sockets in it and those cap a path
    # at around 100 bytes.
    # Resolved (/tmp is /private/tmp on macOS), so agents that shorten paths
    # under HOME to ~ see the same path as HOME.
    work=$(cd "$(mktemp -d /tmp/demo.XXXXXX)" && pwd -P)
    home=$work/laptop
    make_home "$home"
    printf 'Output out/%s-%s.webp\nSource %s\nSource %s\n' \
      "$demo" "$theme_name" "$demos/$theme" "$demos/$tape" >"$home/demo.tape"

    shell_env=(HOME="$home" PATH="$home/bin:/usr/bin:/bin" TERM=xterm-256color
      LANG=C.UTF-8 TZ=UTC HISTFILE="$home/.zsh_history")
    if [ -n "$rig" ]; then
      # shellcheck source=/dev/null # one of rigs/
      source "$rig"
      rig_up "$work" "$theme_name"
    fi

    echo "Rendering $tape with $theme"
    status=0
    env -i "${shell_env[@]}" \
      "$betamax" run --quiet --failure-dir out/failures "$home/demo.tape" ||
      status=$?
    cleanup
    [ "$status" -eq 0 ] || exit "$status"
  done
done
