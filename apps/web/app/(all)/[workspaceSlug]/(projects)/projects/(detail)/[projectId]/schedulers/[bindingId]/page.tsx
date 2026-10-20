import { observer } from "mobx-react";
import { useTranslation } from "@karma-team/i18n";
import { PageHead } from "@/components/core/page-title";
import { SchedulerBindingDetail } from "@/components/project/scheduler-bindings/binding-detail";
import { useProject } from "@/hooks/store/use-project";
import type { Route } from "./+types/page";

function ProjectSchedulerBindingDetailPage({ params }: Route.ComponentProps) {
  const { workspaceSlug, projectId, bindingId } = params;
  const { t } = useTranslation();
  const { getProjectById } = useProject();
  const project = getProjectById(projectId);
  const pageTitle = project?.name ? `${project.name} - ${t("Schedulers")}` : t("Schedulers");

  return (
    <>
      <PageHead title={pageTitle} />
      <SchedulerBindingDetail workspaceSlug={workspaceSlug} projectId={projectId} bindingId={bindingId} />
    </>
  );
}

export default observer(ProjectSchedulerBindingDetailPage);
