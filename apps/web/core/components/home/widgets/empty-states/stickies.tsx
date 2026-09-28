/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useTranslation } from "@karma-team/i18n";
import { EmptyStateCompact } from "@karma-team/propel/empty-state";

export function StickiesEmptyState() {
  const { t } = useTranslation();
  return (
    <div className="flex w-full items-center justify-center rounded-lg bg-layer-1 py-10">
      <EmptyStateCompact
        assetKey="note"
        assetClassName="size-20"
        title={t("Jot down an idea, capture an aha, or record a brainwave. Add a sticky to get started.")}
      />
    </div>
  );
}
