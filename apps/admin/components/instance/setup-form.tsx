/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "next/navigation";
// icons
import { Eye, EyeOff } from "lucide-react";
// karma-team internal packages
import { API_BASE_URL, E_PASSWORD_STRENGTH } from "@karma-team/constants";
import { Button } from "@karma-team/propel/button";
import { AuthService } from "@karma-team/services";
import { Checkbox, Input, PasswordStrengthIndicator, Spinner } from "@karma-team/ui";
import { getPasswordStrength, validatePersonName, validateCompanyName } from "@karma-team/utils";
// components
import { AuthHeader } from "@/app/(all)/(home)/auth-header";
import { Banner } from "../common/banner";
import { FormHeader } from "./form-header";

// service initialization
const authService = new AuthService();

// error codes
enum EErrorCodes {
  INSTANCE_NOT_CONFIGURED = "5000",
  INVALID_EMAIL = "5005",
  PASSWORD_TOO_WEAK = "5021",
  ADMIN_ALREADY_EXIST = "5150",
  REQUIRED_EMAIL_PASSWORD_FIRST_NAME = "5155",
  INVALID_ADMIN_EMAIL = "5160",
  INVALID_ADMIN_PASSWORD = "5165",
  ADMIN_USER_ALREADY_EXIST = "5180",
}

type TError = {
  type: EErrorCodes | undefined;
  message: string | undefined;
};

// form data
type TFormData = {
  first_name: string;
  last_name: string;
  email: string;
  company_name: string;
  password: string;
  confirm_password?: string;
  is_telemetry_enabled: boolean;
};

const defaultFromData: TFormData = {
  first_name: "",
  last_name: "",
  email: "",
  company_name: "",
  password: "",
  confirm_password: "",
  is_telemetry_enabled: true,
};

const errorMessages: Partial<Record<EErrorCodes, string>> = {
  [EErrorCodes.INSTANCE_NOT_CONFIGURED]: "Instance not configured. Please contact your administrator.",
  [EErrorCodes.PASSWORD_TOO_WEAK]: "Please use a stronger password.",
  [EErrorCodes.ADMIN_ALREADY_EXIST]: "Admin already exists. Please try again.",
  [EErrorCodes.REQUIRED_EMAIL_PASSWORD_FIRST_NAME]: "Email, password and first name are required. Please try again.",
  [EErrorCodes.INVALID_EMAIL]: "Invalid email. Please try again.",
  [EErrorCodes.INVALID_ADMIN_EMAIL]: "Invalid admin email. Please try again.",
  [EErrorCodes.INVALID_ADMIN_PASSWORD]: "Invalid admin password. Please try again.",
  [EErrorCodes.ADMIN_USER_ALREADY_EXIST]: "Admin user already exists. Please sign in.",
};

export function InstanceSetupForm() {
  // search params
  const searchParams = useSearchParams();
  const firstNameParam = searchParams?.get("first_name") || undefined;
  const lastNameParam = searchParams?.get("last_name") || undefined;
  const companyParam = searchParams?.get("company_name") || searchParams?.get("company") || undefined;
  const emailParam = searchParams?.get("email") || undefined;
  // Honor the URL param; defaults to true when the param is absent so the
  // setup form shows telemetry enabled by default.
  const isTelemetryEnabledParam = searchParams?.get("is_telemetry_enabled") !== "False";
  const errorCode = searchParams?.get("error_code") || undefined;
  const errorMessage = searchParams?.get("error_message") || undefined;
  // state
  const [showPassword, setShowPassword] = useState({
    password: false,
    retypePassword: false,
  });
  const [csrfToken, setCsrfToken] = useState<string | undefined>(undefined);
  const [formData, setFormData] = useState<TFormData>(defaultFromData);
  const [isPasswordInputFocused, setIsPasswordInputFocused] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [isRetryPasswordInputFocused, setIsRetryPasswordInputFocused] = useState(false);

  const handleShowPassword = (key: keyof typeof showPassword) =>
    setShowPassword((prev) => ({ ...prev, [key]: !prev[key] }));

  const handleFormChange = (key: keyof TFormData, value: string | boolean) =>
    setFormData((prev) => ({ ...prev, [key]: value }));

  useEffect(() => {
    if (csrfToken === undefined)
      authService.requestCSRFToken().then((data) => data?.csrf_token && setCsrfToken(data.csrf_token));
  }, [csrfToken]);

  useEffect(() => {
    if (firstNameParam) setFormData((prev) => ({ ...prev, first_name: firstNameParam }));
    if (lastNameParam) setFormData((prev) => ({ ...prev, last_name: lastNameParam }));
    if (companyParam) setFormData((prev) => ({ ...prev, company_name: companyParam }));
    if (emailParam) setFormData((prev) => ({ ...prev, email: emailParam }));
    // Always propagate the telemetry param — `false` must override the default
    // `true`, and when the param is absent the computed value is already `true`
    // so this assignment is idempotent.
    setFormData((prev) => ({ ...prev, is_telemetry_enabled: isTelemetryEnabledParam }));
  }, [firstNameParam, lastNameParam, companyParam, emailParam, isTelemetryEnabledParam]);

  // derived values
  const errorData: TError = useMemo(() => {
    if (!errorCode || !errorMessage) return { type: undefined, message: undefined };

    const type = errorCode as EErrorCodes;
    return { type, message: errorMessages[type] ?? errorMessage };
  }, [errorCode, errorMessage]);

  const isButtonDisabled = useMemo(
    () =>
      !(
        !isSubmitting &&
        formData.first_name &&
        formData.email.trim() &&
        formData.password &&
        getPasswordStrength(formData.password) === E_PASSWORD_STRENGTH.STRENGTH_VALID &&
        formData.password === formData.confirm_password
      ),
    [formData.confirm_password, formData.email, formData.first_name, formData.password, isSubmitting]
  );

  const password = formData?.password ?? "";
  const confirmPassword = formData?.confirm_password ?? "";
  const renderPasswordMatchError = !isRetryPasswordInputFocused || confirmPassword.length >= password.length;

  return (
    <>
      <AuthHeader />
      <div className="mt-10 flex w-full flex-grow flex-col items-center justify-center py-6">
        <div className="relative flex w-full max-w-[22.5rem] flex-col gap-6">
          <FormHeader
            heading="Setup your karma-team Instance"
            subHeading="Post setup you will be able to manage this karma-team instance."
          />
          {errorData.type &&
            errorData?.message &&
            ![EErrorCodes.INVALID_EMAIL, EErrorCodes.INVALID_ADMIN_PASSWORD].includes(errorData.type) && (
              <Banner type="error" message={errorData?.message} />
            )}
          <form
            className="space-y-4"
            method="POST"
            action={`${API_BASE_URL}/api/instances/admins/sign-up/`}
            onSubmit={() => setIsSubmitting(true)}
            onError={() => setIsSubmitting(false)}
          >
            <input type="hidden" name="csrfmiddlewaretoken" value={csrfToken ?? ""} />
            <input type="hidden" name="is_telemetry_enabled" value={formData.is_telemetry_enabled ? "True" : "False"} />

            <div className="flex flex-col items-center gap-4 sm:flex-row">
              <div className="w-full space-y-1">
                <label className="text-13 font-medium text-tertiary" htmlFor="first_name">
                  First name <span className="text-danger-primary">*</span>
                </label>
                <Input
                  className="w-full border border-subtle !bg-surface-1 placeholder:text-placeholder"
                  id="first_name"
                  name="first_name"
                  type="text"
                  inputSize="md"
                  placeholder="Wilber"
                  value={formData.first_name}
                  onChange={(e) => {
                    const validation = validatePersonName(e.target.value);
                    if (validation === true || e.target.value === "") {
                      handleFormChange("first_name", e.target.value);
                    }
                  }}
                  autoComplete="off"
                  // autoFocus is intentional: this is the first field of the
                  // standalone instance-setup page so keyboard flow starts here.
                  // eslint-disable-next-line jsx-a11y/no-autofocus
                  autoFocus
                  maxLength={50}
                />
              </div>
              <div className="w-full space-y-1">
                <label className="text-13 font-medium text-tertiary" htmlFor="last_name">
                  Last name <span className="text-danger-primary">*</span>
                </label>
                <Input
                  className="w-full border border-subtle !bg-surface-1 placeholder:text-placeholder"
                  id="last_name"
                  name="last_name"
                  type="text"
                  inputSize="md"
                  placeholder="Wright"
                  value={formData.last_name}
                  onChange={(e) => {
                    const validation = validatePersonName(e.target.value);
                    if (validation === true || e.target.value === "") {
                      handleFormChange("last_name", e.target.value);
                    }
                  }}
                  autoComplete="off"
                  maxLength={50}
                />
              </div>
            </div>

            <div className="w-full space-y-1">
              <label className="text-13 font-medium text-tertiary" htmlFor="email">
                Email or username <span className="text-danger-primary">*</span>
              </label>
              <Input
                className="w-full border border-subtle !bg-surface-1 placeholder:text-placeholder"
                id="email"
                name="email"
                type="text"
                inputSize="md"
                placeholder="Email or username"
                value={formData.email}
                onChange={(e) => handleFormChange("email", e.target.value)}
                hasError={Boolean(errorData.type && errorData.type === EErrorCodes.INVALID_EMAIL)}
                autoComplete="off"
              />
              {errorData.type && errorData.type === EErrorCodes.INVALID_EMAIL && errorData.message && (
                <p className="px-1 text-11 text-danger-primary">{errorData.message}</p>
              )}
            </div>

            <div className="w-full space-y-1">
              <label className="text-13 font-medium text-tertiary" htmlFor="company_name">
                Company name <span className="text-danger-primary">*</span>
              </label>
              <Input
                className="w-full border border-subtle !bg-surface-1 placeholder:text-placeholder"
                id="company_name"
                name="company_name"
                type="text"
                inputSize="md"
                placeholder="Company name"
                value={formData.company_name}
                onChange={(e) => {
                  const validation = validateCompanyName(e.target.value, false);
                  if (validation === true || e.target.value === "") {
                    handleFormChange("company_name", e.target.value);
                  }
                }}
                maxLength={80}
              />
            </div>

            <div className="w-full space-y-1">
              <label className="text-13 font-medium text-tertiary" htmlFor="password">
                Set a password <span className="text-danger-primary">*</span>
              </label>
              <div className="relative">
                <Input
                  className="w-full border border-subtle !bg-surface-1 placeholder:text-placeholder"
                  id="password"
                  name="password"
                  type={showPassword.password ? "text" : "password"}
                  inputSize="md"
                  placeholder="New password"
                  value={formData.password}
                  onChange={(e) => handleFormChange("password", e.target.value)}
                  hasError={Boolean(errorData.type && errorData.type === EErrorCodes.INVALID_ADMIN_PASSWORD)}
                  onFocus={() => setIsPasswordInputFocused(true)}
                  onBlur={() => setIsPasswordInputFocused(false)}
                  autoComplete="new-password"
                />
                {showPassword.password ? (
                  <button
                    type="button"
                    tabIndex={-1}
                    className="absolute top-3.5 right-3 flex items-center justify-center text-placeholder"
                    onClick={() => handleShowPassword("password")}
                  >
                    <EyeOff className="h-4 w-4" />
                  </button>
                ) : (
                  <button
                    type="button"
                    tabIndex={-1}
                    className="absolute top-3.5 right-3 flex items-center justify-center text-placeholder"
                    onClick={() => handleShowPassword("password")}
                  >
                    <Eye className="h-4 w-4" />
                  </button>
                )}
              </div>
              {errorData.type && errorData.type === EErrorCodes.INVALID_ADMIN_PASSWORD && errorData.message && (
                <p className="px-1 text-11 text-danger-primary">{errorData.message}</p>
              )}
              <PasswordStrengthIndicator password={formData.password} isFocused={isPasswordInputFocused} />
            </div>

            <div className="w-full space-y-1">
              <label className="text-13 font-medium text-tertiary" htmlFor="confirm_password">
                Confirm password <span className="text-danger-primary">*</span>
              </label>
              <div className="relative">
                <Input
                  type={showPassword.retypePassword ? "text" : "password"}
                  id="confirm_password"
                  name="confirm_password"
                  inputSize="md"
                  value={formData.confirm_password}
                  onChange={(e) => handleFormChange("confirm_password", e.target.value)}
                  placeholder="Confirm password"
                  className="w-full border border-subtle !bg-surface-1 pr-12 placeholder:text-placeholder"
                  onFocus={() => setIsRetryPasswordInputFocused(true)}
                  onBlur={() => setIsRetryPasswordInputFocused(false)}
                  autoComplete="new-password"
                />
                {showPassword.retypePassword ? (
                  <button
                    type="button"
                    tabIndex={-1}
                    className="absolute top-3.5 right-3 flex items-center justify-center text-placeholder"
                    onClick={() => handleShowPassword("retypePassword")}
                  >
                    <EyeOff className="h-4 w-4" />
                  </button>
                ) : (
                  <button
                    type="button"
                    tabIndex={-1}
                    className="absolute top-3.5 right-3 flex items-center justify-center text-placeholder"
                    onClick={() => handleShowPassword("retypePassword")}
                  >
                    <Eye className="h-4 w-4" />
                  </button>
                )}
              </div>
              {!!formData.confirm_password &&
                formData.password !== formData.confirm_password &&
                renderPasswordMatchError && (
                  <span className="text-13 text-danger-primary">Passwords don{"'"}t match</span>
                )}
            </div>

            <div className="relative flex gap-2">
              <div>
                <Checkbox
                  className="h-4 w-4"
                  iconClassName="w-3 h-3"
                  id="is_telemetry_enabled"
                  onChange={() => handleFormChange("is_telemetry_enabled", !formData.is_telemetry_enabled)}
                  checked={formData.is_telemetry_enabled}
                />
              </div>
              <label className="cursor-pointer text-13 font-medium text-tertiary" htmlFor="is_telemetry_enabled">
                Allow karma-team to anonymously collect usage events.{" "}
                <a
                  tabIndex={-1}
                  href="https://github.com/xerecter/karma-team#readme"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-blue-500 hover:text-blue-600 flex-shrink-0 text-13 font-medium"
                >
                  See More
                </a>
              </label>
            </div>

            <div className="py-2">
              <Button type="submit" size="xl" className="w-full" disabled={isButtonDisabled}>
                {isSubmitting ? <Spinner height="20px" width="20px" /> : "Continue"}
              </Button>
            </div>
          </form>
        </div>
      </div>
    </>
  );
}
