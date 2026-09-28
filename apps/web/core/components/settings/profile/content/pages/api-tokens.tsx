/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useState } from "react";
import { observer } from "mobx-react";
import useSWR from "swr";
// karma-team imports
import { useTranslation } from "@karma-team/i18n";
import { Button } from "@karma-team/propel/button";
import { EmptyStateCompact } from "@karma-team/propel/empty-state";
import { APITokenService } from "@karma-team/services";
// components
import { CreateApiTokenModal } from "@/components/api-token/modal/create-token-modal";
import { ApiTokenListItem } from "@/components/api-token/token-list-item";
import { ProfileSettingsHeading } from "@/components/settings/profile/heading";
import { APITokenSettingsLoader } from "@/components/ui/loader/settings/api-token";
// constants
import { API_TOKENS_LIST } from "@/constants/fetch-keys";

const apiTokenService = new APITokenService();

export const APITokensProfileSettings = observer(function APITokensProfileSettings() {
  // states
  const [isCreateTokenModalOpen, setIsCreateTokenModalOpen] = useState(false);
  // store hooks
  const { data: tokens } = useSWR(API_TOKENS_LIST, () => apiTokenService.list());
  // translation
  const { t } = useTranslation();

  if (!tokens) {
    return <APITokenSettingsLoader />;
  }

  return (
    <div className="size-full">
      <CreateApiTokenModal isOpen={isCreateTokenModalOpen} onClose={() => setIsCreateTokenModalOpen(false)} />
      <ProfileSettingsHeading
        title={t("Personal Access Tokens")}
        description={t("Generate secure API tokens to integrate your data with external systems and applications.")}
        control={
          <Button variant="primary" size="lg" onClick={() => setIsCreateTokenModalOpen(true)}>
            {t("Add personal access token")}
          </Button>
        }
      />
      <div className="mt-7">
        {tokens.length > 0 ? (
          <>
            <div>
              {tokens.map((token) => (
                <ApiTokenListItem key={token.id} token={token} />
              ))}
            </div>
          </>
        ) : (
          <EmptyStateCompact
            assetKey="token"
            assetClassName="size-20"
            title={t("No Personal token yet")}
            description={t(
              "Generate secure API tokens to connect your workspace with external systems and applications."
            )}
            actions={[
              {
                label: t("Add API token"),
                onClick: () => {
                  setIsCreateTokenModalOpen(true);
                },
              },
            ]}
            align="start"
            rootClassName="py-20"
          />
        )}
      </div>
    </div>
  );
});
