## Workflow routing

Policy version: `workflow-roles-v2`.

Use this section for every Claude Code Workflow script, also when ultracode is on.
It replaces the Codex hierarchy and the legacy flow for workflow agents.
Do not route the complete request first.
Halv's router sets the model and the effort of each workflow agent.
This is an explicit instruction to set them.

1. Scout first. Then call `route_tasks` once, before you write the script.
   Load it with ToolSearch `select:mcp__halv-savings__route_tasks` when it is deferred.
   Pass your exact model id as `coordinatorModel`.
2. Send one entry for each agent stage or each distinct work item.
   Give each entry a unique `key`, a `role`, and a brief of two to five lines.
   The brief states the goal, the scope, and the completion check.
   Set `instances` when similar agents share one stage.
   Do not route a synthesis or final judge agent. It inherits your model.
3. Pass the returned `routes` object to the Workflow tool as `args.jev`.
4. For each routed agent, spread `args.jev[key].opts` into the agent() options.
   For several agents on one route, append `:` and an item name to `opts.label`.
   Do not change, add, or remove a routed model or effort.
   Options without a model and an effort inherit your model and effort.
5. When a failed check makes the script retry an agent, use `args.jev[key].retry` in the same way.
6. A workflow runs in the background. Wait for its notification. Do not poll it.
7. After the workflow ends, call `report_task_results` once for all task ids.
   `passed` is the result of the first attempt. Add `retried` for a stage that ran its retry.
8. The legacy flow, the foreground rule, the eight-worker limit, and `wait_agent` do not apply to workflow agents.
   If `route_tasks` fails, run the workflow without routed options.
