#!/usr/bin/env bash
# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.
#
# Build this checkout's runner and stage a pinned upstream agent engine.
set -euo pipefail
desktop_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
oss_dir="$(cd -- "$desktop_dir/.." && pwd)"
engine_tag="${CODEX_BUNDLE_VERSION:-rust-v0.153.4}"
runner_profile="${KARMA_TEAM_RUNNER_PROFILE:-dev}"
target="$(rustc -vV | sed -n 's/^host: //p')"
suffix=""
case "$target" in
  *-unknown-linux-gnu) engine_target="${target%-gnu}-musl" ;;
  *-apple-darwin) engine_target="$target" ;;
  *-pc-windows-msvc) engine_target="$target"; suffix=".exe" ;;
  *) echo "Unsupported desktop build target: $target" >&2; exit 1 ;;
esac
bin_dir="$desktop_dir/src-tauri/bin"
license_dir="$desktop_dir/src-tauri/licenses"
mkdir -p "$bin_dir" "$license_dir"
cargo build --manifest-path "$oss_dir/Cargo.toml" --profile "$runner_profile" --bin karma-team
output_profile="$runner_profile"
if [[ "$runner_profile" == dev ]]; then output_profile=debug; fi
cp "$oss_dir/target/$output_profile/karma-team$suffix" "$bin_dir/karma-team$suffix"
cp "$oss_dir/LICENSE.txt" "$license_dir/karma-team-LICENSE.txt"

if [[ ! -f "$bin_dir/engine-version.txt" ]] || [[ "$(<"$bin_dir/engine-version.txt")" != "$engine_tag" ]] || [[ ! -x "$bin_dir/karma-team-agent-engine$suffix" ]]; then
  task_tmp="$(mktemp -d)"
  asset="codex-$engine_target$suffix.tar.gz"
  gh release download "$engine_tag" --repo openai/codex --pattern "$asset" --dir "$task_tmp"
  digest="$(gh api "repos/openai/codex/releases/tags/$engine_tag" --jq ".assets[] | select(.name == \"$asset\") | .digest")"
  if [[ "$digest" != sha256:* ]]; then echo "Missing upstream checksum for $asset" >&2; exit 1; fi
  actual="$(shasum -a 256 "$task_tmp/$asset" | cut -d ' ' -f 1)"
  if [[ "sha256:$actual" != "$digest" ]]; then echo "Agent archive checksum mismatch" >&2; exit 1; fi
  tar -xf "$task_tmp/$asset" -C "$task_tmp"
  cp "$task_tmp/codex-$engine_target$suffix" "$bin_dir/karma-team-agent-engine$suffix"
  chmod +x "$bin_dir/karma-team-agent-engine$suffix"
  gh api "repos/openai/codex/contents/LICENSE?ref=$engine_tag" -H 'Accept: application/vnd.github.raw+json' > "$license_dir/agent-engine-LICENSE.txt"
  printf '%s\n' "$engine_tag" > "$bin_dir/engine-version.txt"
  printf 'karma-team Agent includes OpenAI Codex (%s), licensed under Apache-2.0. The upstream binary is renamed to karma-team-agent-engine.\n' "$engine_tag" > "$license_dir/agent-engine-NOTICE.txt"
fi
"$bin_dir/karma-team-agent-engine$suffix" --version
