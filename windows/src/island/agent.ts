// Chat turn progress ("agent-event") → tool chips and Mochi's mood.
// The final text arrives as the agent_send result; these events only show
// what happens meanwhile, and late ones are dropped by turn id.

import { onEvent, type AgentEvent } from "../core/bridge";
import { State } from "../core/state";

function isSearch(name: string): boolean {
  return name === "web_fetch" || name === "web_search";
}

export function registerAgentHandlers() {
  void onEvent<AgentEvent>("agent-event", (e) => {
    if (!State.agentBusy) return;
    if (e.type === "turn_start") {
      State.agentTurnId = e.turnId;
    } else if (State.agentTurnId !== e.turnId) {
      return;
    }

    switch (e.type) {
      case "tool_start":
        State.agentSteps.push({ id: e.id, name: e.name, summary: e.summary, status: "running" });
        State.stateOverride = isSearch(e.name) ? "searching" : "working";
        break;
      case "tool_end": {
        const step = State.agentSteps.find((s) => s.id === e.id);
        if (step) step.status = e.ok ? "ok" : "error";
        State.stateOverride = "thinking";
        break;
      }
      case "text":
        State.agentInterim = e.text;
        break;
      case "retry":
        State.stateOverride = "ratelimit";
        break;
      default:
        // turn_start, done, cancelled, error: the agent_send promise handles them.
        break;
    }
    State.notify();
  });
}
