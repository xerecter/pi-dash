import type { TAgentRunStatus } from "@karma-team/types";
import type { TBadgeVariant } from "@karma-team/ui";
import { Badge } from "@karma-team/ui";

const STATUS_BADGE_VARIANT: Record<TAgentRunStatus, TBadgeVariant> = {
  queued: "accent-neutral",
  assigned: "accent-primary",
  running: "primary",
  cancel_requested: "accent-warning",
  awaiting_approval: "accent-warning",
  awaiting_reauth: "accent-warning",
  paused_awaiting_input: "accent-warning",
  blocked: "accent-warning",
  completed: "accent-success",
  failed: "accent-destructive",
  cancelled: "accent-neutral",
  refused: "accent-destructive",
};

const RUN_STATUS_LABELS: Record<TAgentRunStatus, string> = {
  queued: "queued",
  assigned: "assigned",
  running: "running",
  cancel_requested: "cancellation requested",
  awaiting_approval: "awaiting approval",
  awaiting_reauth: "awaiting reauth",
  paused_awaiting_input: "paused awaiting input",
  blocked: "blocked",
  completed: "completed",
  failed: "failed",
  cancelled: "cancelled",
  refused: "refused",
};

type TranslationFn = (key: string, vars?: Record<string, unknown>) => string;

export function statusBadgeVariant(status: TAgentRunStatus): TBadgeVariant {
  return (STATUS_BADGE_VARIANT as Partial<Record<string, TBadgeVariant>>)[status] ?? "accent-neutral";
}

export function statusLabel(status: TAgentRunStatus, t: TranslationFn): string {
  return t((RUN_STATUS_LABELS as Partial<Record<string, string>>)[status] ?? status);
}

export function RunStatusBadge({ status, t }: { status: TAgentRunStatus; t: TranslationFn }) {
  return (
    <Badge variant={statusBadgeVariant(status)} size="sm">
      {statusLabel(status, t)}
    </Badge>
  );
}
