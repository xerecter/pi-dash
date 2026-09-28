/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { observer } from "mobx-react";
import Link from "next/link";
import { useTheme as useNextTheme } from "next-themes";
// ui
import { Button, getButtonStyling } from "@karma-team/propel/button";
import { resolveGeneralTheme } from "@karma-team/utils";
// hooks
import KarmaTeamSymbolDark from "@/app/assets/logos/karma-team-symbol-dark.svg?url";
import KarmaTeamSymbolLight from "@/app/assets/logos/karma-team-symbol-light.svg?url";
import { useTheme } from "@/hooks/store";
// icons

export const NewUserPopup = observer(function NewUserPopup() {
  // hooks
  const { isNewUserPopup, toggleNewUserPopup } = useTheme();
  // theme
  const { resolvedTheme } = useNextTheme();

  if (!isNewUserPopup) return <></>;
  return (
    <div className="shadow-md absolute right-8 bottom-8 w-96 rounded-lg border border-subtle bg-surface-1 p-6">
      <div className="flex gap-4">
        <div className="grow">
          <div className="text-14 font-semibold">Create workspace</div>
          <div className="py-2 text-13 font-medium text-tertiary">
            Instance setup done! Welcome to karma-team instance portal. Start your journey with by creating your first
            workspace.
          </div>
          <div className="flex items-center gap-4 pt-2">
            <Link href="/workspace/create" className={getButtonStyling("primary", "lg")}>
              Create workspace
            </Link>
            <Button variant="secondary" size="lg" onClick={toggleNewUserPopup}>
              Close
            </Button>
          </div>
        </div>
        <div className="flex shrink-0 items-center justify-center">
          <img
            src={resolveGeneralTheme(resolvedTheme) === "dark" ? KarmaTeamSymbolDark : KarmaTeamSymbolLight}
            height={80}
            width={147}
            alt="karma-team icon"
          />
        </div>
      </div>
    </div>
  );
});
