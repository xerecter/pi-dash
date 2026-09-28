/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import * as React from "react";

import type { ISvgIcons } from "../type";

export function KarmaTeamWordmark({ width = "146", height = "44", className, color = "currentColor" }: ISvgIcons) {
  return (
    <svg
      width={width}
      height={height}
      viewBox="0 0 146 44"
      fill={color}
      xmlns="http://www.w3.org/2000/svg"
      className={className}
      role="img"
      aria-label="karma-team"
    >
      <text
        x="1"
        y="31"
        fontFamily="Inter, sans-serif"
        fontSize="24"
        fontWeight="600"
        textLength="144"
        lengthAdjust="spacingAndGlyphs"
      >
        karma-team
      </text>
    </svg>
  );
}
