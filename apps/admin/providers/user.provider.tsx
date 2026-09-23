/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useEffect } from "react";
import { observer } from "mobx-react";
import useSWR from "swr";
// hooks
import { useInstance, useTheme, useUser } from "@/hooks/store";

export const UserProvider = observer(function UserProvider({ children }: React.PropsWithChildren) {
  // hooks
  const { isSidebarCollapsed, toggleSidebar } = useTheme();
  const { currentUser, fetchCurrentUser } = useUser();
  const { instance, fetchInstanceAdmins } = useInstance();
  const isSetupDone = instance?.is_setup_done === true;

  useSWR(isSetupDone ? "CURRENT_USER" : null, () => fetchCurrentUser(), {
    shouldRetryOnError: false,
  });

  useSWR(isSetupDone ? "INSTANCE_ADMINS" : null, () => fetchInstanceAdmins());

  useEffect(() => {
    const localValue = localStorage && localStorage.getItem("god_mode_sidebar_collapsed");
    const localBoolValue = localValue === "true";
    if (isSidebarCollapsed === undefined && localBoolValue != isSidebarCollapsed) toggleSidebar(localBoolValue);
  }, [isSidebarCollapsed, currentUser, toggleSidebar]);

  return <>{children}</>;
});
