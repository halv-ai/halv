import assert from "node:assert/strict";
import test from "node:test";

import { appendDictation, dictationTranscript } from "../src/dictation.ts";

test("assembles final and interim dictation without dropping prior prompt text", () => {
  const results = [
    { isFinal: true, 0: { transcript: "fix the tests" } },
    { isFinal: false, 0: { transcript: " in auth" } },
  ];
  const transcript = dictationTranscript(results);
  assert.deepEqual(transcript, {
    finalText: "fix the tests",
    interimText: " in auth",
  });
  assert.equal(
    appendDictation("Please", transcript.finalText, transcript.interimText),
    "Please fix the tests in auth",
  );
});

test("does not add a second separator after trailing whitespace", () => {
  assert.equal(appendDictation("Please ", "continue", ""), "Please continue");
});
