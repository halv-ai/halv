import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { setIpcTransport } from "../src/ipc.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  acceptActiveDialog,
  cancelActiveDialog,
  getActiveDialog,
} from "../src/dialogs.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { offerCruxIndexApproval } from "../src/cruxIndexApproval.ts";

const t = (source: string) => source;

function recordApprovals() {
  const approvals: unknown[] = [];
  setIpcTransport({
    invoke: (async (name: string, args?: unknown) => {
      if (name === "crux_approve_indexing") {
        approvals.push(args);
      }
      return null;
    }) as never,
    listen: async () => () => undefined,
  } as never);
  return approvals;
}

async function nextTick() {
  await new Promise((resolve) => setImmediate(resolve));
}

test("an approved project gets no indexing prompt", async () => {
  const approvals = recordApprovals();
  const target = { projectId: 1, workspaceId: "main" };

  assert.equal(
    await offerCruxIndexApproval(t, target, { approvalRequired: false }),
    false,
  );
  assert.equal(await offerCruxIndexApproval(t, target, null), false);
  assert.equal(getActiveDialog(), null);
  assert.deepEqual(approvals, []);
});

test("approval names the build tools and then starts indexing", async () => {
  const approvals = recordApprovals();
  const target = { projectId: 2, workspaceId: "main" };

  const offered = offerCruxIndexApproval(t, target, { approvalRequired: true });
  const dialog = getActiveDialog();
  assert.equal(dialog?.kind, "confirm");
  assert.match(dialog?.message ?? "", /runs the build tools of this project/);
  assert.deepEqual(approvals, []);
  acceptActiveDialog();

  assert.equal(await offered, true);
  assert.deepEqual(approvals, [target]);
});

test("a declined prompt does not approve or come back in the session", async () => {
  const approvals = recordApprovals();
  const target = { projectId: 3, workspaceId: "feature" };

  const offered = offerCruxIndexApproval(t, target, { approvalRequired: true });
  cancelActiveDialog();
  assert.equal(await offered, false);

  const again = offerCruxIndexApproval(t, target, { approvalRequired: true });
  await nextTick();
  assert.equal(getActiveDialog(), null);
  assert.equal(await again, false);
  assert.deepEqual(approvals, []);
});
