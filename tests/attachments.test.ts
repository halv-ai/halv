import assert from "node:assert/strict";
import test from "node:test";

import {
  appendImageAttachments,
  appendImageFiles,
  imageAttachmentDataUrl,
  MAX_IMAGE_ATTACHMENT_BYTES,
  MAX_IMAGE_ATTACHMENTS_PER_MESSAGE,
  MAX_IMAGE_ATTACHMENTS_TOTAL_BYTES,
} from "../src/attachments.ts";
import type { ImageAttachment } from "../src/types.ts";

const PNG_SIGNATURE = Uint8Array.from([
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
]);

function fakeFile(name: string, type: string, bytes: Uint8Array): File {
  return {
    name,
    type,
    size: bytes.byteLength,
    arrayBuffer: async () => bytes.slice().buffer,
  } as File;
}

function existingAttachment(sizeBytes = 8): ImageAttachment {
  return {
    id: crypto.randomUUID(),
    name: "prior.png",
    mediaType: "image/png",
    sizeBytes,
    data: "iVBORw0KGgo=",
  };
}

test("reads image bytes into separate base64 attachment metadata", async () => {
  const [attachment] = await appendImageFiles(
    [],
    [fakeFile("screenshot.png", "image/png", PNG_SIGNATURE)],
  );

  assert.equal(attachment.name, "screenshot.png");
  assert.equal(attachment.mediaType, "image/png");
  assert.equal(attachment.sizeBytes, PNG_SIGNATURE.byteLength);
  assert.equal(attachment.data, "iVBORw0KGgo=");
  assert.equal(
    imageAttachmentDataUrl(attachment),
    "data:image/png;base64,iVBORw0KGgo=",
  );
});

test("detects the format when clipboard files omit a MIME type", async () => {
  const [attachment] = await appendImageFiles(
    [],
    [fakeFile("pasted-image", "", PNG_SIGNATURE)],
  );
  assert.equal(attachment.mediaType, "image/png");
});

test("applies the same metadata limits to command-read images", () => {
  const [attachment] = appendImageAttachments([], [
    {
      name: "dropped.png",
      mediaType: "image/png",
      sizeBytes: PNG_SIGNATURE.byteLength,
      data: "iVBORw0KGgo=",
    },
  ]);
  assert.equal(attachment.name, "dropped.png");
  assert.equal(attachment.mediaType, "image/png");

  assert.throws(
    () =>
      appendImageAttachments([], [
        {
          name: "huge.png",
          mediaType: "image/png",
          sizeBytes: MAX_IMAGE_ATTACHMENT_BYTES + 1,
          data: "",
        },
      ]),
    /5 MiB per-image limit/,
  );
});

test("rejects mismatched or unsupported image payloads", async () => {
  await assert.rejects(
    appendImageFiles(
      [],
      [fakeFile("wrong.jpg", "image/jpeg", PNG_SIGNATURE)],
    ),
    /does not match its declared image type/,
  );
  await assert.rejects(
    appendImageFiles(
      [],
      [fakeFile("text.png", "image/png", Uint8Array.from([1, 2, 3]))],
    ),
    /does not contain a valid PNG, JPEG, or WebP image/,
  );
});

test("enforces count, per-image, and aggregate limits before sending", async () => {
  await assert.rejects(
    appendImageFiles(
      Array.from({ length: MAX_IMAGE_ATTACHMENTS_PER_MESSAGE }, () =>
        existingAttachment(),
      ),
      [fakeFile("extra.png", "image/png", PNG_SIGNATURE)],
    ),
    /Attach at most 8 images/,
  );

  const oversized = fakeFile("huge.png", "image/png", PNG_SIGNATURE);
  Object.defineProperty(oversized, "size", {
    value: MAX_IMAGE_ATTACHMENT_BYTES + 1,
  });
  await assert.rejects(appendImageFiles([], [oversized]), /5 MiB per-image limit/);

  await assert.rejects(
    appendImageFiles(
      [existingAttachment(MAX_IMAGE_ATTACHMENTS_TOTAL_BYTES)],
      [fakeFile("extra.png", "image/png", PNG_SIGNATURE)],
    ),
    /20 MiB total limit/,
  );
});
