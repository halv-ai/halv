import assert from "node:assert/strict";
import test from "node:test";

import {
  acceptedNativeDropPaths,
  droppedMentionPath,
  insertDroppedFileMentions,
  isDroppedImagePath,
  partitionDroppedPaths,
} from "../src/composerDrop.ts";
import {
  MAX_IMAGE_ATTACHMENTS_PER_MESSAGE,
  mergeImageAttachments,
  type ImageAttachmentData,
} from "../src/attachments.ts";
import type { ImageAttachment } from "../src/types.ts";

const droppedImage: ImageAttachmentData = {
  name: "dropped.png",
  mediaType: "image/png",
  sizeBytes: 8,
  data: "iVBORw0KGgo=",
};

function existingImage(index: number): ImageAttachment {
  return {
    id: `existing-${index}`,
    name: `existing-${index}.png`,
    mediaType: "image/png",
    sizeBytes: 8,
    data: "iVBORw0KGgo=",
  };
}

test("routes only supported dropped image extensions to the image reader", () => {
  assert.equal(isDroppedImagePath("/tmp/screen.PNG"), true);
  assert.equal(isDroppedImagePath("C:\\work\\photo.jpeg"), true);
  assert.equal(isDroppedImagePath("/tmp/preview.webp"), true);
  assert.equal(isDroppedImagePath("/tmp/animation.gif"), false);
  assert.equal(isDroppedImagePath("/tmp/image.png.txt"), false);
});

test("partitions mixed native drops into images and file mentions", () => {
  assert.deepEqual(
    partitionDroppedPaths([
      "/tmp/screen.PNG",
      "/tmp/notes.txt",
      "C:\\work\\photo.jpeg",
    ]),
    {
      imagePaths: ["/tmp/screen.PNG", "C:\\work\\photo.jpeg"],
      mentionPaths: ["/tmp/notes.txt"],
    },
  );
});

test("accepts images anywhere and other paths only inside the composer", () => {
  const paths = ["/tmp/screen.png", "/tmp/notes.txt"];

  assert.deepEqual(acceptedNativeDropPaths(paths, false), [
    "/tmp/screen.png",
  ]);
  assert.deepEqual(acceptedNativeDropPaths(paths, true), paths);
});

test("formats project paths like project file-search mentions", () => {
  assert.equal(
    droppedMentionPath("/work/halv/src/main.ts", "/work/halv"),
    "src/main.ts",
  );
  assert.equal(
    droppedMentionPath("/work/halves/readme.md", "/work/halv"),
    "/work/halves/readme.md",
  );
  assert.equal(
    droppedMentionPath("C:\\Work\\Halv\\src\\main.ts", "c:\\work\\halv\\"),
    "src/main.ts",
  );
});

test("inserts deduplicated mention tokens at the cursor", () => {
  const insertion = insertDroppedFileMentions(
    "Review please",
    6,
    ["/work/halv/src/main.ts", "/tmp/outside.log", "/tmp/outside.log"],
    "/work/halv",
  );

  assert.equal(
    insertion.draft,
    "Review @{src/main.ts} @{/tmp/outside.log}  please",
  );
  assert.equal(insertion.cursor, 42);
  assert.deepEqual(insertion.mentions, [
    { token: "@{src/main.ts}", path: "src/main.ts" },
    { token: "@{/tmp/outside.log}", path: "/tmp/outside.log" },
  ]);
});

test("merges dropped images onto the current attachment list", () => {
  const current = [existingImage(1)];
  const result = mergeImageAttachments(current, [droppedImage]);

  assert.deepEqual(
    result.attachments.map((attachment) => attachment.name),
    ["existing-1.png", "dropped.png"],
  );
  assert.equal(result.firstError, null);
});

test("keeps the current list and reports a merge cap rejection", () => {
  const current = Array.from(
    { length: MAX_IMAGE_ATTACHMENTS_PER_MESSAGE },
    (_, index) => existingImage(index),
  );
  const result = mergeImageAttachments(current, [droppedImage]);

  assert.equal(result.attachments.length, MAX_IMAGE_ATTACHMENTS_PER_MESSAGE);
  assert.match(result.firstError ?? "", /Attach at most 8 images/);
});
