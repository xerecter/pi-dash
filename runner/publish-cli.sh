#!/usr/bin/env sh
# Build and atomically publish a CLI binary for the self-hosted download API.
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
ARTIFACT_DIR=${KARMA_TEAM_CLI_ARTIFACTS_HOST_DIR:-"$REPO_ROOT/deployments/cli/artifacts"}
CLI_TARGET=${KARMA_TEAM_CLI_TARGET:-}
CLI_BINARY=${KARMA_TEAM_CLI_LOCAL_BINARY:-}
STAGED_BIN=""

cleanup() {
  if [ -n "$STAGED_BIN" ]; then
    rm -f -- "$STAGED_BIN"
  fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

if [ -z "$CLI_TARGET" ]; then
  CLI_TARGET=$(rustc -vV | sed -n 's/^host: //p')
fi
case "$CLI_TARGET" in
  x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu | \
  x86_64-unknown-linux-musl | aarch64-unknown-linux-musl | \
  x86_64-apple-darwin | aarch64-apple-darwin)
    BIN_NAME=karma-team ;;
  x86_64-pc-windows-msvc)
    BIN_NAME=karma-team.exe ;;
  *)
    echo "Unsupported CLI target: $CLI_TARGET" >&2
    exit 1 ;;
esac

if [ -z "$CLI_BINARY" ]; then
  CLI_HOST=$(rustc -vV | sed -n 's/^host: //p')
  TARGET_DIR=${CARGO_TARGET_DIR:-"$REPO_ROOT/target"}
  case "$TARGET_DIR" in
    /*) ;;
    *) TARGET_DIR="$REPO_ROOT/$TARGET_DIR" ;;
  esac
  if [ "$CLI_TARGET" = "$CLI_HOST" ]; then
    (cd "$REPO_ROOT" && CARGO_TARGET_DIR="$TARGET_DIR" cargo build -p karma-team --release --locked)
    CLI_BINARY="$TARGET_DIR/release/$BIN_NAME"
  else
    (cd "$REPO_ROOT" && CARGO_TARGET_DIR="$TARGET_DIR" cargo build -p karma-team --release --locked --target "$CLI_TARGET")
    CLI_BINARY="$TARGET_DIR/$CLI_TARGET/release/$BIN_NAME"
  fi
fi

if [ ! -s "$CLI_BINARY" ]; then
  echo "CLI binary is missing or empty: $CLI_BINARY" >&2
  exit 1
fi

mkdir -p "$ARTIFACT_DIR/$CLI_TARGET"
STAGED_BIN=$(mktemp "$ARTIFACT_DIR/$CLI_TARGET/.karma-team.XXXXXX")
cp "$CLI_BINARY" "$STAGED_BIN"
chmod 0755 "$STAGED_BIN"
mv -f "$STAGED_BIN" "$ARTIFACT_DIR/$CLI_TARGET/$BIN_NAME"
STAGED_BIN=""
echo "Published $ARTIFACT_DIR/$CLI_TARGET/$BIN_NAME"
echo "Download path: /api/cli/bin/$CLI_TARGET/"
