# ADR 0014: JEV routing for Claude Code Workflow agents

Status: Accepted

## Context

Claude Code has a Workflow tool. A Workflow script starts many agents with `agent(prompt, opts)`.
Each agent has a fresh context. `opts.model` and `opts.effort` set its model and effort.
With ultracode on, Claude Code uses a workflow for every substantive task.

Before this ADR, JEV had no effect on workflow agents:

- The Workflow reference tells the model to omit `opts.model`. So every agent inherits the coordinator model, which is mostly Opus at `xhigh` or `max`.
- JEV routed one brief per `route_task` call. Nothing connected a route to an `agent()` call.
- The legacy flow routes the complete request first and starts one worker. That conflicts with a workflow.
- JEV marks 94% of briefs as "needs reasoning". The legacy ladder maps that to Opus.
- Halv could not see the tokens of workflow agents, so a route had no receipt.

A replay of 100 real workflow briefs through JEV (phase 0) showed that narrow briefs score like whole tasks.
Only the role of an agent can move it off Opus with the current JEV model.

## Decision

### Scope

Workflow routing applies only to agent panes that Halv starts.
Halv adds it through its `halv-savings` MCP server and its coordinator instructions.
Halv changes no user or global Claude Code configuration.

Two preferences control it:

- `workflowRouting` (default on): the coordinator instructions get the "Workflow routing" section, and the MCP server lists `route_tasks` and `report_task_results`. Off sends `--workflow-routing off` to the server.
- `claudeUltracode` (default off): new Claude panes start with `"ultracode": true` in their per-session `--settings` file. Users can also turn ultracode on in a pane with `/effort ultracode on`.

Both need task routing on. A Codex pane gets no workflow routing.

### `route_tasks`

The coordinator scouts, then calls `route_tasks` once, before it writes the script.

- Input: `workflow` (a name), `coordinatorModel` (its exact model id), and 1 to 64 `tasks`. A task has a unique `key`, a `role` (`read`, `implement`, or `verify`), a brief of at most 4,000 characters, and `instances` (1 to 200).
- Halv asks JEV for up to eight briefs at the same time and writes all routes of the batch in one transaction.
- Output for each key: `taskId`, `opts`, and `retry`. `opts` holds `model`, `effort`, and the label `jev:<first 8 characters of taskId>:<key>`. `retry` holds the options of one retry, with the label suffix `:retry`, or `null`.
- The script spreads `args.jev[key].opts` into the agent options. For several agents on one route, it appends `:` and an item name to the label.
- Options without a model and an effort inherit the coordinator.
- When JEV cannot answer a brief, that stage inherits the coordinator (reason `fallback`). When the user is signed out, all stages inherit. The workflow never waits for JEV.
- A batch does not touch the Codex hierarchy and does not wait for an active hierarchy root.

### Policy `workflow-roles-v2`

| Role | Band | `needs_reasoning >= 0.5` | `risky >= 0.5` |
| --- | --- | --- | --- |
| `read` | cheapest to cheap | one effort step up, and the band cap | no effect |
| `implement` | cheap to strong | one effort step up, at most `xhigh` | raise to `strong` |
| `verify` | medium to strong | one effort step up, at most `xhigh` | raise to `strong` |

- Low confidence raises the tier one step.
- The coordinator model caps every band. A Sonnet coordinator never gets an Opus agent.
- No route collapses into the coordinator.
- Routes use the Claude lane only. Every route below `max` has an explicit effort. The Haiku tier gets the CLI default effort, else `low`.
- A retry goes one tier up inside the band, then one effort step up at the band cap, then to the coordinator model.
- A synthesis or final judge agent is not routed.

### The default of a route

Every route, legacy or workflow, records `default_model`, `default_effort`, and `default_source`.
The default is what the worker would run without JEV: the coordinator model and effort.
The source order is:

1. `observed`: the last assistant message in the coordinator transcript (`message.model` and `effort`).
2. `coordinator`: the `coordinatorModel` argument.
3. `hint`: the launch `--coordinator-model`. A Claude pane passes only a Claude model as the hint.
4. `none`.

### `report_task_results` and receipts

After the workflow, the coordinator calls `report_task_results` once.

- Each result has `taskId` and `passed` (the first attempt), and `retried` when the stage ran its retry.
- Halv reads `<projects>/*/<session>/subagents/workflows/*/agent-*.meta.json` and `agent-*.jsonl` of the Claude session. It matches the labels to the routes and keeps the last usage of each message id.
- Halv stores one receipt per route in `task_route_receipts`: the outcome, the agent counts, the tokens, the duration, and the usage source (`measured`, `reported`, or `none`).
- Halv stores one row per agent and model in `task_route_agents`: the observed model and effort, and the input, output, cache read, and cache write tokens (5-minute and 1-hour). This is the one cost table of the dollar-savings design.
- A stage that failed and ran its retry gets the outcome `escalated`. The retry becomes a new route with `escalated_from_id`, and it gets its own receipt.
- A repeated identical report changes nothing. A conflicting report fails and writes nothing.
- A session end marks an unreported route `failed` with `closed_unreported = 1`.

Migration `0056_task_route_receipts` adds the columns and the two tables. It only adds columns, because older MCP servers still read `task_routes` (ADR 0012).

### Launch

- A Claude pane joins the Ponytail and the routing text into one `--append-system-prompt`. Claude Code keeps only the last value of that flag.
- With routing on and the `never` approval mode (`dontAsk`), a Claude pane gets `--allowedTools mcp__halv-savings`.
- The `halv-savings` MCP server gets a 120-second tool-call timeout.
- The MCP server instructions are shorter and tell workers not to route.

## Consequences

- JEV can route workflow agents in Halv panes. Ultracode sessions in other terminals get nothing.
- With JEV 1.13, mostly read agents move to Sonnet. The phase-0 replay estimates about 10% of workflow-agent cost.
- Compliance depends on the model. The Workflow reference says to omit `opts.model`. The receipts show the observed model of each agent, so the task rail can show when a route was not applied.
- Measured receipts need the agent labels. An agent without a `jev:` label has no receipt.
- Halv reads the Claude session id from `agent_sessions.cli_session_id`. When Claude Code changes the session id (for example after `/clear`), the receipts of later workflows have no measured usage.
- More briefs leave the computer: one per stage. The existing task routing opt-in applies.
