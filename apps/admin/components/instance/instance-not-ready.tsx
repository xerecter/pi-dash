/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { observer } from "mobx-react";
import Link from "next/link";
import { useTheme } from "next-themes";
import { Button } from "@karma-team/propel/button";
// assets
import KarmaTeamSymbolDark from "@/app/assets/logos/karma-team-symbol-dark.svg?url";
import KarmaTeamSymbolLight from "@/app/assets/logos/karma-team-symbol-light.svg?url";

export const InstanceNotReady = observer(function InstanceNotReady() {
  const { resolvedTheme } = useTheme();
  const karmaTeamSymbol = resolvedTheme === "dark" ? KarmaTeamSymbolDark : KarmaTeamSymbolLight;

  return (
    <div className="relative container mx-auto flex h-full w-full items-center justify-center px-5">
      <div className="relative w-auto max-w-2xl space-y-8 py-10">
        <div className="relative flex flex-col items-center justify-center space-y-4">
          <h1 className="pb-3 text-24 font-bold">Welcome aboard karma-team!</h1>
          <img src={karmaTeamSymbol} alt="karma-team logo" className="h-32 w-auto" />
          <p className="text-14 font-medium text-placeholder">Get started by setting up your instance and workspace</p>
        </div>

        <div>
          <Link href={"/setup/?auth_enabled=0"}>
            <Button size="xl" className="w-full">
              Get started
            </Button>
          </Link>
        </div>
      </div>
    </div>
  );
});
