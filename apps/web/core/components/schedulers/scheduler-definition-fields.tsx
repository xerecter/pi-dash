import { Controller } from "react-hook-form";
import type { Control, FieldErrors, FieldValues, Path } from "react-hook-form";
import { useTranslation } from "@karma-team/i18n";
import { Input, TextArea } from "@karma-team/ui";
import { SCHEDULER_COLOR_PALETTE as COLOR_PALETTE } from "@/components/project/scheduler-bindings/constants";

const SLUG_PATTERN = /^[a-z0-9][a-z0-9-]*$/;

type Props<T extends FieldValues> = {
  control: Control<T>;
  errors: FieldErrors<T>;
  nameName: Path<T>;
  slugName: Path<T>;
  descriptionName: Path<T>;
  promptName: Path<T>;
  colorName: Path<T>;
  slugDisabled?: boolean;
  /** Hidden create-tab fields stay registered, so validation is mode-aware. */
  isActive?: () => boolean;
};

/** Definition fields shared by workspace and project scheduler forms. */
export function SchedulerDefinitionFields<T extends FieldValues>({
  control,
  errors,
  nameName,
  slugName,
  descriptionName,
  promptName,
  colorName,
  slugDisabled,
  isActive,
}: Props<T>) {
  const { t } = useTranslation();
  const active = () => (isActive ? isActive() : true);

  // FieldErrors is recursive and cannot resolve a runtime Path<T> statically.
  const errorMap = errors as Record<string, { message?: unknown } | undefined>;
  const nameError = errorMap[nameName as string];
  const slugError = errorMap[slugName as string];
  const promptError = errorMap[promptName as string];

  return (
    <>
      <div className="flex flex-col gap-1">
        <label htmlFor="scheduler-name" className="text-13 font-medium text-primary">
          {t("Name")}
        </label>
        <Controller
          control={control}
          name={nameName}
          rules={{
            validate: (value) => !active() || String(value ?? "").trim().length > 0 || t("Name is required."),
          }}
          render={({ field: { value, onChange, ref } }) => (
            <Input
              id="scheduler-name"
              name="name"
              type="text"
              value={value}
              onChange={onChange}
              ref={ref}
              hasError={Boolean(nameError)}
              placeholder={t("Security audit")}
              className="w-full"
            />
          )}
        />
        {Boolean(nameError?.message) && <p className="text-12 text-danger-primary">{String(nameError?.message)}</p>}
      </div>

      <div className="flex flex-col gap-1">
        <label htmlFor="scheduler-slug" className="text-13 font-medium text-primary">
          {t("Slug")}
        </label>
        <Controller
          control={control}
          name={slugName}
          rules={{
            validate: (value) => {
              if (!active()) return true;
              const slug = String(value ?? "");
              if (!slug) return t("Slug is required.");
              if (!SLUG_PATTERN.test(slug)) return t("Use lowercase letters, numbers, and dashes only.");
              return true;
            },
          }}
          render={({ field: { value, onChange, ref } }) => (
            <Input
              id="scheduler-slug"
              name="slug"
              type="text"
              value={value}
              onChange={onChange}
              ref={ref}
              disabled={slugDisabled}
              hasError={Boolean(slugError)}
              placeholder={t("security-audit")}
              className="w-full"
            />
          )}
        />
        <p className="text-12 text-secondary">
          {t("Lowercase identifier used in URLs. Cannot be changed after creation.")}
        </p>
        {Boolean(slugError?.message) && <p className="text-12 text-danger-primary">{String(slugError?.message)}</p>}
      </div>

      <div className="flex flex-col gap-1">
        <label htmlFor="scheduler-description" className="text-13 font-medium text-primary">
          {t("Description")}
        </label>
        <Controller
          control={control}
          name={descriptionName}
          render={({ field: { value, onChange, ref } }) => (
            <TextArea
              id="scheduler-description"
              name="description"
              value={value}
              onChange={onChange}
              ref={ref}
              placeholder={t("Short summary shown in the install picker.")}
              className="min-h-[60px] w-full"
            />
          )}
        />
      </div>

      <div className="flex flex-col gap-1">
        <label htmlFor="scheduler-prompt" className="text-13 font-medium text-primary">
          {t("Prompt")}
        </label>
        <Controller
          control={control}
          name={promptName}
          rules={{
            validate: (value) => !active() || String(value ?? "").trim().length > 0 || t("Prompt is required."),
          }}
          render={({ field: { value, onChange, ref } }) => (
            <TextArea
              id="scheduler-prompt"
              name="prompt"
              value={value}
              onChange={onChange}
              ref={ref}
              hasError={Boolean(promptError)}
              placeholder={t("Look for outstanding security issues in this project…")}
              className="font-mono min-h-[180px] w-full text-13"
            />
          )}
        />
        <p className="text-12 text-secondary">
          {t(
            "The base prompt the agent runs each tick. Per-project context is appended at install time, so keep this prompt project-agnostic."
          )}
        </p>
        {Boolean(promptError?.message) && <p className="text-12 text-danger-primary">{String(promptError?.message)}</p>}
      </div>

      <div className="flex flex-col gap-2">
        <span className="text-13 font-medium text-primary">{t("Color")}</span>
        <Controller
          control={control}
          name={colorName}
          render={({ field: { value, onChange } }) => (
            <div className="flex flex-wrap items-center gap-2">
              {COLOR_PALETTE.map((color) => {
                const selected = color.toLowerCase() === String(value ?? "").toLowerCase();
                return (
                  <button
                    key={color}
                    type="button"
                    aria-label={t("Color {color}", { color })}
                    aria-pressed={selected}
                    onClick={() => onChange(color)}
                    className={`h-6 w-6 rounded-md border ${
                      selected ? "ring-offset-surface-1 ring-primary ring-2 ring-offset-1" : "border-subtle"
                    }`}
                    style={{ backgroundColor: color }}
                  />
                );
              })}
            </div>
          )}
        />
        <p className="text-12 text-secondary">{t("Used to color this scheduler's blocks on the project calendar.")}</p>
      </div>
    </>
  );
}
