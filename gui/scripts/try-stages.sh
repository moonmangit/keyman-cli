#!/usr/bin/env bash
# Opens the app on a demo master file with stages, apart from your real data.
#
# The demo has its own home folder (master file, tied folders, app settings), so
# ~/.keyman.yml and your app settings are never read or written. Keyman Cloud is
# pointed at an address that does not answer, so nothing can reach your account.
#
#   gui/scripts/try-stages.sh            # build, set up the demo once, open the app
#   gui/scripts/try-stages.sh --reset    # start the demo again from scratch
#   gui/scripts/try-stages.sh --cli      # only set up, and print CLI commands to try
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=${TMPDIR:-/tmp}
demo=${KEYMAN_DEMO_DIR:-${tmp%/}/keyman-stages-demo}
demo=${demo%/}

# Homebrew's rustup is keg-only: add it when cargo is not on PATH.
if ! command -v cargo >/dev/null && [ -x /opt/homebrew/opt/rustup/bin/cargo ]; then
  export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
fi

reset=0 cli_only=0
for arg in "$@"; do
  case $arg in
    --reset) reset=1 ;;
    --cli) cli_only=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

echo "Building the CLI and the app…"
(cd "$root" && cargo build -q)
(cd "$root/gui" && cargo build -q)
keyman="$root/target/debug/keyman"

if [ $reset = 1 ] && [ -d "$demo" ]; then
  # Only ever the demo folder this script made.
  case $(basename "$demo") in
    keyman-stages-demo) rm -rf -- "$demo" ;;
    *) echo "refusing to delete '$demo' (not a keyman-stages-demo folder)" >&2; exit 1 ;;
  esac
fi

if [ ! -d "$demo" ]; then
  echo "Setting up the demo in $demo"
  mkdir -p "$demo/home" "$demo/prefs/dev.keyman.app" \
    "$demo/code/shop-api" "$demo/code/shop-api-staging" "$demo/code/shop-api-preprod" \
    "$demo/code/shop-api-new" "$demo/code/blog"
  cat > "$demo/home/.keyman.yml" <<'YAML'
# Keyman demo master file (made up values).

shop-api:
  # Shared by every stage.
  LOG_LEVEL: "info"
  DEBUG: "false"
  API_URL: "https://api.shop.example"
  production:
    DATABASE_URL: "postgres://prod.db.example/shop"
    STRIPE_KEY: "sk_live_demo"
  staging:
    DATABASE_URL: "postgres://staging.db.example/shop"
    DEBUG: "true"
    STRIPE_KEY: "sk_test_demo"
  pre-production:
    DATABASE_URL: "postgres://preprod.db.example/shop"

blog:
  SITE_URL: "https://blog.example"
  ANALYTICS_ID: "demo-123"
YAML
  chmod 600 "$demo/home/.keyman.yml"
  HOME="$demo/home" "$keyman" load "$demo/home/.keyman.yml" >/dev/null

  # production: in sync. staging: one value changed by hand, so it differs.
  # pre-production: no .env yet. shop-api-new: tied, but no stage picked yet.
  HOME="$demo/home" "$keyman" inject shop-api:production "$demo/code/shop-api" >/dev/null
  HOME="$demo/home" "$keyman" inject shop-api:staging "$demo/code/shop-api-staging" >/dev/null
  sed -i.bak 's/^LOG_LEVEL=.*/LOG_LEVEL="debug"/' "$demo/code/shop-api-staging/.env"
  rm -f "$demo/code/shop-api-staging/.env.bak"

  c=$demo/code
  cat > "$demo/prefs/dev.keyman.app/prefs.json" <<JSON
{
  "ties": {
    "shop-api": ["$c/shop-api", "$c/shop-api-staging", "$c/shop-api-preprod", "$c/shop-api-new"],
    "blog": ["$c/blog"]
  },
  "active": { "shop-api": "$c/shop-api-staging" },
  "tie_stages": {
    "shop-api\n$c/shop-api": "production",
    "shop-api\n$c/shop-api-staging": "staging",
    "shop-api\n$c/shop-api-preprod": "pre-production"
  },
  "cloud_prompt_done": true
}
JSON
fi

cat <<TEXT

Demo ready: $demo
  Master file: $demo/home/.keyman.yml
  Folders:     $demo/code/{shop-api,shop-api-staging,shop-api-preprod,shop-api-new,blog}

Try the CLI against the demo (paste into a shell):

  alias km='HOME="$demo/home" "$keyman"'
  km status
  km show shop-api
  km show shop-api:staging --reveal
  km inject shop-api "$demo/code/blog"                 # stops: names the stages
  (cd "$demo/code/shop-api" && km diff :production)  # ':stage' uses the folder's project
  km inject shop-api:pre-production "$demo/code/shop-api-preprod"
  km diff shop-api:staging "$demo/code/shop-api-staging" --reveal
  km save shop-api:qa "$demo/code/shop-api-staging" --new

TEXT

[ $cli_only = 1 ] && exit 0

echo "Opening the app on the demo. Close its window to finish."
cd "$root/gui"
HOME="$demo/home" \
RUNE_PREFS_DIR="$demo/prefs" \
KEYMAN_SUPABASE_URL="http://127.0.0.1:9" \
  exec ./target/debug/keyman-app
