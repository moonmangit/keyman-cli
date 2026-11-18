#!/usr/bin/env bash
# Checks the bash completion script against a demo master file with stages.
#   scripts/check-completions.sh [path/to/keyman]   (default: target/debug/keyman)
# Runs in plain bash (no bash-completion package, no extglob), like a fresh shell.
set -euo pipefail

keyman=$(cd "$(dirname "${1:-target/debug/keyman}")" && pwd)/$(basename "${1:-target/debug/keyman}")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/home" "$work/bin" "$work/dir/sub"
ln -s "$keyman" "$work/bin/keyman"
export HOME="$work/home" PATH="$work/bin:$PATH"
printf 'shop-api:\n  LOG: "info"\n  production:\n    DB: "p"\n  staging:\n    DB: "s"\nblog:\n  URL: "b"\n' > "$work/master.yml"
keyman load "$work/master.yml" >/dev/null

eval "$(keyman completions bash)"

failures=0
# Completes `line` the way bash does (words split at spaces and ':'), and compares.
check() {
  local line=$1 expected=$2
  COMP_LINE=$line
  COMP_POINT=${#line}
  local split=${line//:/ : }
  read -ra COMP_WORDS <<< "$split"
  if [[ $line == *" " || $line == *: ]]; then COMP_WORDS+=(""); fi
  COMP_CWORD=$(( ${#COMP_WORDS[@]} - 1 ))
  COMPREPLY=()
  _keyman || true
  local got="${COMPREPLY[*]}"
  if [[ $got == "$expected" ]]; then
    echo "ok    [$line] -> $got"
  else
    echo "FAIL  [$line] -> '$got', expected '$expected'"
    failures=$((failures + 1))
  fi
}

check "keyman inj" "inject"
check "keyman inject sh" "shop-api shop-api:production shop-api:staging"
check "keyman inject shop-api:" "production staging"
check "keyman inject shop-api:st" "staging"
check "keyman show bl" "blog"
check "keyman completions z" "zsh"
cd "$work/dir"
check "keyman inject shop-api:staging su" "sub"

if (( failures > 0 )); then
  echo "$failures completion check(s) failed"
  exit 1
fi
echo "All completion checks passed"
