/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useCallback } from "react";
// karma-team editor
import type { TMentionSection, TMentionSuggestion } from "@karma-team/editor";
// karma-team types
import type {
  TSearchEntities,
  TSearchEntityRequestPayload,
  TSearchResponse,
  TUserSearchResponse,
} from "@karma-team/types";
// karma-team ui
import { Avatar } from "@karma-team/ui";
// helpers
import { getFileURL } from "@karma-team/utils";
// karma-team web hooks
import { useAdditionalEditorMention } from "@/karma-team-web/hooks/use-additional-editor-mention";

type TArgs = {
  enableAdvancedMentions?: boolean;
  searchEntity: (payload: TSearchEntityRequestPayload) => Promise<TSearchResponse>;
};

export const useEditorMention = (args: TArgs) => {
  const { enableAdvancedMentions = false, searchEntity } = args;
  // additional mentions
  const { editorMentionTypes, updateAdditionalSections } = useAdditionalEditorMention({
    enableAdvancedMentions,
  });
  // fetch mentions handler
  const fetchMentions = useCallback(
    async (query: string): Promise<TMentionSection[]> => {
      try {
        const res = await searchEntity({
          count: 5,
          query_type: editorMentionTypes,
          query,
        });
        const suggestionSections: TMentionSection[] = [];
        if (!res) {
          throw new Error("No response found");
        }
        Object.keys(res).map((key) => {
          const responseKey = key as TSearchEntities;
          const response = res[responseKey];
          if (responseKey === "user_mention" && response && response.length > 0) {
            const items: TMentionSuggestion[] = (response as TUserSearchResponse[]).map((user) => ({
              icon: (
                <Avatar
                  className="flex-shrink-0"
                  src={getFileURL(user.member__avatar_url)}
                  name={user.member__display_name}
                />
              ),
              id: user.member__id,
              entity_identifier: user.member__id,
              entity_name: "user_mention",
              title: user.member__display_name,
            }));
            suggestionSections.push({
              key: "users",
              title: "Users",
              items,
            });
          }
        });
        const { sections } = updateAdditionalSections({
          response: res,
        });
        return [...suggestionSections, ...sections];
      } catch (error) {
        console.error("Error in fetching mentions:", error);
        throw error;
      }
    },
    [editorMentionTypes, searchEntity, updateAdditionalSections]
  );

  return {
    fetchMentions,
  };
};
