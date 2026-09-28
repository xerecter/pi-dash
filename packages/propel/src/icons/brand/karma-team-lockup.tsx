/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import type { ISvgIcons } from "../type";
import { KarmaTeamSymbol } from "./karma-team-symbol";

/**
 * Header / lockup brand mark.
 *
 * Aliased to ``KarmaTeamSymbol`` so the project ships a single canonical
 * mark — the same one the root README points at. Existing callsites
 * keep working unchanged; they render the Pi symbol instead of the
 * older "karma-team" wordmark.
 */
export function KarmaTeamLockup(props: ISvgIcons) {
  return <KarmaTeamSymbol {...props} />;
}
