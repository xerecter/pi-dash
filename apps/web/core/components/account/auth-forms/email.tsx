/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import type { FormEvent } from "react";
import { useRef, useState } from "react";
import { observer } from "mobx-react";
// icons
import { XCircle } from "lucide-react";
// karma-team imports
import { useTranslation } from "@karma-team/i18n";
import { Button } from "@karma-team/propel/button";
import type { IEmailCheckData } from "@karma-team/types";
import { Input, Spinner } from "@karma-team/ui";
// helpers
type TAuthEmailForm = {
  defaultEmail: string;
  onSubmit: (data: IEmailCheckData) => Promise<void>;
};

export const AuthEmailForm = observer(function AuthEmailForm(props: TAuthEmailForm) {
  const { onSubmit, defaultEmail } = props;
  // states
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [email, setEmail] = useState(defaultEmail);
  // karma-team hooks
  const { t } = useTranslation();

  const handleFormSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!email.trim() || isSubmitting) return;
    setIsSubmitting(true);
    const payload: IEmailCheckData = {
      email: email.trim(),
    };
    try {
      await onSubmit(payload);
    } finally {
      setIsSubmitting(false);
    }
  };

  const isButtonDisabled = email.trim().length === 0 || isSubmitting;
  const inputRef = useRef<HTMLInputElement>(null);

  return (
    <form onSubmit={handleFormSubmit} className="space-y-4">
      <div className="space-y-1">
        <label htmlFor="email" className="text-13 font-medium text-tertiary">
          {t("Email or username")}
        </label>
        <div className="relative flex items-center rounded-md border border-strong bg-surface-1">
          <Input
            id="email"
            name="email"
            type="text"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            placeholder={t("Email or username")}
            className={`h-10 w-full border-0 disable-autofill-style placeholder:text-placeholder autofill:bg-danger-primary focus:bg-none active:bg-transparent`}
            autoComplete="off"
            autoFocus
            ref={inputRef}
          />
          {email.length > 0 && (
            <button
              type="button"
              onClick={() => {
                setEmail("");
                inputRef.current?.focus();
              }}
              className="absolute right-3 grid size-5 place-items-center"
              aria-label={t("Clear email or username")}
              tabIndex={-1}
            >
              <XCircle className="size-5 stroke-placeholder" />
            </button>
          )}
        </div>
      </div>
      <Button type="submit" variant="primary" className="w-full" size="xl" disabled={isButtonDisabled}>
        {isSubmitting ? <Spinner height="20px" width="20px" /> : t("Continue")}
      </Button>
    </form>
  );
});
