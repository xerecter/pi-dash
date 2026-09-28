#!/usr/bin/env sh
# karma-team runner installer.
#
# Installs the local runner binary and then launches `karma-team auth login` so
# the user lands in the device-code flow on the same command. A checked-out
# source tree is enough to install the runner: an existing local binary is
# reused, and a release binary is built from `runner/Cargo.toml` when needed.
# Set KARMA_TEAM_BIN_URL to download a raw binary from a custom HTTP(S) URL,
# or KARMA_TEAM_LOCAL_BIN to install a specific local executable instead.
#
# Usage:
#   bash ./runner/install.sh
#   KARMA_TEAM_BIN_URL=https://example.com/karma-team bash ./runner/install.sh
#   KARMA_TEAM_LOCAL_BIN=/path/to/karma-team bash ./runner/install.sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
INSTALL_DIR=${KARMA_TEAM_INSTALL_DIR:-"$HOME/.local/bin"}
BIN_URL=${KARMA_TEAM_BIN_URL:-}
DOWNLOAD_DIR=""
STAGED_BIN=""

cleanup() {
  if [ -n "$STAGED_BIN" ]; then
    rm -f -- "$STAGED_BIN"
  fi
  if [ -n "$DOWNLOAD_DIR" ]; then
    rm -rf -- "$DOWNLOAD_DIR"
  fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

# A caller can point at a locally built binary explicitly. Relative paths are
# resolved from the repository root so `KARMA_TEAM_LOCAL_BIN=target/debug/karma-team`
# works regardless of the caller's current directory.
LOCAL_BIN=${KARMA_TEAM_LOCAL_BIN:-}
if [ -n "$LOCAL_BIN" ]; then
  case "$LOCAL_BIN" in
    /*) ;;
    *) LOCAL_BIN="$REPO_ROOT/$LOCAL_BIN" ;;
  esac
fi

TARGET_DIR=${CARGO_TARGET_DIR:-"$REPO_ROOT/target"}
case "$TARGET_DIR" in
  /*) ;;
  *) TARGET_DIR="$REPO_ROOT/$TARGET_DIR" ;;
esac

is_executable_file() {
  [ -f "$1" ] && [ -x "$1" ]
}

if [ -n "$LOCAL_BIN" ] && [ -n "$BIN_URL" ]; then
  echo "Set only one of KARMA_TEAM_LOCAL_BIN and KARMA_TEAM_BIN_URL." >&2
  exit 1
fi
if [ -n "$LOCAL_BIN" ] && ! is_executable_file "$LOCAL_BIN"; then
  echo "KARMA_TEAM_LOCAL_BIN must point to an existing executable: $LOCAL_BIN" >&2
  exit 1
fi
if [ -n "$BIN_URL" ]; then
  case "$BIN_URL" in
    http://* | https://*) ;;
    *)
      echo "KARMA_TEAM_BIN_URL must be an HTTP or HTTPS URL." >&2
      exit 1
      ;;
  esac
fi

download_binary() {
  DOWNLOAD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/karma-team-install.XXXXXX")
  SOURCE_BIN="$DOWNLOAD_DIR/karma-team"
  URL_PATH=${BIN_URL%%\?*}
  URL_PATH=${URL_PATH%%\#*}
  case "$URL_PATH" in
    *.exe) SOURCE_BIN="$SOURCE_BIN.exe" ;;
  esac

  echo "==> Downloading karma-team from KARMA_TEAM_BIN_URL..."
  if command -v curl >/dev/null 2>&1; then
    curl --fail --location --silent --show-error --output "$SOURCE_BIN" "$BIN_URL"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$SOURCE_BIN" "$BIN_URL"
  else
    echo "Downloading a binary requires curl or wget." >&2
    exit 1
  fi
  if [ ! -s "$SOURCE_BIN" ]; then
    echo "KARMA_TEAM_BIN_URL returned an empty file." >&2
    exit 1
  fi
  chmod 0755 "$SOURCE_BIN"
}

# Return the first usable local binary. Cargo's workspace target directory is
# normally at the repository root, but include runner/target as well for users
# who built the package from inside `runner/`.
find_local_binary() {
  for candidate in \
    "$TARGET_DIR/release/karma-team" \
    "$TARGET_DIR/release/karma-team.exe" \
    "$TARGET_DIR/debug/karma-team" \
    "$TARGET_DIR/debug/karma-team.exe" \
    "$SCRIPT_DIR/target/release/karma-team" \
    "$SCRIPT_DIR/target/release/karma-team.exe" \
    "$SCRIPT_DIR/target/debug/karma-team" \
    "$SCRIPT_DIR/target/debug/karma-team.exe" \
    "$INSTALL_DIR/karma-team" \
    "$INSTALL_DIR/karma-team.exe"; do
    if is_executable_file "$candidate"; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  return 1
}

# Running under WSL installs the *Linux* build, which is usually not what a
# Windows user wants: it cannot see the Windows filesystem the way they expect
# and cannot drive coding agents installed on the Windows side. The install
# still succeeds, so warn loudly rather than failing — a runner that genuinely
# lives inside the WSL distro is a legitimate setup.
if grep -qi microsoft /proc/version 2>/dev/null || [ -n "${WSL_DISTRO_NAME:-}" ]; then
  echo ""
  echo "Warning: this looks like WSL, so the Linux build of karma-team will be installed."
  echo "It will drive coding agents inside this WSL distro only — not ones installed"
  echo "on Windows. If your agents live on the Windows side, cancel and run this"
  echo "from PowerShell instead:"
  echo ""
  echo "  Run the Windows installer from a Windows checkout instead."
  echo ""

  # Actually stop and ask, rather than printing advice the user cannot act on:
  # install and `auth login` follow immediately, so a bare warning would need
  # to be read and Ctrl-C'd before the command continues. stdin may be a pipe,
  # so read from /dev/tty — probed for openability because
  # the device node exists even where it cannot be opened (Docker without -t,
  # cron, systemd, SSH without -t).
  if (: </dev/tty) 2>/dev/null; then
    printf "Continue installing the Linux build? [y/N] "
    read -r _wsl_reply </dev/tty || _wsl_reply=""
    case "$_wsl_reply" in
      [yY] | [yY][eE][sS]) ;;
      *)
        echo "Cancelled."
        exit 0
        ;;
    esac
    echo ""
  else
    # Headless: nobody to ask, and erroring out would break legitimate
    # automated installs into a WSL distro. Warn and continue.
    echo "No terminal detected — continuing with the Linux build."
    echo ""
  fi
fi

if [ -n "$BIN_URL" ]; then
  download_binary
elif [ -n "$LOCAL_BIN" ]; then
  SOURCE_BIN="$LOCAL_BIN"
  echo "Using KARMA_TEAM_LOCAL_BIN: $SOURCE_BIN"
else
  echo "==> Looking for a local karma-team binary..."
  SOURCE_BIN=$(find_local_binary || true)
fi

if [ -z "$SOURCE_BIN" ]; then
  if ! command -v cargo >/dev/null 2>&1; then
    echo ""
    echo "No local karma-team binary was found, and cargo is not installed."
    echo "Install Rust 1.93 or newer, then rerun: bash ./runner/install.sh"
    exit 1
  fi

  echo "==> Building karma-team from local source..."
  (
    cd "$REPO_ROOT"
    CARGO_TARGET_DIR="$TARGET_DIR" cargo build \
      --manifest-path "$SCRIPT_DIR/Cargo.toml" \
      --locked \
      --bin karma-team \
      --release
  )
  SOURCE_BIN="$TARGET_DIR/release/karma-team"
  if [ ! -x "$SOURCE_BIN" ] && [ -x "$TARGET_DIR/release/karma-team.exe" ]; then
    SOURCE_BIN="$TARGET_DIR/release/karma-team.exe"
  fi
  if [ ! -x "$SOURCE_BIN" ]; then
    echo ""
    echo "cargo completed, but the karma-team binary was not found at:"
    echo "  $SOURCE_BIN"
    exit 1
  fi
else
  echo "Installing binary: $SOURCE_BIN"
fi

mkdir -p "$INSTALL_DIR"
case "$SOURCE_BIN" in
  *.exe) KARMA_TEAM_BIN="$INSTALL_DIR/karma-team.exe" ;;
  *) KARMA_TEAM_BIN="$INSTALL_DIR/karma-team" ;;
esac

if [ "$SOURCE_BIN" != "$KARMA_TEAM_BIN" ]; then
  # Stage first and replace atomically. A running karma-team process keeps its
  # existing inode open, so upgrades also work while the daemon is active.
  STAGED_BIN="$KARMA_TEAM_BIN.tmp.$$"
  cp "$SOURCE_BIN" "$STAGED_BIN"
  chmod 0755 "$STAGED_BIN"
  mv -f "$STAGED_BIN" "$KARMA_TEAM_BIN"
  STAGED_BIN=""
fi
chmod 0755 "$KARMA_TEAM_BIN"
cleanup
trap - EXIT HUP INT TERM

echo "Installed karma-team at $KARMA_TEAM_BIN"

echo ""
echo "==> Starting authentication..."
echo ""

# When this script is invoked through a pipe, sh's stdin is the pipe and an
# interactive prompt would see EOF. Reattach /dev/tty so the device-code flow
# can read keystrokes for the workspace and runner-add prompts.
#
# `[ -e /dev/tty ]` is not enough: the device node exists on disk even
# in headless contexts (Docker without -t, cron, systemd, SSH without
# -t), so the check would pass and then `< /dev/tty` would crash with
# "No such device or address". Probe openability by actually trying to
# read from /dev/tty in a subshell.
if (: </dev/tty) 2>/dev/null; then
  exec "$KARMA_TEAM_BIN" auth login < /dev/tty
fi

# No TTY (CI, Dockerfile, piped without a terminal): don't try to
# auth — there's no one to approve the device code. Point at the
# headless path instead.
echo "No terminal detected — skipping auto-auth."
echo "Run \`karma-team auth login --no-browser --url <URL>\`, approve the printed"
echo "URL from another browser, then run \`karma-team runner add --project <PROJECT>\`."
