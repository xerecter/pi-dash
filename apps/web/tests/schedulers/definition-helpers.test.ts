import { describe, expect, it } from "vitest";
import { deriveSchedulerSlug } from "@/components/schedulers/definition-helpers";

describe("deriveSchedulerSlug", () => {
  it("normalizes names into stable lowercase slugs", () => {
    expect(deriveSchedulerSlug("Security Audit")).toBe("security-audit");
    expect(deriveSchedulerSlug("SOC2 Type 2")).toBe("soc2-type-2");
  });

  it("collapses punctuation and strips edge separators", () => {
    expect(deriveSchedulerSlug("  ...weekly review & report!  ")).toBe("weekly-review-report");
  });

  it("returns an empty value when no ASCII identifier can be derived", () => {
    expect(deriveSchedulerSlug("!!!")).toBe("");
  });
});
