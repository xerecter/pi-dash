/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useContext } from "react";
// types
import type { TTimelineType } from "@karma-team/types";
// lib
import { StoreContext } from "@/lib/store-context";
// karma-team-web
import { getTimelineStore } from "@/karma-team-web/hooks/use-timeline-chart";
import type { IBaseTimelineStore } from "@/karma-team-web/store/timeline/base-timeline.store";
import { useTimeLineType } from "../components/gantt-chart/contexts";

export const useTimeLineChart = (timelineType: TTimelineType): IBaseTimelineStore => {
  const context = useContext(StoreContext);
  if (!context) throw new Error("useTimeLineChart must be used within StoreProvider");

  return getTimelineStore(context.timelineStore, timelineType);
};

export const useTimeLineChartStore = (): IBaseTimelineStore => {
  const context = useContext(StoreContext);
  const timelineType = useTimeLineType();

  if (!context) throw new Error("useTimeLineChartStore must be used within StoreProvider");
  if (!timelineType) throw new Error("useTimeLineChartStore must be used within TimeLineTypeContext");

  return getTimelineStore(context.timelineStore, timelineType);
};
