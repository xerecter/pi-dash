/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { RotateCw } from "lucide-react";
import { useTranslation } from "@karma-team/i18n";
import { Button } from "@karma-team/propel/button";

export function CliInstallConfigStatus({ error, onRetry }: { error: unknown; onRetry: () => void }) {
  const { t } = useTranslation();

  return error ? (
    <div role="alert" className="flex flex-wrap items-center gap-3 text-13 text-danger-primary">
      <p>{t("Could not load CLI installation configuration.")}</p>
      <Button variant="secondary" size="sm" prependIcon={<RotateCw />} onClick={onRetry}>
        {t("Retry")}
      </Button>
    </div>
  ) : (
    <p role="status" className="text-13 text-secondary">
      {t("Loading CLI installation configuration...")}
    </p>
  );
}
