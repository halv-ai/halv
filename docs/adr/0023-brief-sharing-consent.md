# ADR 0023: Task brief consent

Date: 2026-10-24
Status: Accepted.

This ADR adds a consent rule to the task routing of ADR 0014 and ADR 0016. It also stops the Cloud handoff of Automations until a worker exists.

## Context

JEV task routing (ADR 0016) and workflow routing (ADR 0014) send a task brief to halv.ai for each routed task:

- The coordinator agent writes the brief. A brief can contain code, file paths, and secrets from the task.
- halv.ai sends the brief to OpenRouter. There the TypeSafe JEV model picks a model tier.
- Halv does not store or log the brief.

Before this ADR, the routing switch was on by default. An account with Halv Cloud sent briefs, and nobody asked the user. The JEV page in Settings said only "Briefs leave this computer."

ADR 0021 made usage events opt-in. A task brief can carry much more than a usage event, so it needs at least the same rule.

Three more facts made the gap larger:

- The server reads the brief before it refuses an account without Halv Cloud with HTTP 403. A refused account still sent its brief.
- The `halv-savings` MCP server lists the routing tools in every session, also with routing off. An agent can call `route_task` without the coordinator instructions.
- Cloud handoff in Automations posts the prompt (up to 64 KiB), the repository URL, the base ref, the model, and the effort to `/v1/cloud/tasks`. No worker in any repository runs these tasks. The data leaves the device, and nothing runs.

## Decision

1. **One answer with three values.** `AppPreferences.briefSharing` has the values `unset`, `allowed`, and `declined`. A new install reads `unset`. A preferences file without the key reads `unset`. A damaged value also reads `unset`. The command `preferences_set_brief_sharing` accepts only `allowed` and `declined`.
2. **Routing needs the switch and a yes.** A launch routes tasks only when `taskRouting` is on and `briefSharing` is `allowed`. Workflow routing needs both values too. Until then, a launch with the switch on starts without routing:
   - The agent gets no coordinator instructions.
   - The `halv-savings` server starts with `--task-routing off`. It lists only `get_session_savings` and `get_savings_totals`. These two tools read local data and send nothing.
   - The server refuses `route_task`, `report_task_result`, `route_tasks`, and `report_task_results`.
3. **The prompt asks before a launch that the user started.** The IPC layer runs a gate before each agent launch command: `agent_terminal_create`, `agent_terminal_duplicate`, and `agent_ghostty_launch`. A launch counts as started by the user when a click or a key press came in the last 10 seconds. The prompt holds the launch until the answer saves. The prompt states these facts:
   - The agent writes a task brief. A brief can contain code, file paths, and secrets from the task.
   - Halv sends the brief to halv.ai. halv.ai sends it to OpenRouter, where the TypeSafe JEV model picks a model tier.
   - Halv does not store or log the brief.

   A "Learn more" link opens `https://github.com/halv-ai/halv#task-briefs`. The buttons are "Allow" and "Don't send". Escape closes the prompt without an answer. The launch then starts without routing, and the app does not ask again in that run.
4. **The card asks after a launch without a user action.** A session restore at startup or a local automation starts at once, without routing. The app then shows a card with the same facts and the same buttons. The card never blocks. Its close button works like Escape on the prompt. A launch also gets the card when no view can show the prompt, for example while the account page is open. A held launch starts when the prompt view unmounts.

   Rust can also start a pane, for example from the phone remote. Such a launch has no IPC gate. When the routing switch is on and the answer is `unset`, the launcher emits the `brief-sharing-needed` event for a Claude or Codex pane. The main window then shows the card, with the same access rule.
5. **Only Halv Cloud accounts get the question.** The app never asks a signed-out install or an account without Halv Cloud. A launch before the account status loads starts without routing. The card appears when the status shows Halv Cloud access.
6. **Settings changes the answer.** Settings, JEV shows "Task briefs" with the facts, the link, and two buttons: "Allowed" and "Declined". An `unset` answer shows "Not decided yet". This section replaces the sentence "Briefs leave this computer."
7. **The MCP server checks before each brief.** Before any network call, the `halv-savings` server reads local data in this order:
   - No valid session: the route runs in the parent with the reason `sign_in_required`.
   - Cached `/v1/me` entitlements without Halv Cloud: the reason is `halv_cloud_required`.
   - A `briefSharing` value other than `allowed` in `preferences.json`: the reason is `brief_sharing_off`.
   - Unknown entitlements: the server sends the brief, and halv.ai decides.

   The server reads the preferences file before each brief. Thus a "no" in Settings stops the next brief of a running session. A missing or damaged file reads `unset`. The route stays on the device, so the coordinator can report it. The cached access follows `cloudAccess` in `src/halvCloud.ts`.
8. **Cloud handoff is off.** `CLOUD_HANDOFF_ENABLED` is `false` in `src-tauri/src/automations.rs` and in `src/components/AutomationsPanel.tsx`:
   - The panel does not offer "Cloud handoff" for a new automation.
   - `enqueue_cloud_task` returns "Cloud handoff not available" before any network call.
   - The scheduler skips a cloud automation and records no run. "Run now" refuses it.
   - The panel marks a cloud automation "paused" and shows the reason.
   - An edit keeps the cloud target of an existing automation. No automation can move to the cloud target.

   The handoff code stays for a later worker.

## Consequences

- No task brief leaves the device before the user says yes once. The launch, the MCP server, and the migration of old preferences files all keep this rule.
- An upgraded install with routing on stops routing until the user answers. The first launch that the user starts asks the question.
- The prompt is a modal. Decision 5 of ADR 0021 forbids a modal for the Halv Cloud offer. The prompt is a consent question, not an offer. It holds one launch only, only for an account with Halv Cloud. "Don't send" and Escape both start the launch at once.
- The 10-second window is a heuristic. A restore within 10 seconds of a click gets the prompt instead of the card. A launch after a slow lane refresh can get the card instead of the prompt. Both cases ask and send nothing before the answer.
- A "yes" applies to new sessions. A session that started without routing stays without routing. A "no" stops the next brief of every running session.
- A "no" fails closed. The app keeps "no" in memory before the write, so a new launch does not route. When Halv cannot write `preferences.json`, it writes the empty file `brief-sharing-declined` in the same folder. The MCP server and the next app start read that file as "no". Only when both writes fail does the user see an error. Then a running session can send briefs until a write succeeds. A later "yes" removes the file.
- The Tauri command `route_task` in `src-tauri/src/auth/mod.rs` posts a brief to `/v1/route` without this check. No view calls it. A follow-up adds the check or removes the command.
- Stale cached entitlements can refuse a brief that the server would accept. The app refreshes the entitlements, and the next brief uses the new cache.
- The task board does not know `brief_sharing_off` yet. It shows such a route as a row with the raw reason. A follow-up adds the reason to `src/taskBoard.ts`.
- Cloud automations from older versions stay in `automations.json`. They do not run until a worker exists and the flag turns on.
