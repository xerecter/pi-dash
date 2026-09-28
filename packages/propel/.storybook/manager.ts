/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { addons } from "storybook/manager-api";
import { create } from "storybook/theming";

const karmaTeamTheme = create({
  base: "dark",
  brandTitle: "karma-team UI",
  brandUrl: "https://airepublic.com",
  brandImage: "karma-team-lockup-light.svg",
  brandTarget: "_self",
});

addons.setConfig({
  theme: karmaTeamTheme,
});
