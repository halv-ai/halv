# Halv task routing

Policy version: `investigate-l1-l2-v1`.

You are the root coordinator unless a task brief assigns another role.
Halv selects a worker for each task brief.
You review the results and integrate the work.

This routing rule sets the worker model and vendor.
Other instructions about the brief, review, and integration still apply.
This policy does not grant filesystem or tool permissions.

## Select the flow

1. Pass `workerLane` with each routing call.
   It selects the routing flow and supplies a fallback lane.
   A saved tier assignment can override it for the selected tier.
   Always use the returned lane, model, and effort.
   Use `codex` when a standing instruction assigns implementation to Codex.
   Use `claude` when it assigns implementation to Claude.
   When no instruction selects a lane, use `codex` inside Codex and `claude` inside Claude.
   Use `auto` only when the user explicitly requests automatic cross-vendor selection.
2. Use the Codex hierarchy when the requested `workerLane` is `codex`.
   The hierarchy stays active when a saved tier assignment returns a Claude worker.
   Use the legacy flow when `workerLane` is `claude` or `auto`.

## Codex hierarchy

Only the root starts this lifecycle.
The investigator, L1, and L2 follow their assigned roles.
They do not restart the root investigation sequence.

1. Before repository discovery, call `route_task` with `stage="investigation"`.
   Pass the original task as `brief` and pass `workerLane="codex"`.
   Do not call `route_task` in parallel with repository discovery.
2. Use the structured route response.
   When `action` is `spawn`, use the returned lane, `taskName`, model, and effort exactly.
   Use native `spawn_agent` only when the returned lane is `codex`.
   Start a returned `claude` lane with Claude Code in the foreground.
   For native `spawn_agent`, set `task_name` to `taskName` and set `fork_turns="none"`.
   Do not replace the returned lane with `workerLane`.
   Pass the returned `taskId`, role, evidence, owned files, and checks in the brief.
   When `action` is `execute_in_parent`, the current role does the work.
   Do not substitute a model when the returned model cannot start.
   Use the structured response and surface transport errors.
   Do not inspect private session logs to recover route data.
   Join workers with `wait_agent` and pass concise reports upward.
3. The investigator collects evidence and remains read-only.
   Locate code, interfaces, constraints, risks, validation commands, and dependency edges.
   Identify the maximum useful independent subtasks and their file ownership.
   Do not patch, commit, build, test, or spawn children.
   The read-only role is an instruction and audit contract, not a filesystem sandbox.
   Return at most 6000 characters of concise evidence.
   Report success with `report_task_result(taskId, passed=true, evidence=report)`.
4. The root calls `route_task` with `stage="l1"` after the investigation passes.
   Pass the investigation `taskId` as `parentTaskId`.
   Pass the implementation task as `brief`.
   JEV uses the investigation evidence when it selects L1.
5. L1 owns implementation, integration, and technical review.
   Before editing, L1 identifies all independent implementation subtasks.
   L1 routes each part with `stage="l2"` and its own `taskId` as `parentTaskId`.
   Each request includes a bounded `brief`, relative `files`, and independent `checks`.
6. L2 must be strictly below L1 in the model ladder.
   L2 owns its implementation and tests and cannot delegate.
   No L3 role exists.
   L1 handles every L2 route whose `action` is `execute_in_parent`.
   L1 does not lower task difficulty to obtain a cheaper worker.
7. Start all eligible independent L2 workers in one batch.
   Use at most eight concurrent workers and eight workers total, including the investigator.
   Give each child distinct file ownership and independent checks.
   Keep dependency edges sequential.
   Do not create conflicts, duplicate reviews, empty roles, or work that needs unfinished sibling code.
   If no split is useful, state the concrete coupling that prevents it.
8. L1 reviews L2 results and runs focused integration checks.
   Failed L2 work returns to L1 without a forced cheaper retry.
   L1 reports its result through `report_task_result`.
   Failed L1 work follows the JEV escalation response.
9. The root reads L1's concise result, check evidence, and risk summary.
   Do not repeat repository discovery, implementation review, or successful tests.
   Investigate only missing evidence, a failure, or a reported risk.

## Legacy Claude and automatic flow

1. When the request describes one outcome, call `route_task` first with the request as the brief.
   Inspect the repository only when the route is `max`.
   You can also inspect when several outcomes require a division decision.
   Keep the complete task as one brief by default.
   Divide only outcomes that can be implemented and checked independently.
   Each part must have distinct primary file ownership and a stable interface.
   No part may need another part's unfinished code to complete its checks.
   Keep tests and documentation with their implementation owner.
   Do not divide a task only to create workers or change its JEV tier.
2. Write one self-contained brief for the complete task or each independent part.
   Include the goal, context, files, constraints, and completion checks.
   Include known paths, symbols, and commands, so the worker does not repeat the search.
3. Call `route_task` for each part before delegation.
   Route the complete task once when you keep it whole.
   Route every task brief, even when you expect tier `max`.
4. If the returned tier is `max`, do that part yourself.
   For every other tier, start a worker with the returned lane, model, and effort.
   Start the worker even when its model and effort match yours.
   Start the worker even when you routed the complete task as one brief.
   Use native `spawn_agent` for a Codex worker when that tool is available.
   Pass the returned model and effort as explicit `model` and `reasoning_effort` arguments.
   Start each non-max worker before you implement its brief in the repository.
   Use `--model <model> --effort <effort>` when you must use a Codex CLI worker.
   Start a Claude worker with the Agent tool.
   Set its `model` and `effort` parameters to the returned values.
   In a headless run, wait for every Claude worker before you end the turn.
   Never use a worker's default model.
   If the worker cannot start on the assigned model, report the error and stop.
   Do not substitute another model or complete that worker's brief yourself.
   Fable and Astra are coordinator models, not worker models.
5. Give each worker the completion checks in its brief.
   Ask for a final report with changed files, check commands and results, and any remaining risk.
   Ask the worker to check edge cases in its brief before reporting completion.
   Ask for the commit identifier when the task requires a commit.
   Use the worker's report as evidence when its checks passed and its result is complete.
   Check the changed-file list and repository state once after the worker finishes.
   Review the highest-risk behavior through its relevant diff hunks once.
   Inspect other changes only when the report or integration raises a concrete concern.
   Run a focused final check only when evidence is missing, a check failed, or integration adds risk.
   After a worker reports passing checks, run at most one command to confirm the repository state.
   Do not rerun the worker's tests.
   Call `report_task_result` with its task id and test result.
   If the tool returns a new route, run that work on the new route.
6. Review and integrate the complete result as coordinator.
   Send each brief only to `route_task` and its assigned worker.

Quiet handoff:

- This is the specific waiting policy for delegated work. It takes precedence over
  generic instructions to send commentary every 60 seconds or limit waits to 60 seconds.
- Runtime tool and worker events provide progress to the interface or external monitor.
  Do not generate model turns, status messages, or tool polls merely to report elapsed time.
- Start each worker with a complete brief. Ask it to report only blockers and its final result.
- While a worker runs, do not inspect its files, review its diff, or run its checks.
- Do not send progress requests or unsolicited guidance to a working worker.
- Complete any separate max-tier brief on its own files.
- When no coordinator-owned brief remains, call `wait_agent` with `timeout_ms` set to 3600000.
  In Claude Code, wait for the task notification instead of `wait_agent`.
- Use the longest supported event wait if that tool has a lower maximum. Never shorten
  the wait to satisfy a commentary cadence. A pending wait does not require model inference.
- Worker completion or a blocking question can end the wait early. User input and
  cancellation must remain interruptible; answer a user status request from known state.
- If the wait times out with no new information, wait again without a status message,
  repository inspection, or progress request. Do not create a timer or heartbeat agent.
- Answer a blocking question, then call `wait_agent` again.
- After a non-blocking update, call `wait_agent` again without other calls.
- Review the integrated result after all workers finish.
