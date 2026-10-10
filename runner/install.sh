#!/usr/bin/env sh
# karma-team runner installer.
#
# Installs the local runner binary and then launches `karma-team auth login` so
# the user lands in the device-code flow on the same command. A checked-out
# source tree is enough to install the runner: an existing local binary is
# reused, and a release binary is built from `runner/Cargo.toml` when needed.
# Set KARMA_TEAM_API_URL to install from your karma-team deployment,
# KARMA_TEAM_BIN_URL for a custom raw binary, or KARMA_TEAM_LOCAL_BIN for
# a specific local executable.
#
# Usage:
#   bash ./runner/install.sh
#   curl -fsSL https://example.com/api/cli/install.sh | sh
#   KARMA_TEAM_BIN_URL=https://example.com/karma-team bash ./runner/install.sh
#   KARMA_TEAM_LOCAL_BIN=/path/to/karma-team bash ./runner/install.sh
set -eu

if [ -f "$0" ]; then
  SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
  REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
else
  # A script piped to sh has no source-tree location.
  SCRIPT_DIR=$(pwd)
  REPO_ROOT="$SCRIPT_DIR"
fi
INSTALL_DIR=${KARMA_TEAM_INSTALL_DIR:-"$HOME/.local/bin"}
BIN_URL=${KARMA_TEAM_BIN_URL:-}
API_URL=${KARMA_TEAM_API_URL:-}
while [ "${API_URL%/}" != "$API_URL" ]; do API_URL=${API_URL%/}; done
BIN_SUFFIX=""
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

prepare_install_dir() {
  case "$INSTALL_DIR" in
    /*) ;;
    *) INSTALL_DIR="$(pwd)/$INSTALL_DIR" ;;
  esac
  case "$INSTALL_DIR" in
    *:* | *'
'*)
      echo "KARMA_TEAM_INSTALL_DIR must not contain a colon or newline (not usable in PATH)." >&2
      exit 1
      ;;
  esac
  if ! mkdir -p -- "$INSTALL_DIR" || [ ! -w "$INSTALL_DIR" ] || [ ! -x "$INSTALL_DIR" ]; then
    printf 'Cannot write to installation directory: %s\n' "$INSTALL_DIR" >&2
    echo 'Set KARMA_TEAM_INSTALL_DIR to a user-owned directory, such as "$HOME/.local/bin"; sudo is not required.' >&2
    exit 1
  fi
  INSTALL_DIR=$(CDPATH= cd -- "$INSTALL_DIR" && pwd)
}

shell_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

update_profile() {
  if { [ -e "$1" ] || [ -L "$1" ]; } && [ ! -f "$1" ]; then
    printf 'Warning: Could not update %s (not a regular file). Configure PATH manually.\n' "$1" >&2
    return
  fi
  if grep -Fqx -- "$PATH_SNIPPET" "$1" 2>/dev/null; then
    return
  fi
  if ! (mkdir -p -- "$(dirname -- "$1")" && printf '\n# karma-team PATH\n%s\n' "$PATH_SNIPPET" >> "$1"); then
    printf 'Warning: Could not update %s. Configure PATH manually with the command below.\n' "$1" >&2
  fi
}

configure_path() {
  QUOTED_INSTALL_DIR=$(shell_quote "$INSTALL_DIR")
  PATH_COMMAND='export PATH='"$QUOTED_INSTALL_DIR"'${PATH:+:"$PATH"}'
  PATH_SNIPPET='case ":${PATH:-}:" in *:'"$QUOTED_INSTALL_DIR"':*) ;; *) '"$PATH_COMMAND"' ;; esac'
  case ":${PATH:-}:" in
    *:"$INSTALL_DIR":*) ;;
    *) export PATH="$INSTALL_DIR${PATH:+:$PATH}" ;;
  esac
  if [ -n "${HOME:-}" ]; then
    USER_SHELL=${SHELL:-sh}
    update_profile "$HOME/.profile"
    if [ "${USER_SHELL##*/}" = bash ] || [ -f "$HOME/.bashrc" ]; then
      update_profile "$HOME/.bashrc"
    fi
    # Do not create login files that would hide the user's existing .profile.
    for profile in "$HOME/.bash_profile" "$HOME/.bash_login"; do
      if [ -f "$profile" ]; then update_profile "$profile"; fi
    done
    ZSH_CONFIG_DIR=${ZDOTDIR:-"$HOME"}
    if [ "${USER_SHELL##*/}" = zsh ] || [ -f "$ZSH_CONFIG_DIR/.zshrc" ] || [ -f "$ZSH_CONFIG_DIR/.zprofile" ]; then
      update_profile "$ZSH_CONFIG_DIR/.zprofile"
      update_profile "$ZSH_CONFIG_DIR/.zshrc"
    fi
  else
    echo "Warning: HOME is not set; configure PATH manually." >&2
  fi
  echo ""
  echo "To use karma-team in the current shell (sh/bash/zsh), run:"
  printf '  %s\n' "$PATH_COMMAND"
}

if [ -n "$LOCAL_BIN" ] && [ -n "$BIN_URL" ]; then
  echo "Set only one of KARMA_TEAM_LOCAL_BIN and KARMA_TEAM_BIN_URL." >&2
  exit 1
fi
if [ -n "$LOCAL_BIN" ] && ! is_executable_file "$LOCAL_BIN"; then
  echo "KARMA_TEAM_LOCAL_BIN must point to an existing executable: $LOCAL_BIN" >&2
  exit 1
fi
if [ -n "$API_URL" ]; then
  case "$API_URL" in
    http://?* | https://?*) ;;
    *)
      echo "KARMA_TEAM_API_URL must be an HTTP or HTTPS base URL." >&2
      exit 1
      ;;
  esac
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

detect_target() {
  os=$(uname -s)
  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) echo "Unsupported architecture for karma-team: $(uname -m)" >&2; return 1 ;;
  esac
  case "$os" in
    Linux)
      libc=gnu
      if ldd --version 2>&1 | grep -qi musl; then
        libc=musl
      else
        for loader in /lib/ld-musl-*.so.1; do
          if [ -f "$loader" ]; then libc=musl; break; fi
        done
      fi
      printf '%s-unknown-linux-%s\n' "$arch" "$libc"
      ;;
    Darwin) printf '%s-apple-darwin\n' "$arch" ;;
    MINGW* | MSYS* | CYGWIN*)
      if [ "$arch" != x86_64 ]; then
        echo "The Windows karma-team binary requires x86_64." >&2
        return 1
      fi
      printf '%s\n' x86_64-pc-windows-msvc
      ;;
    *) echo "Unsupported operating system for karma-team: $os" >&2; return 1 ;;
  esac
}

download_binary() {
  DOWNLOAD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/karma-team-install.XXXXXX")
  SOURCE_BIN="$DOWNLOAD_DIR/karma-team$BIN_SUFFIX"
  URL_PATH=${BIN_URL%%\?*}
  URL_PATH=${URL_PATH%%\#*}
  case "$URL_PATH" in
    *.exe) SOURCE_BIN="$DOWNLOAD_DIR/karma-team.exe" ;;
  esac

  echo "==> Downloading karma-team from $BIN_URL..."
  if command -v curl >/dev/null 2>&1; then
    curl --fail --location --silent --show-error --output "$SOURCE_BIN" "$BIN_URL"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$SOURCE_BIN" "$BIN_URL"
  else
    echo "Downloading a binary requires curl or wget." >&2
    exit 1
  fi
  if [ ! -s "$SOURCE_BIN" ]; then
    echo "The binary download returned an empty file." >&2
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

if [ -z "$BIN_URL" ] && [ -z "$LOCAL_BIN" ] && [ -n "$API_URL" ]; then
  TARGET=$(detect_target)
  case "$TARGET" in *-windows-*) BIN_SUFFIX=.exe ;; esac
  BIN_URL="$API_URL/api/cli/bin/$TARGET/"
fi

prepare_install_dir

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
  if [ ! -f "$SCRIPT_DIR/Cargo.toml" ]; then
    echo "No local source tree found. Set KARMA_TEAM_API_URL to your deployment URL." >&2
    exit 1
  fi
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

case "$SOURCE_BIN" in
  *.exe) KARMA_TEAM_BIN="$INSTALL_DIR/karma-team.exe" ;;
  *) KARMA_TEAM_BIN="$INSTALL_DIR/karma-team" ;;
esac

if [ "$SOURCE_BIN" != "$KARMA_TEAM_BIN" ]; then
  # Stage first and replace atomically. A running karma-team process keeps its
  # existing inode open, so upgrades also work while the daemon is active.
  STAGED_BIN=$(mktemp "$KARMA_TEAM_BIN.tmp.XXXXXX")
  cp "$SOURCE_BIN" "$STAGED_BIN"
  chmod 0755 "$STAGED_BIN"
  mv -f "$STAGED_BIN" "$KARMA_TEAM_BIN"
  STAGED_BIN=""
fi
chmod 0755 "$KARMA_TEAM_BIN"
cleanup
trap - EXIT HUP INT TERM

echo "Installed karma-team at $KARMA_TEAM_BIN"
configure_path

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
  if [ -n "$API_URL" ]; then
    exec "$KARMA_TEAM_BIN" auth login --url "$API_URL" < /dev/tty
  fi
  exec "$KARMA_TEAM_BIN" auth login < /dev/tty
fi

# No TTY (CI, Dockerfile, piped without a terminal): don't try to
# auth — there's no one to approve the device code. Point at the
# headless path instead.
echo "No terminal detected — skipping auto-auth."
if [ -n "$API_URL" ]; then
  printf "Run: '%s' auth login --no-browser --url '%s'\n" \
    "$(printf '%s' "$KARMA_TEAM_BIN" | sed "s/'/'\\\\''/g")" \
    "$(printf '%s' "$API_URL" | sed "s/'/'\\\\''/g")"
else
  echo "Run \`karma-team auth login --no-browser --url <URL>\`."
fi
echo "Approve the printed URL in a browser, then run \`karma-team runner add --project <PROJECT>\`."
