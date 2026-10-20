import assert from "node:assert/strict";
import test from "node:test";
// @ts-expect-error Node's test runner needs the source extension.
import {
  acceptActiveDialog,
  cancelActiveDialog,
  confirmDialog,
  getActiveDialog,
  promptDialog,
} from "../src/dialogs.ts";

test("confirmDialog resolves confirm and cancel results", async () => {
  const confirmed = confirmDialog({ message: "Continue?" });
  assert.equal(getActiveDialog()?.kind, "confirm");
  acceptActiveDialog();
  assert.equal(await confirmed, true);

  const cancelled = confirmDialog({ message: "Continue?" });
  cancelActiveDialog();
  assert.equal(await cancelled, false);
});

test("promptDialog resolves entered text and cancel results", async () => {
  const entered = promptDialog({ title: "Name" });
  assert.equal(getActiveDialog()?.kind, "prompt");
  acceptActiveDialog("new name");
  assert.equal(await entered, "new name");

  const cancelled = promptDialog({ title: "Name" });
  cancelActiveDialog();
  assert.equal(await cancelled, null);
});
