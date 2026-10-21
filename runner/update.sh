#!/usr/bin/env sh
# Update the per-user karma-team CLI from a self-hosted deployment.
#
# This script intentionally has no authentication step. It is safe to run
# through `curl .../api/cli/update.sh | sh` for an already configured runner:
# the downloaded executable is staged, checked, and atomically moved into the
# user's installation directory. Existing config, credentials, and runner
# registrations are left untouched.
set -eu

path_owner_uid() {
  stat -c '%u' "$1" 2>/dev/null || stat -f '%u' "$1" 2>/dev/null
}

passwd_home_for_uid() {
  user_uid=$1
  if command -v getent >/dev/null 2>&1; then
    getent passwd "$user_uid" 2>/dev/null | awk -F: 'NR == 1 { print $6; exit }'
  elif [ -r /etc/passwd ]; then
    awk -F: -v user_uid="$user_uid" '$3 == user_uid { print $6; exit }' /etc/passwd
  fi
}

runtime_base_is_usable() {
  runtime_base=$1
  runtime_uid=$2
  [ -d "$runtime_base" ] &&
    [ "$(path_owner_uid "$runtime_base" || true)" = "$runtime_uid" ] &&
    [ -r "$runtime_base" ] && [ -w "$runtime_base" ] && [ -x "$runtime_base" ]
}

# Do not let sudo/su make the updater act on another user's HOME or runtime
# directory. The same rule is used by install.sh and uninstall.sh.
normalize_user_environment() {
  effective_uid=$(id -u)
  case "$effective_uid" in
    '' | *[!0-9]*)
      echo "Could not determine the effective user ID." >&2
      exit 1
      ;;
  esac

  home_owner=''
  if [ -n "${HOME:-}" ] && [ -d "$HOME" ]; then
    home_owner=$(path_owner_uid "$HOME" || true)
  fi

  if [ "$effective_uid" = 0 ]; then
    elevated_from=${SUDO_USER:-${DOAS_USER:-}}
    if { [ -n "$elevated_from" ] && [ "$elevated_from" != root ]; } ||
      { [ -n "$home_owner" ] && [ "$home_owner" != 0 ]; }; then
      echo "Refusing to update a per-user karma-team CLI as root for another user." >&2
      echo "Open that user's login shell first (for example: sudo -iu ${elevated_from:-USER}), then run the updater without sudo." >&2
      exit 1
    fi
  elif [ -z "${HOME:-}" ] || [ "${HOME#/}" = "$HOME" ] || [ "$home_owner" != "$effective_uid" ]; then
    resolved_home=$(passwd_home_for_uid "$effective_uid" || true)
    if [ -z "$resolved_home" ] || [ "${resolved_home#/}" = "$resolved_home" ] ||
      [ ! -d "$resolved_home" ] || [ "$(path_owner_uid "$resolved_home" || true)" != "$effective_uid" ]; then
      echo "HOME does not belong to effective uid $effective_uid, and its account home could not be resolved safely." >&2
      echo "Start a login shell for the target user (for example: sudo -iu USER)." >&2
      exit 1
    fi
    printf 'Warning: replacing inherited HOME=%s with %s for uid %s.\n' "${HOME:-<unset>}" "$resolved_home" "$effective_uid" >&2
    HOME=$resolved_home
    export HOME
  fi

  case "$(uname -s 2>/dev/null || printf unknown)" in
    Linux)
      inherited_runtime=${XDG_RUNTIME_DIR:-}
      if [ -n "$inherited_runtime" ] && ! runtime_base_is_usable "$inherited_runtime" "$effective_uid"; then
        printf 'Warning: ignoring XDG_RUNTIME_DIR=%s because it is not owned and usable by uid %s.\n' "$inherited_runtime" "$effective_uid" >&2
        unset XDG_RUNTIME_DIR
        case "${DBUS_SESSION_BUS_ADDRESS:-}" in
          *"$inherited_runtime"*) unset DBUS_SESSION_BUS_ADDRESS ;;
        esac
      fi
      if [ -z "${XDG_RUNTIME_DIR:-}" ]; then
        standard_runtime="/run/user/$effective_uid"
        if runtime_base_is_usable "$standard_runtime" "$effective_uid"; then
          XDG_RUNTIME_DIR=$standard_runtime
          export XDG_RUNTIME_DIR
        fi
      fi
      ;;
  esac
}

usage() {
  cat <<'EOF'
Usage: update.sh [--check] [--no-restart] [--force] [--help]

  --check       download and compare the published binary without changing files
  --no-restart  update the binary but leave an existing daemon running
  --force       replace the binary even when its bytes are unchanged
  --help        show this help

The updater acts only on the current user's installation. Run it as the user
that owns the runner, without sudo. It preserves config, credentials, and
runner registrations. An active service is restarted after a successful
update only when all its runners report idle and --no-restart is not supplied.
Busy or unknown daemon state defers the restart; run karma-team restart later.
EOF
}

# Parse the structured status rather than matching arbitrary text in runner
# names or task output. Neither parser is required for the binary update;
# without one, leave the daemon running and explain how to apply the update.
daemon_restart_state() {
  if ! status_json=$("$KARMA_TEAM_BIN" status --json 2>/dev/null); then
    echo unknown
    return
  fi
  if command -v python3 >/dev/null 2>&1; then
    printf '%s' "$status_json" | python3 -c '
import json
import sys

try:
    status = json.load(sys.stdin)
    service = status.get("service")
    if service in ("inactive", "failed", "not installed", "stopped"):
        result = "inactive"
    elif service != "active":
        result = "unknown"
    else:
        runners = status["daemon"]["runners"]
        if not isinstance(runners, list):
            result = "unknown"
        elif all(
            isinstance(runner, dict)
            and runner.get("status") == "idle"
            and "current_run" in runner
            and runner["current_run"] is None
            and runner.get("approvals_pending") == 0
            for runner in runners
        ):
            result = "idle"
        else:
            result = "busy"
except (ValueError, KeyError, TypeError, AttributeError):
    result = "unknown"
print(result)
' 2>/dev/null || echo unknown
  elif command -v jq >/dev/null 2>&1; then
    printf '%s' "$status_json" | jq -r '
      if .service == "inactive" or .service == "failed" or
         .service == "not installed" or .service == "stopped" then "inactive"
      elif .service != "active" or (.daemon.runners | type) != "array" then "unknown"
      elif all(.daemon.runners[];
        type == "object" and .status == "idle" and has("current_run") and
        .current_run == null and .approvals_pending == 0
      ) then "idle"
      else "busy" end
    ' 2>/dev/null || echo unknown
  else
    echo unknown
  fi
}

CHECK_ONLY=0
NO_RESTART=0
FORCE=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --check) CHECK_ONLY=1 ;;
    --no-restart) NO_RESTART=1 ;;
    --force) FORCE=1 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

normalize_user_environment

INSTALL_DIR=${KARMA_TEAM_INSTALL_DIR:-"$HOME/.local/bin"}
case "$INSTALL_DIR" in
  /*) ;;
  *) INSTALL_DIR="$(pwd)/$INSTALL_DIR" ;;
esac
case "$INSTALL_DIR" in
  *:* | *'
'*)
    echo "KARMA_TEAM_INSTALL_DIR must not contain a colon or newline." >&2
    exit 1
    ;;
esac
if [ -L "$INSTALL_DIR" ]; then
  echo "Refusing to update through a symlinked installation directory: $INSTALL_DIR" >&2
  exit 1
fi
if [ ! -d "$INSTALL_DIR" ] || [ ! -w "$INSTALL_DIR" ] || [ ! -x "$INSTALL_DIR" ]; then
  echo "Cannot write to installation directory: $INSTALL_DIR" >&2
  echo 'Run the updater as the user who owns the installation, or set KARMA_TEAM_INSTALL_DIR to a writable directory.' >&2
  exit 1
fi
INSTALL_DIR=$(CDPATH= cd -- "$INSTALL_DIR" && pwd)

KARMA_TEAM_BIN="$INSTALL_DIR/karma-team"
if [ ! -f "$KARMA_TEAM_BIN" ] || [ ! -x "$KARMA_TEAM_BIN" ]; then
  if [ -f "$INSTALL_DIR/karma-team.exe" ] && [ -x "$INSTALL_DIR/karma-team.exe" ]; then
    KARMA_TEAM_BIN="$INSTALL_DIR/karma-team.exe"
  else
    echo "No installed karma-team binary found in $INSTALL_DIR." >&2
    echo "Install it first with the deployment installer." >&2
    exit 1
  fi
fi

API_URL=${KARMA_TEAM_API_URL:-}
while [ "${API_URL%/}" != "$API_URL" ]; do API_URL=${API_URL%/}; done
BIN_URL=${KARMA_TEAM_BIN_URL:-}
if [ -z "$BIN_URL" ]; then
  if [ -z "$API_URL" ]; then
    echo "Set KARMA_TEAM_API_URL to the self-hosted karma-team service URL." >&2
    exit 1
  fi
  os=$(uname -s)
  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) echo "Unsupported architecture for karma-team: $(uname -m)" >&2; exit 1 ;;
  esac
  case "$os" in
    Linux)
      libc=gnu
      if ldd --version 2>&1 | grep -qi musl; then libc=musl; fi
      for loader in /lib/ld-musl-*.so.1; do
        if [ -f "$loader" ]; then libc=musl; break; fi
      done
      target="$arch-unknown-linux-$libc"
      ;;
    Darwin) target="$arch-apple-darwin" ;;
    MINGW* | MSYS* | CYGWIN*)
      [ "$arch" = x86_64 ] || { echo "The Windows karma-team binary requires x86_64." >&2; exit 1; }
      target=x86_64-pc-windows-msvc
      ;;
    *) echo "Unsupported operating system for karma-team: $os" >&2; exit 1 ;;
  esac
  BIN_URL="$API_URL/api/cli/bin/$target/"
fi
case "$BIN_URL" in
  http://* | https://*) ;;
  *) echo "KARMA_TEAM_BIN_URL must be an HTTP or HTTPS URL." >&2; exit 1 ;;
esac

DOWNLOAD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/karma-team-update.XXXXXX")
STAGED_BIN="$DOWNLOAD_DIR/karma-team"
staged_install=''
cleanup() {
  if [ -n "$staged_install" ]; then rm -f -- "$staged_install"; fi
  rm -rf -- "$DOWNLOAD_DIR"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

echo "==> Downloading karma-team from $BIN_URL..."
if command -v curl >/dev/null 2>&1; then
  curl --proto '=http,https' --proto-redir '=http,https' --fail --location --silent --show-error --output "$STAGED_BIN" "$BIN_URL"
elif command -v wget >/dev/null 2>&1; then
  wget -q -O "$STAGED_BIN" "$BIN_URL"
else
  echo "Updating a binary requires curl or wget." >&2
  exit 1
fi
if [ ! -s "$STAGED_BIN" ]; then
  echo "The binary download returned an empty file." >&2
  exit 1
fi
chmod 0755 "$STAGED_BIN"

current_version=$("$KARMA_TEAM_BIN" --version 2>/dev/null || true)
if ! new_version=$("$STAGED_BIN" --version 2>/dev/null); then
  echo "Downloaded file is not a valid karma-team executable." >&2
  exit 1
fi
case "$new_version" in
  karma-team\ *) ;;
  *) echo "Downloaded file is not a valid karma-team executable." >&2; exit 1 ;;
esac
if [ -z "$current_version" ]; then current_version="unknown"; fi
echo "Current version: $current_version"
echo "Published version: $new_version"

same_binary=0
if command -v cmp >/dev/null 2>&1 && cmp -s "$KARMA_TEAM_BIN" "$STAGED_BIN"; then
  same_binary=1
fi
if [ "$same_binary" -eq 1 ] && [ "$FORCE" -eq 0 ]; then
  echo "karma-team is already up to date."
  exit 0
fi
if [ "$CHECK_ONLY" -eq 1 ]; then
  if [ "$same_binary" -eq 1 ]; then
    echo "No update available."
  else
    echo "An update is available."
  fi
  exit 0
fi

restart_state=skipped
if [ "$NO_RESTART" -eq 0 ]; then
  restart_state=$(daemon_restart_state)
fi

staged_install="$INSTALL_DIR/.karma-team.update.XXXXXX"
staged_install=$(mktemp "$staged_install")
cp "$STAGED_BIN" "$staged_install"
chmod 0755 "$staged_install"
mv -f "$staged_install" "$KARMA_TEAM_BIN"
staged_install=''
echo "Updated $KARMA_TEAM_BIN to $new_version."

# Recheck immediately before restarting in case work arrived during the swap.
if [ "$restart_state" = idle ]; then
  restart_state=$(daemon_restart_state)
fi
case "$restart_state" in
  idle)
    echo "==> Restarting the idle karma-team daemon..."
    if ! "$KARMA_TEAM_BIN" restart; then
      echo "The binary was updated, but the daemon restart failed. Run karma-team restart when ready." >&2
      exit 1
    fi
    ;;
  inactive)
    echo "No active karma-team service was detected; the new binary will be used on the next start."
    ;;
  skipped) echo "Daemon restart skipped (--no-restart)." ;;
  *)
    echo "Daemon restart deferred: runners are busy or their idle state could not be confirmed."
    echo "Run karma-team restart when all tasks have finished to apply the new binary to the daemon."
    ;;
esac
