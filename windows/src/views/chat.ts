// Chat view — prompt, chat bubbles and typing dots.

import { h, svg, clear } from "./dom";
import { ICONS } from "./icons";
import { Bridge, type ChatContext } from "../core/bridge";
import { Sound } from "../core/sound";
import { State, type AgentStep, type ChatMessage } from "../core/state";
import type { ViewHost } from "./views";

let nextId = 1;

function bubble(message: ChatMessage): HTMLElement {
  if (message.role === "user") {
    return h(
      "div",
      { class: "chat-row user" },
      h("div", { class: "bubble", text: message.content }),
    );
  }
  return h("div", { class: "chat-row" }, h("div", { class: "reply", text: message.content }));
}

function typingDots(): HTMLElement {
  return h(
    "div",
    { class: "chat-row" },
    h("div", { class: "typing" }, h("i"), h("i"), h("i")),
  );
}

const STEP_LABEL_MAX = 32;

function stepLabel(step: AgentStep): string {
  const label =
    step.name === "read_file"
      ? `Reading ${step.summary}`
      : step.name === "list_dir"
        ? `Looking in ${step.summary}`
        : step.name === "web_fetch"
          ? `Opening ${step.summary}`
          : step.name === "web_search"
            ? `Searching “${step.summary}”`
            : step.name;
  return label.length > STEP_LABEL_MAX ? `${label.slice(0, STEP_LABEL_MAX - 1)}…` : label;
}

/** A tool call of the turn in flight — the prototype's plain grey chip. */
function toolChip(step: AgentStep): HTMLElement {
  return h(
    "div",
    { class: `chip tool ${step.status}`, title: step.summary },
    h("i", { class: "chip-dot" }),
    h("span", { text: stepLabel(step) }),
  );
}

/** The coloured chip showing what the question is about (a dropped file). */
function contextChip(label: string): HTMLElement {
  const chip = h("div", { class: "chip" }, h("i", { class: "chip-dot" }), h("span", { text: label }));
  requestAnimationFrame(() => chip.classList.add("settled"));
  return chip;
}

export function buildPrompt(onHeightChange: () => void): ViewHost {
  const chipRow = h("div", { class: "chip-row" });
  const log = h("div", { class: "chat-log" });
  const input = h("input", {
    type: "text",
    class: "chat-input",
    placeholder: "Ask me anything…",
    spellcheck: "false",
  }) as HTMLInputElement;
  const send = h("button", { class: "send-btn", title: "Send" }, svg(ICONS.arrowUp, 11));
  const bar = h("div", { class: "chat-bar" }, input, send);

  const el = h(
    "div",
    { class: "view" },
    h("div", { class: "card wash chat-card" }, h("div", { class: "chat-body" }, chipRow, log, bar)),
  );
  (el.querySelector(".card") as HTMLElement).style.setProperty("--wash", "rgba(99,102,241,0.5)");

  let sending = false;
  let renderedKey = "";
  let buttonStop: boolean | null = null;

  async function submit() {
    if (sending) {
      // The send button is Stop while a turn runs.
      void Bridge.agentCancel();
      return;
    }
    const query = input.value.trim();
    if (!query) return;
    input.value = "";
    sending = true;
    Sound.play("send");

    const asked: ChatMessage = { id: nextId++, role: "user", content: query };
    State.chatHistory.push(asked);
    State.agentBusy = true;
    State.agentSteps = [];
    State.agentInterim = null;
    State.agentTurnId = null;
    State.stateOverride = "thinking";
    State.notify();
    onHeightChange();

    const file = State.droppedFile;
    const context: ChatContext | null =
      State.chatHistory.length === 1 && file ? { kind: "file", name: file.name, path: file.path } : null;

    try {
      const reply = await Bridge.agentSend(query, context);
      if (reply.cancelled) {
        // Stopped: the turn was rolled back, so the question goes back to the field.
        if (State.chatHistory[State.chatHistory.length - 1]?.id === asked.id) State.chatHistory.pop();
        if (!input.value) input.value = query;
      } else {
        State.chatHistory.push({ id: nextId++, role: "assistant", content: reply.text });
        Sound.play("finish");
      }
    } catch (err) {
      State.noteMessage = String(err).replace(/^Error:\s*/, "");
      State.view = "note";
      Sound.play("error");
    } finally {
      sending = false;
      State.agentBusy = false;
      State.agentSteps = [];
      State.agentInterim = null;
      State.agentTurnId = null;
      State.stateOverride = null;
      State.notify();
      onHeightChange();
      input.focus();
    }
  }

  send.addEventListener("click", () => void submit());
  input.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Enter") {
      e.preventDefault();
      void submit();
    }
    e.stopPropagation(); // Escape closes the island, not the chat
  });

  return {
    el,
    sync() {
      const file = State.droppedFile;
      const wantChip = file?.name ?? "";
      if (chipRow.dataset.label !== wantChip) {
        chipRow.dataset.label = wantChip;
        clear(chipRow);
        if (wantChip) chipRow.append(contextChip(wantChip));
      }

      const busy = State.agentBusy;
      const steps = State.agentSteps;
      const interim = State.agentInterim;
      const key = `${State.chatHistory.length}|${busy}|${steps.map((s) => s.id + s.status).join(",")}|${interim ?? ""}`;
      if (key !== renderedKey) {
        renderedKey = key;
        clear(log);
        for (const m of State.chatHistory) log.append(bubble(m));
        if (busy) {
          if (interim) log.append(h("div", { class: "chat-row" }, h("div", { class: "reply interim", text: interim })));
          if (steps.length) log.append(h("div", { class: "chat-row tools" }, ...steps.map(toolChip)));
          log.append(typingDots());
        }
        log.scrollTop = log.scrollHeight;
      }

      // Swap the icon only when the mode changes, so the button isn't rebuilt
      // under the pointer.
      if (buttonStop !== sending) {
        buttonStop = sending;
        clear(send);
        send.append(svg(sending ? ICONS.xmark : ICONS.arrowUp, 11));
        send.title = sending ? "Stop" : "Send";
      }

      input.placeholder = State.chatHistory.length === 0 ? "Ask me anything…" : "Continue…";
      input.disabled = sending;
    },
    focus() {
      input.focus();
      input.select();
    },
  };
}
