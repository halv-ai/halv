# ADR 0016: JEV routing dollars, calibrated from the bench

Status: Accepted

## Context

ADR 0015 made "dollars saved" the savings metric. It left JEV routing as "not estimated yet". ADR 0014 added the receipts that routing dollars need: the default model of each route, the outcome, and the measured tokens of each workflow agent per model.

A route sends work to a model that can use more tokens but cost less money. Halv measures the routed cost. Halv cannot measure what the default model would have spent on the same work. It must estimate that cost.

Pedro decided on 2026-10-15:

- No holdout on users. Halv does not run a share of routes on the default model to measure the difference.
- Calibrate the estimate from Halv's paired benchmark campaigns ("usa o benchmark de calibração").

The source is the internal design study of ADR 0015, sections 7 and 10 (stages 6 to 8).

## Decision

### The chain estimator

A chain is a root route and its escalations. Halv settles a chain once, when no attempt is still `routed`. The code is `src-tauri/src/savings/routing.rs`.

```text
A    = the measured cost of every attempt, failed attempts too
D(k) = k x the final attempt's tokens at the default model's prices
S(k) = D(k) - A - overhead
```

| Final state | Value |
| --- | --- |
| Passed, worker on the default model and effort | Exact: `D = A` for that worker. Earlier attempts and the overhead are losses. |
| Passed, worker on another model | `S(k)` with the `k` cell of the model pair. |
| Reported failed | `S = -A - overhead` in every estimate. |
| Closed unreported at session end | Low: as a failure. Point: none. High: as a pass at `k_high`. |
| `max` (no worker) | `S = -overhead`. |
| A worker attempt without measured agents | Not priced. The chain adds nothing and counts as "unpriced". |

- The overhead is 300 prompt tokens and 150 output tokens per routing tool call, plus the brief (4 characters per token), at the default model's prices. A workflow batch shares its two calls among its routes.
- The workflow receipt reader prices each agent message at its own prompt size. A long-context tier (Claude Haiku 5.5 above 100K prompt tokens) applies to the right requests. `task_route_agents.actual_usd` keeps the result.
- A workflow route that a session end closed has no report. Before Halv settles it, the app reads the agent transcripts of the session, so the low estimate can count their cost as a failure.
- A route without a known default model gets no uncertain part: no gain and no loss.

### The factor k

`k` is the cost of the work on the default model divided by the cost of the routed worker's tokens at the default model's prices. Both sides use the same price card, so `k` measures token use only. `k < 1` means that the default model needs fewer tokens.

`scripts/jev-calibration.py` derives `k` from paired bench runs and writes `src-tauri/src/pricing/routing_calibration.json`. The app embeds that file next to the price cards. The file holds `calibration_version`, the `price_version` that priced the bench, the method, the limits, the per-pair values, and every excluded run with its reason.

Pair selection:

- One vanilla run and one Halv run of the same task and repetition. The first valid run per task, arm and repetition. The Astra hierarchy campaign uses the published selection of 42 pairs.
- Both runs passed the verifier. A pair with different outcomes or two failures does not give `k`.
- Runs with an infrastructure error, a failed JEV runtime proof, a missing vanilla run, a missing transcript, or an escalation to `max` are excluded.

### How the routing effect is isolated

The Halv arm of the bench also has Headroom, Crux and RTK. These tools reduce the Halv tokens, which makes `k` too large. Halv removes their effect and charges the remaining doubt to the route:

1. **Toolkit factor `f`.** Halv runs where JEV chose `max` have the toolkit and no worker. `f` = Halv cost / vanilla cost on the same model, over passed pairs. Claude Opus 5.5: 20 pairs, 6 tasks, `f` = 0.972. GPT-5.6 Luna (the 20-pair report, no JEV): 7 pairs, `f` = 0.762. Halv uses the smaller of the median and the ratio of sums. Astra has no toolkit-only runs, so it takes the smallest measured factor (0.762).
2. **Two bounds per pair.** `V` is the vanilla cost, `C` the Halv coordinator cost, and `W` the routed workers at the default model's prices.
   - `k_charged = (f x V - C) / W` also charges the coordinator's own work to the route. It is a lower bound.
   - `k_brief = f x V / W` assumes that the worker did the complete task. It is an upper bound.
3. **Cell values.** `k_low` = the task-level bootstrap 10th percentile of the `k_charged` ratio of sums (one task: the 25th percentile of the pairs), at least 0. `k_point` = the `k_charged` ratio of sums. `k_high` = the `k_brief` ratio of sums.
4. **Thin cells.** A cell needs 5 pairs from 2 tasks to be "calibrated". A "thin" cell can only lower `k` below the floor, and the floor rule applies to it.

The floor stays for pairs without bench data: `k_low = 0.25`, `k_high = 1`. Under the floor rule the uncertain part counts only when it still gains at `k_low`, so a floor or thin cell shows no gain and no loss when it cannot prove one. A calibrated cell counts its values as they are, also a loss.

Cells match by model family (`claude-opus`, `claude-sonnet`, `gpt-astra`, ...) and role. The lookup order is: the exact role, any role, any routed family, the floor. A legacy route uses the role `implement`.

### Calibration 2026-10-15.1

| Default → routed | Role | Status | Pairs | Tasks | k_low | k_point | k_high | Campaigns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| gpt-astra → * (Luna and Sol) | any | calibrated | 23 | 9 | 0.193 | 0.427 | 0.573 | `rebench-astra-medium-jev-hierarchy-quiet-20260928-r3` |
| gpt-astra → gpt-sol | implement | calibrated | 9 | 4 | 0 | 0.043 | 0.290 | `deepswe-v11-jev-main-crux0102-paired-normal-idle-r15`, `-lean-r16`, `rebench-astra-medium-jev-crux-on-20260928-r1` |
| claude-opus → claude-sonnet | implement | thin | 3 | 1 | 0 | 0.019 | 0.368 | `rebench-opus55-jev-reasoning-cheap-4281` (vanilla from `rebench-opus55-high-jev-halv060-r3`) |
| claude-opus → claude-opus (lower effort) | implement | thin | 2 | 1 | 0 | 0 | 1.0 | `rebench-opus55-high-jev-halv060-r1` |
| any other pair | any | floor | 0 | 0 | 0.25 | 0.25 | 1.0 | none |

What the table means at list prices:

- Astra to Luna and Sol (hierarchy): Astra costs 6.9 times the routed tokens. At `k_low` a dollar of routed work saves about $0.32; at `k_point` about $1.93.
- Astra to Sol (legacy flow, one worker for the complete task): a loss at `k_low` and `k_point`. The bench runs cost more than vanilla.
- Opus to Sonnet: every value in the bench range is a loss, but three runs of one task are thin evidence. Halv counts no gain and no loss.
- Opus to Haiku 5.5: no bench data. The floor gives a gain, because Opus costs 20 to 40 times the routed tokens when the prompts stay under 100K tokens. Above 100K, Haiku costs 5 times more, and the gain gets small.

### Storage and reports

- Migration `0058_routing_savings` adds the `routing_savings` table: one row per settled chain with the outcome, the measured and overhead costs, the counterfactual and net values (low, point, high), `k` and its source, and the estimator, calibration and price versions.
- Only chains whose root is newer than the migration settle. Old routes have no default model and no receipts (Pedro: no restatement).
- The savings events SQL adds one branch with each priced chain at its low estimate and 0 tokens. Every dollar total (lifetime, 7 days, today, daily activity) holds the routing low estimate.
- A settled chain with a price queues one report event `routing:<root id>`: `kind = routing`, 0 tokens, `net_usd` = the low estimate, `method = chain_low`. `savings_usd` stays at least 0. The server needs no change.
- The savings reporter and the task rail settle chains. Settling is idempotent.

### Display

- A total that holds a routing estimate shows "≈": the status bar, the drawer, the session meter, and "all time".
- The Details rows split context and routing: "Routing: ≈ $0.84 (range $0.84 to $2.65)". The counted value is the low end.
- The task rail header shows "≈ $0.42 saved by routing". It shows the range and the cost of each route ("Cost $0.43 on claude-sonnet-5-5").
- The totals count the low estimate of a floor or thin chain as its point and high end, and the rows keep the stored values. The UI shows a range only when the ends differ at cents, else only "≈ $0.84". On a Fable default with no calibrated cell, the floor k_high of 1 gave $10.76 to $853.96 (2026-10-22).
- "How Halv estimates savings" names the bench calibration and the floor rule, and shows the calibration version.
- The agent-facing MCP tools give dollars: "Current Halv session savings: ≈ $0.42 at API prices (412,300 measured tokens)."

### Counted value

Pedro decided on 2026-10-18: savings never go below zero. A negative "saved" figure is wrong for the user.

- Halv keeps the signed estimate of each chain in `routing_savings`. The details and the report event's `net_usd` show it.
- Every counted total floors each chain at zero: the dollar totals, the routing totals, the pending dollars, the task rail header, and the event's `savings_usd`.
- A failed chain, an escalation, or a `max` route adds zero to the headline. It does not take the gain of another chain away.

## Consequences

- Routing dollars appear only for routes after the upgrade that have measured receipts. Workflow routes have them; legacy `route_task` routes do not, so their worker routes stay unpriced and only `max` routes count (as overhead).
- The bench says that most current routes save less than Pedro's premise suggests. Opus to Sonnet gets no credit, the legacy Astra to Sol flow shows a loss, and only Haiku routes and the Astra hierarchy gain. The figure is honest, and it can be small or negative.
- A failed or unreported attempt always reduces the total.
- A new calibration needs a new file and an app release. Settled chains keep the calibration version that settled them.

## Limits

- The bench mixes all Halv features. `f` comes from other runs and other models, so the toolkit correction is approximate. Halv uses the smallest measured `f`, which lowers `k`.
- No bench run has Workflow agents. The Claude bench routes the complete task to one worker.
- The Claude routed pairs have one task and Claude Sonnet 5, not Sonnet 5.5. Cells match by family.
- The Astra hierarchy runs have Luna and Sol workers in one run, so their `k` is one value for both.
- The bench prices sessions at base rates. The bench data has no per-request prompt sizes.
- The overhead constant is not measured from the coordinator transcript.

## Alternatives

- A user holdout (study section 7.3): rejected by Pedro.
- `k = 1` (the price difference on the routed tokens): rejected. It is an upper bound when the default model needs fewer tokens.
- The whole-stack token ratio as `k`: rejected. It includes the toolkit and the coordinator.
- The 25th percentile of the pairs as `k_low` for every cell: rejected. The headline adds many routes, so the low estimate needs a bound of the mean. The bootstrap gives it. The data file keeps the percentile.
