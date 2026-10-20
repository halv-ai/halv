import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { clearHalvLocalStorage } from "../src/freshState.ts";

function memoryStorage(entries: Array<[string, string]>) {
  const values = new Map(entries);
  return {
    get length() {
      return values.size;
    },
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    key(index: number) {
      return [...values.keys()][index] ?? null;
    },
    removeItem(key: string) {
      values.delete(key);
    },
  };
}

test("fresh state clears Halv storage keys only", () => {
  const storage = memoryStorage([
    ["halv.guidedFirstTask", "completed"],
    ["halv.theme", "dark"],
    ["other.preference", "keep"],
  ]);

  clearHalvLocalStorage(storage);

  assert.equal(storage.getItem("halv.guidedFirstTask"), null);
  assert.equal(storage.getItem("halv.theme"), null);
  assert.equal(storage.getItem("other.preference"), "keep");
});

test("main imports the fresh-state boot module first", () => {
  const main = readFileSync(new URL("../src/main.tsx", import.meta.url), "utf8");
  const firstImport = main.match(/^import[^;]+;/m)?.[0];

  assert.equal(firstImport, 'import "./freshStateBoot";');
});
