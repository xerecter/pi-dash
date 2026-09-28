/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { observer } from "mobx-react";
// ui
import { EModalPosition, EModalWidth, ModalCore } from "@karma-team/ui";
// components
import { ProductUpdatesFooter } from "@/components/global";
// karma-team web components
import { ProductUpdatesChangelog } from "@/karma-team-web/components/global/product-updates/changelog";
import { ProductUpdatesHeader } from "@/karma-team-web/components/global/product-updates/header";

export type ProductUpdatesModalProps = {
  isOpen: boolean;
  handleClose: () => void;
};

export const ProductUpdatesModal = observer(function ProductUpdatesModal(props: ProductUpdatesModalProps) {
  const { isOpen, handleClose } = props;

  return (
    <ModalCore isOpen={isOpen} handleClose={handleClose} position={EModalPosition.CENTER} width={EModalWidth.XXXXL}>
      <ProductUpdatesHeader />
      <ProductUpdatesChangelog />
      <ProductUpdatesFooter />
    </ModalCore>
  );
});
