// Integration events → island state: each poller update refreshes the data
// behind its card.

import { onEvent, Bridge, type IntegrationUpdate } from "../core/bridge";
import { State } from "../core/state";

/** Which Credential Manager key backs each pill. */
const KEY_FOR: Record<string, string> = {
  integration_github: "github-token",
  integration_notion: "notion-api-key",
  integration_calcom: "calcom-api-key",
};

export function registerIntegrationHandlers() {
  void onEvent<IntegrationUpdate>("integration", handle);
  void refreshConfigured();
}

/** Asks Rust which keys exist so the idle cards can say so. */
export async function refreshConfigured() {
  for (const [id, key] of Object.entries(KEY_FOR)) {
    const present = (await Bridge.secretPresent(key)) ?? false;
    const info = State.integrations[id] ?? { data: {}, error: null, loaded: false, configured: false };
    State.integrations[id] = { ...info, configured: present };
  }
  const hooks = State.settings.hooksInstalled;
  const claude = State.integrations.integration_claude ?? {
    data: {}, error: null, loaded: false, configured: false,
  };
  State.integrations.integration_claude = { ...claude, configured: hooks };
  State.notify();
}

function handle(update: IntegrationUpdate) {
  if (State.paused) return;

  const previous = State.integrations[update.id];
  State.integrations[update.id] = {
    data: update.error ? (previous?.data ?? {}) : update.data,
    error: update.error,
    loaded: update.error ? (previous?.loaded ?? false) : true,
    configured: previous?.configured ?? true,
  };
  State.notify();
}
