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
// types
import { Button } from "@karma-team/propel/button";
import type { IEmailCheckData } from "@karma-team/types";
// ui
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
    <form onSubmit={handleFormSubmit} className="mt-5 space-y-4">
      <div className="space-y-1">
        <label className="text-13 font-medium text-tertiary" htmlFor="email">
          Email or username
        </label>
        <div className="relative flex items-center rounded-md border border-subtle bg-surface-1">
          <Input
            id="email"
            name="email"
            type="text"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            placeholder="Email or username"
            className={`h-10 w-full border-0 disable-autofill-style placeholder:text-placeholder autofill:bg-danger-subtle focus:bg-none active:bg-transparent`}
            autoComplete="off"
            autoFocus
            ref={inputRef}
          />
          {email.length > 0 && (
            <button
              type="button"
              aria-label="Clear email or username"
              onClick={() => {
                setEmail("");
                inputRef.current?.focus();
              }}
              tabIndex={-1}
            >
              <XCircle className="h-10 w-11 stroke-placeholder px-3 text-11 hover:cursor-pointer" />
            </button>
          )}
        </div>
      </div>
      <Button type="submit" variant="primary" className="w-full" size="xl" disabled={isButtonDisabled}>
        {isSubmitting ? <Spinner height="20px" width="20px" /> : "Continue"}
      </Button>
    </form>
  );
});
