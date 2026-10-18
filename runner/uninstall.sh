#!/usr/bin/env sh
# Remove the per-user karma-team CLI, service, state, and PATH additions.
# This script deliberately never uses sudo and never removes an installation
# directory (or a user's project/workspace directory) itself.
set -eu

YES=0
DRY_RUN=0
LOCAL_ONLY=0

usage() {
  cat <<'EOF'
Usage: uninstall.sh [--yes|-y] [--dry-run] [--local-only] [--help]

  --yes, -y       do not ask for confirmation (required without a TTY)
  --dry-run       show what would be removed without running commands or edits
  --local-only    skip cloud runner/token deregistration
  --help          show this help

The script removes only the current user's karma-team installation. It does
not require root and does not remove parent directories or project workspaces.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --yes|-y) YES=1 ;;
    --dry-run) DRY_RUN=1 ;;
    --local-only) LOCAL_ONLY=1 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

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

# Keep uninstall scoped to the effective user even when `su`/`sudo -u`
# preserved root's environment. In particular, never stop services through or
# remove files from another UID's XDG runtime directory.
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
      echo "Refusing to uninstall a per-user karma-team CLI as root for another user." >&2
      echo "Open that user's login shell first (for example: sudo -iu ${elevated_from:-USER}), then run the uninstaller without sudo." >&2
      exit 1
    fi
  elif [ -z "${HOME:-}" ] || [ "${HOME#/}" = "$HOME" ] || [ "$home_owner" != "$effective_uid" ]; then
    resolved_home=$(passwd_home_for_uid "$effective_uid" || true)
    if [ -z "$resolved_home" ] || [ "${resolved_home#/}" = "$resolved_home" ] ||
      [ ! -d "$resolved_home" ] ||
      [ "$(path_owner_uid "$resolved_home" || true)" != "$effective_uid" ]; then
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

normalize_user_environment

HOME_DIR=${HOME:-}
if [ -z "$HOME_DIR" ]; then
  echo "HOME is not set; refusing to guess which user's karma-team files to remove." >&2
  exit 1
fi

if [ "${KARMA_TEAM_CONFIG_DIR+x}" = x ] && [ -z "$KARMA_TEAM_CONFIG_DIR" ]; then
  echo "Refusing unsafe empty karma-team path override." >&2
  exit 1
fi
if [ "${KARMA_TEAM_DATA_DIR+x}" = x ] && [ -z "$KARMA_TEAM_DATA_DIR" ]; then
  echo "Refusing unsafe empty karma-team path override." >&2
  exit 1
fi
if [ "${KARMA_TEAM_CACHE_DIR+x}" = x ] && [ -z "$KARMA_TEAM_CACHE_DIR" ]; then
  echo "Refusing unsafe empty karma-team path override." >&2
  exit 1
fi

INSTALL_DIR=${KARMA_TEAM_INSTALL_DIR:-"$HOME_DIR/.local/bin"}
case "$INSTALL_DIR" in
  /*) ;;
  *) INSTALL_DIR="$(pwd)/$INSTALL_DIR" ;;
esac
case "$INSTALL_DIR" in
  *:* | *'
'*) echo "KARMA_TEAM_INSTALL_DIR contains a colon or newline; refusing unsafe cleanup." >&2; exit 1 ;;
  ..|../*|*/..|*/../*|.|./*|*/.|*/./*) echo "KARMA_TEAM_INSTALL_DIR contains unsafe relative components." >&2; exit 1 ;;
esac
if [ -L "$INSTALL_DIR" ]; then
  echo "Refusing to remove a binary through symlinked installation directory: $INSTALL_DIR" >&2
  exit 1
fi

if [ "${KARMA_TEAM_CONFIG_DIR+x}" = x ]; then
  CONFIG_DIR=$KARMA_TEAM_CONFIG_DIR
elif [ "${XDG_CONFIG_HOME:-}" != "" ] && case "$XDG_CONFIG_HOME" in /*) true ;; *) false ;; esac; then
  CONFIG_DIR="$XDG_CONFIG_HOME/karma-team"
else
  CONFIG_DIR="$HOME_DIR/.config/karma-team"
fi
if [ "${KARMA_TEAM_DATA_DIR+x}" = x ]; then
  DATA_DIR=$KARMA_TEAM_DATA_DIR
elif [ "${XDG_DATA_HOME:-}" != "" ] && case "$XDG_DATA_HOME" in /*) true ;; *) false ;; esac; then
  DATA_DIR="$XDG_DATA_HOME/karma-team"
else
  DATA_DIR="$HOME_DIR/.local/share/karma-team"
fi
if [ "${KARMA_TEAM_CACHE_DIR+x}" = x ]; then
  CACHE_DIR=$KARMA_TEAM_CACHE_DIR
elif [ "${XDG_CACHE_HOME:-}" != "" ] && case "$XDG_CACHE_HOME" in /*) true ;; *) false ;; esac; then
  CACHE_DIR="$XDG_CACHE_HOME/karma-team"
else
  CACHE_DIR="$HOME_DIR/.cache/karma-team"
fi
DATA_OVERRIDE=${KARMA_TEAM_DATA_DIR+x}
case "$(uname -s 2>/dev/null || printf unknown)" in
  Darwin)
    [ "${KARMA_TEAM_CONFIG_DIR+x}" = x ] || CONFIG_DIR="$HOME_DIR/Library/Application Support/so.karma-team.karma-team"
    [ "${KARMA_TEAM_DATA_DIR+x}" = x ] || DATA_DIR="$HOME_DIR/Library/Application Support/so.karma-team.karma-team"
    [ "${KARMA_TEAM_CACHE_DIR+x}" = x ] || CACHE_DIR="$HOME_DIR/Library/Caches/so.karma-team.karma-team"
    ;;
esac
RUNTIME_BASE=${XDG_RUNTIME_DIR:-}
case "$RUNTIME_BASE" in
  /*) ;;
  *) RUNTIME_BASE='' ;;
esac

case "$CONFIG_DIR" in
  /*) ;;
  *) CONFIG_DIR="$(pwd)/$CONFIG_DIR" ;;
esac
case "$DATA_DIR" in
  /*) ;;
  *) DATA_DIR="$(pwd)/$DATA_DIR" ;;
esac
case "$CACHE_DIR" in
  /*) ;;
  *) CACHE_DIR="$(pwd)/$CACHE_DIR" ;;
esac

if [ -z "$CONFIG_DIR" ] || [ -z "$DATA_DIR" ] || [ -z "$CACHE_DIR" ]; then
  echo "Refusing unsafe empty karma-team path." >&2
  exit 1
fi

for path_value in "$CONFIG_DIR" "$DATA_DIR" "$CACHE_DIR" "$RUNTIME_BASE"; do
  case "$path_value" in
    *'
'* | *'\000'*) echo "A configured karma-team path contains a control character; refusing cleanup." >&2; exit 1 ;;
  esac
done

validate_state_path() {
  path_value=$1
  case "$path_value" in
    *'$'*|*'`'*|*';'*|*'|'*|*'&'*|*'<'*|*'>'*)
      echo "Refusing unsafe karma-team path: $path_value" >&2
      exit 1
      ;;
    ..|../*|*/..|*/../*|.|./*|*/.|*/./*)
      echo "Refusing unsafe relative components in karma-team path: $path_value" >&2
      exit 1
      ;;
    ""|/|"$HOME_DIR"|"$HOME_DIR/.config"|"$HOME_DIR/.local"|"$HOME_DIR/.local/share"|"$HOME_DIR/.cache"|/tmp|/var|/usr|/etc)
      echo "Refusing to remove broad or unsafe karma-team path: $path_value" >&2
      exit 1
      ;;
  esac
  if [ -L "$path_value" ]; then
    echo "Refusing to remove symlinked karma-team path: $path_value" >&2
    exit 1
  fi
}
validate_state_path "$CONFIG_DIR"
validate_state_path "$DATA_DIR"
validate_state_path "$CACHE_DIR"
case "$CACHE_DIR" in
  */karma-team|*/so.karma-team.karma-team) ;;
  *) echo "Refusing unsafe cache path: $CACHE_DIR" >&2; exit 1 ;;
esac

SYSTEMD_UNIT="$HOME_DIR/.config/systemd/user/karma-team.service"
LAUNCHD_PLIST="$HOME_DIR/Library/LaunchAgents/so.karma-team.daemon.plist"

is_entry() { [ -e "$1" ] || [ -L "$1" ]; }

entry_is_safe_regular() {
  [ -f "$1" ] && [ ! -L "$1" ]
}

remove_file() {
  file=$1
  if entry_is_safe_regular "$file"; then
    if [ "$DRY_RUN" -eq 1 ]; then echo "Would remove $file"; else rm -f -- "$file"; fi
  elif [ -L "$file" ]; then
    echo "Skipping symlink (not following it): $file" >&2
  fi
}

remove_tree() {
  dir=$1
  if [ -L "$dir" ]; then
    echo "Skipping symlink directory (not following it): $dir" >&2
  elif [ -d "$dir" ]; then
    if [ "$DRY_RUN" -eq 1 ]; then echo "Would remove karma-team directory $dir"; else rm -rf -- "$dir"; fi
  fi
}

remove_empty_dir() {
  dir=$1
  if [ -d "$dir" ] && [ ! -L "$dir" ]; then
    if [ "$DRY_RUN" -eq 1 ]; then echo "Would remove empty directory $dir"; else rmdir -- "$dir" 2>/dev/null || :; fi
  fi
}

profile_mode() {
  stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1" 2>/dev/null || printf '644'
}

remove_profile_marker() {
  profile=$1
  if ! entry_is_safe_regular "$profile"; then
    [ -L "$profile" ] && echo "Skipping symlink profile: $profile" >&2
    return 0
  fi
  if ! grep -Fqx '# karma-team PATH' "$profile" 2>/dev/null; then return 0; fi
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "Would remove karma-team PATH block from $profile"
    return 0
  fi
  tmp="$profile.karma-team-uninstall.$$"
  awk '
    snippet_pending {
      snippet_pending = 0
      if ($0 == "") { next }
      print
      next
    }
    marker_pending {
      if ($0 ~ /^case ":\$\{PATH:-\}:" in \*:/ || $0 ~ /^case \047:\$\{PATH:-\}:\047 in \*:/) {
        marker_pending = 0
        snippet_pending = 1
        next
      }
      print "# karma-team PATH"
      marker_pending = 0
    }
    $0 == "# karma-team PATH" { marker_pending = 1; next }
    { print }
    END { if (marker_pending) print "# karma-team PATH" }
  ' "$profile" > "$tmp"
  chmod "$(profile_mode "$profile")" "$tmp" 2>/dev/null || :
  mv -f -- "$tmp" "$profile"
}

profile_cleanup() {
  remove_profile_marker "$HOME_DIR/.profile"
  remove_profile_marker "$HOME_DIR/.bashrc"
  remove_profile_marker "$HOME_DIR/.bash_profile"
  remove_profile_marker "$HOME_DIR/.bash_login"
  zsh_dir=${ZDOTDIR:-$HOME_DIR}
  remove_profile_marker "$zsh_dir/.zprofile"
  remove_profile_marker "$zsh_dir/.zshrc"
}

confirm() {
  if [ "$YES" -eq 1 ] || [ "$DRY_RUN" -eq 1 ]; then return 0; fi
  if ! (: </dev/tty) 2>/dev/null; then
    echo "No terminal detected. Re-run with --yes to confirm karma-team removal." >&2
    return 1
  fi
  printf '%s' "Remove the current user's karma-team installation, service, config, and data? [y/N] " >/dev/tty
  reply=''
  read -r reply </dev/tty || reply=''
  case "$reply" in
    y|Y|yes|YES|Yes) return 0 ;;
    *) echo "Cancelled."; return 1 ;;
  esac
}

if [ "$DRY_RUN" -eq 1 ]; then
  echo "Dry run: no commands, service actions, or file edits will be performed."
fi
confirm || exit 1

foreground_daemon_running() {
  command -v ps >/dev/null 2>&1 || return 1
  daemon_bin="$INSTALL_DIR/karma-team"
  ps -eo pid=,args= 2>/dev/null | awk -v self="$$" -v daemon_bin="$daemon_bin" '
    $0 ~ /awk -v self=/ { next }
    $1 != self && index($0, daemon_bin " __run") > 0 { found = 1 }
    END { exit found ? 0 : 1 }
  '
}

if [ "$DRY_RUN" -eq 0 ] && foreground_daemon_running; then
  echo "A foreground karma-team __run process is still active; stop it before uninstalling." >&2
  echo "Refusing to delete its configuration or data." >&2
  exit 1
fi

if [ "$DRY_RUN" -eq 1 ]; then
  for candidate in "$INSTALL_DIR/karma-team" "$INSTALL_DIR/karma-team.exe"; do
    is_entry "$candidate" && echo "Would remove $candidate"
  done
  echo "Would stop and remove any karma-team user service."
  echo "Would remove karma-team config/data files and PATH blocks."
  [ "$LOCAL_ONLY" -eq 1 ] || echo "Would deregister configured runners and revoke the CLI token."
  exit 0
fi

TMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/karma-team-uninstall.XXXXXX")
BACKUP_DIR="$TMP_ROOT/state"
OUTPUT_FILE="$TMP_ROOT/cli-output"
cleanup_tmp() { rm -rf -- "$TMP_ROOT"; }
trap cleanup_tmp EXIT
trap 'cleanup_tmp; exit 1' HUP INT TERM

restore_state() {
  mkdir -p -- "$CONFIG_DIR"
  for name in config.toml credentials.toml; do
    if [ -f "$BACKUP_DIR/$name" ]; then cp -p -- "$BACKUP_DIR/$name" "$CONFIG_DIR/$name"; fi
  done
}

backup_state() {
  mkdir -p -- "$BACKUP_DIR"
  for name in config.toml credentials.toml; do
    source="$CONFIG_DIR/$name"
    if [ -L "$source" ]; then
      echo "Refusing to operate on symlinked state file: $source" >&2
      return 1
    fi
    if [ -f "$source" ]; then cp -p -- "$source" "$BACKUP_DIR/$name"; fi
  done
}

stop_service() {
  os=$(uname -s 2>/dev/null || printf unknown)
  if [ "$os" = Linux ] && is_entry "$SYSTEMD_UNIT"; then
    if ! command -v systemctl >/dev/null 2>&1; then
      echo "systemctl is required to stop the installed karma-team service." >&2
      return 1
    fi
    set +e
    stop_output=$(systemctl --user stop karma-team.service 2>&1)
    stop_status=$?
    active_output=$(systemctl --user is-active karma-team.service 2>&1)
    active_status=$?
    set -e
    if [ "$active_status" -eq 0 ]; then
      echo "karma-team service is still active; refusing to delete its state." >&2
      return 1
    fi
    case "$active_output" in
      *inactive*|*failed*|*not\ found*|*not-found*|*"No such unit"*) ;;
      *) echo "Could not verify that karma-team service stopped; refusing to delete its state." >&2; return 1 ;;
    esac
    if [ "$stop_status" -ne 0 ]; then
      case "$stop_output" in
        *inactive*|*not\ found*|*not-found*|*"No such unit"*) ;;
        *) echo "Could not stop karma-team service; refusing to delete its state." >&2; return 1 ;;
      esac
    fi
    systemctl --user disable karma-team.service >/dev/null 2>&1 || :
    systemctl --user daemon-reload >/dev/null 2>&1 || :
    remove_file "$SYSTEMD_UNIT"
  elif [ "$os" = Darwin ] && is_entry "$LAUNCHD_PLIST" && command -v launchctl >/dev/null 2>&1; then
    uid=$(id -u)
    set +e
    launchctl bootout "gui/$uid/so.karma-team.daemon" >/dev/null 2>&1
    launchctl print "gui/$uid/so.karma-team.daemon" >/dev/null 2>&1
    active_status=$?
    set -e
    if [ "$active_status" -eq 0 ]; then
      echo "karma-team launch agent is still active; refusing to delete its state." >&2
      return 1
    fi
    if is_entry "$LAUNCHD_PLIST"; then remove_file "$LAUNCHD_PLIST"; fi
  elif [ "$os" = Darwin ] && is_entry "$LAUNCHD_PLIST"; then
    echo "launchctl is required to stop the installed karma-team service." >&2
    return 1
  fi
}

find_cli() {
  for candidate in "$INSTALL_DIR/karma-team" "$INSTALL_DIR/karma-team.exe"; do
    if entry_is_safe_regular "$candidate" && [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

remove_cloud_and_cli_state() {
  cli=''
  cli=$(find_cli || true)
  had_config=0
  [ -f "$CONFIG_DIR/config.toml" ] && had_config=1
  if [ -z "$cli" ]; then
    if [ "$LOCAL_ONLY" -eq 0 ]; then
      echo "karma-team binary was not found; cannot deregister cloud runners." >&2
      echo "Re-run with --local-only only if cloud records will be removed separately." >&2
      return 1
    fi
    return 0
  fi
  backup_state
  set +e
  if [ "$LOCAL_ONLY" -eq 1 ]; then
    "$cli" remove --all --local-only >"$OUTPUT_FILE" 2>&1
  else
    "$cli" remove --all >"$OUTPUT_FILE" 2>&1
  fi
  cli_status=$?
  set -e
  if [ "$cli_status" -ne 0 ]; then
    restore_state
    echo "karma-team refused to remove its state; no further files were deleted." >&2
    return 1
  fi
  if [ "$LOCAL_ONLY" -eq 0 ] && grep -Eiq 'cloud cleanup failed|cloud delete failed|cloud token revoke failed|could not read cli token|no cli token configured|cloud cleanup was skipped' "$OUTPUT_FILE"; then
    restore_state
    echo "cloud cleanup was incomplete; local config was restored and data was kept. The service remains stopped." >&2
    return 1
  fi
  if [ "$LOCAL_ONLY" -eq 0 ] && [ "$had_config" -eq 1 ] && ! grep -Eiq 'cloud runners deleted and dev-machine token revoked' "$OUTPUT_FILE"; then
    restore_state
    echo "karma-team did not confirm cloud deregistration; local config was restored and data was kept. The service remains stopped." >&2
    return 1
  fi
}

stop_service
remove_cloud_and_cli_state

for candidate in "$INSTALL_DIR/karma-team" "$INSTALL_DIR/karma-team.exe"; do
  remove_file "$candidate"
done

if [ -d "$CONFIG_DIR" ] && [ ! -L "$CONFIG_DIR" ]; then
  remove_file "$CONFIG_DIR/config.toml"
  remove_file "$CONFIG_DIR/credentials.toml"
  remove_file "$CONFIG_DIR/.config.lock"
  remove_empty_dir "$CONFIG_DIR"
fi

if [ -d "$DATA_DIR" ] && [ ! -L "$DATA_DIR" ]; then
  # `workspaces` and `worktrees` intentionally remain: they may contain user
  # checkouts. Every other child below is owned by the runner itself.
  for child in logs runners history cache runtime; do
    remove_tree "$DATA_DIR/$child"
  done
  for child in .update.lock pid karma-team.sock; do
    remove_file "$DATA_DIR/$child"
  done
  remove_empty_dir "$DATA_DIR"
fi

if [ -n "$RUNTIME_BASE" ] && [ -z "$DATA_OVERRIDE" ]; then
  # Only clean the canonical per-user Linux runtime location. XDG_RUNTIME_DIR
  # is caller-controlled; accepting an arbitrary owned directory such as
  # $HOME would make this remove an unrelated $HOME/karma-team checkout.
  # Non-standard runtime roots are left intact rather than risking user data.
  expected_runtime_base="/run/user/$(id -u)"
  if [ "$RUNTIME_BASE" = "$expected_runtime_base" ] &&
    runtime_base_is_usable "$RUNTIME_BASE" "$(id -u)"; then
    remove_tree "$RUNTIME_BASE/karma-team"
  else
    printf 'Skipping non-standard runtime cleanup: %s/karma-team\n' "$RUNTIME_BASE" >&2
  fi
fi

remove_tree "$CACHE_DIR"

profile_cleanup
echo "karma-team has been uninstalled for the current user."
