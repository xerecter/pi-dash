import { useState, type ButtonHTMLAttributes } from "react";
import { createEvent, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";

vi.mock("@karma-team/utils", () => ({ cn: (...args: unknown[]) => args.filter(Boolean).join(" ") }));
vi.mock("@karma-team/ui", () => ({
  Button: ({ loading: _loading, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { loading?: boolean }) => (
    <button {...props} />
  ),
}));
vi.mock("@/components/settings/profile/content/pages/dictation-settings", () => ({
  DICTATION_SETTINGS_ANCHOR: "dictation",
}));
vi.mock("@/hooks/use-dictation", () => ({ useDictation: () => ({ status: "idle", isSupported: false }) }));

import { ChatComposer } from "@/components/chat/composer";

function setup(props: Partial<Parameters<typeof ChatComposer>[0]> = {}) {
  const onSend = vi.fn();
  function Composer() {
    const [draft, setDraft] = useState("nihao");
    return <ChatComposer draft={draft} onDraftChange={setDraft} onSend={onSend} {...props} />;
  }
  render(
    <MemoryRouter>
      <Composer />
    </MemoryRouter>
  );
  return { input: screen.getByRole("textbox"), onSend };
}

describe("ChatComposer keyboard submission", () => {
  it("commits IME text without sending, then sends on the next Enter", () => {
    const { input, onSend } = setup();
    fireEvent.compositionStart(input);
    const enter = createEvent.keyDown(input, { key: "Enter", cancelable: true });
    fireEvent(input, enter);
    expect(enter.defaultPrevented).toBe(false);
    expect(onSend).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "\u4f60\u597d" } });
    fireEvent.compositionEnd(input);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input).toHaveValue("\u4f60\u597d");
    expect(onSend).toHaveBeenCalledTimes(1);
  });

  it("honors native composition when no composition event was received", () => {
    const { input, onSend } = setup();
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    expect(onSend).not.toHaveBeenCalled();
  });

  it("handles Safari's compositionend before the confirming keydown", () => {
    const { input, onSend } = setup();
    fireEvent.compositionStart(input);
    fireEvent.compositionEnd(input);
    fireEvent.keyDown(input, { key: "Enter", keyCode: 229, isComposing: false });
    expect(onSend).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Enter", keyCode: 13 });
    expect(onSend).toHaveBeenCalledTimes(1);
  });

  it("preserves Shift+Enter and ignores repeated Enter", () => {
    const { input, onSend } = setup();
    const newline = createEvent.keyDown(input, { key: "Enter", shiftKey: true, cancelable: true });
    fireEvent(input, newline);
    expect(newline.defaultPrevented).toBe(false);
    fireEvent.keyDown(input, { key: "Enter", repeat: true });
    expect(onSend).not.toHaveBeenCalled();
  });

  it.each([{ sending: true }, { disabledReason: "Unavailable" }, { draft: "  " }])(
    "does not submit while unavailable: %j",
    (props) => {
      const { input, onSend } = setup(props);
      fireEvent.keyDown(input, { key: "Enter" });
      expect(onSend).not.toHaveBeenCalled();
    }
  );

  it("clears interrupted composition on blur", () => {
    const { input, onSend } = setup();
    fireEvent.compositionStart(input);
    fireEvent.blur(input);
    fireEvent.focus(input);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSend).toHaveBeenCalledTimes(1);
  });
});
