import type { ImageAttachment } from "./types";

export const MAX_IMAGE_ATTACHMENTS_PER_MESSAGE = 8;
export const MAX_IMAGE_ATTACHMENT_BYTES = 5 * 1024 * 1024;
export const MAX_IMAGE_ATTACHMENTS_TOTAL_BYTES = 20 * 1024 * 1024;
export const MAX_IMAGE_ATTACHMENT_NAME_CHARS = 160;

const supportedMediaTypes = new Set(["image/png", "image/jpeg", "image/webp"]);

export type ImageAttachmentData = Omit<ImageAttachment, "id">;

type ImageAttachmentMetadata = {
  name: string;
  mediaType: string | null;
  sizeBytes: number;
};

export async function appendImageFiles(
  existing: readonly ImageAttachment[],
  files: Iterable<File>,
): Promise<ImageAttachment[]> {
  const additions = Array.from(files);
  const metadata = validateImageAttachmentMetadata(
    existing,
    additions.map((file) => ({
      name: file.name,
      mediaType: file.type || null,
      sizeBytes: file.size,
    })),
  );
  const result = [...existing];
  for (const [index, file] of additions.entries()) {
    const validated = metadata[index];
    const bytes = new Uint8Array(await file.arrayBuffer());
    const detectedType = detectImageMediaType(bytes);
    if (!detectedType) {
      throw new Error(
        `“${validated.name}” does not contain a valid PNG, JPEG, or WebP image.`,
      );
    }
    if (validated.mediaType && validated.mediaType !== detectedType) {
      throw new Error(
        `“${validated.name}” content does not match its declared image type.`,
      );
    }
    result.push({
      id: attachmentId(),
      name: validated.name,
      mediaType: detectedType,
      sizeBytes: bytes.byteLength,
      data: bytesToBase64(bytes),
    });
  }
  return result;
}

export function appendImageAttachments(
  existing: readonly ImageAttachment[],
  additions: readonly ImageAttachmentData[],
): ImageAttachment[] {
  const metadata = validateImageAttachmentMetadata(existing, additions);
  return [
    ...existing,
    ...additions.map((addition, index) => ({
      id: attachmentId(),
      name: metadata[index].name,
      mediaType: requiredMediaType(metadata[index]),
      sizeBytes: metadata[index].sizeBytes,
      data: addition.data,
    })),
  ];
}

export function mergeImageAttachments(
  existing: readonly ImageAttachment[],
  additions: readonly ImageAttachmentData[],
): { attachments: ImageAttachment[]; firstError: string | null } {
  let attachments = [...existing];
  let firstError: string | null = null;
  for (const addition of additions) {
    try {
      attachments = appendImageAttachments(attachments, [addition]);
    } catch (error) {
      firstError ??= error instanceof Error ? error.message : String(error);
    }
  }
  return { attachments, firstError };
}

export function imageAttachmentDataUrl(attachment: ImageAttachment): string {
  return `data:${attachment.mediaType};base64,${attachment.data}`;
}

function validateFilename(value: string): string {
  const name = value.trim();
  if (!name) {
    throw new Error("Every image attachment needs a filename.");
  }
  if (Array.from(name).length > MAX_IMAGE_ATTACHMENT_NAME_CHARS) {
    throw new Error(
      `Image filenames must be at most ${MAX_IMAGE_ATTACHMENT_NAME_CHARS} characters.`,
    );
  }
  if (/[/\\\u0000-\u001f\u007f]/u.test(name)) {
    throw new Error(`“${name}” is not a safe image filename.`);
  }
  return name;
}

function validateImageAttachmentMetadata(
  existing: readonly ImageAttachment[],
  additions: readonly ImageAttachmentMetadata[],
): Array<{
  name: string;
  mediaType: ImageAttachment["mediaType"] | null;
  sizeBytes: number;
}> {
  if (existing.length + additions.length > MAX_IMAGE_ATTACHMENTS_PER_MESSAGE) {
    throw new Error(
      `Attach at most ${MAX_IMAGE_ATTACHMENTS_PER_MESSAGE} images to one message.`,
    );
  }

  let totalBytes = existing.reduce(
    (sum, attachment) => sum + attachment.sizeBytes,
    0,
  );
  return additions.map((addition) => {
    const name = validateFilename(addition.name);
    if (addition.sizeBytes === 0) {
      throw new Error(`“${name}” is empty.`);
    }
    if (addition.sizeBytes > MAX_IMAGE_ATTACHMENT_BYTES) {
      throw new Error(`“${name}” exceeds the 5 MiB per-image limit.`);
    }
    totalBytes += addition.sizeBytes;
    if (totalBytes > MAX_IMAGE_ATTACHMENTS_TOTAL_BYTES) {
      throw new Error("Attached images exceed the 20 MiB total limit.");
    }

    const mediaType = addition.mediaType
      ? normalizeMediaType(addition.mediaType)
      : null;
    if (addition.mediaType && !mediaType) {
      throw new Error(
        `“${name}” is not a supported image. Use PNG, JPEG, or WebP.`,
      );
    }
    return { name, mediaType, sizeBytes: addition.sizeBytes };
  });
}

function requiredMediaType(metadata: {
  name: string;
  mediaType: ImageAttachment["mediaType"] | null;
}): ImageAttachment["mediaType"] {
  if (!metadata.mediaType) {
    throw new Error(
      `“${metadata.name}” is not a supported image. Use PNG, JPEG, or WebP.`,
    );
  }
  return metadata.mediaType;
}

function normalizeMediaType(value: string): ImageAttachment["mediaType"] | null {
  const normalized = value.trim().toLowerCase() === "image/jpg"
    ? "image/jpeg"
    : value.trim().toLowerCase();
  return supportedMediaTypes.has(normalized)
    ? (normalized as ImageAttachment["mediaType"])
    : null;
}

function detectImageMediaType(
  bytes: Uint8Array,
): ImageAttachment["mediaType"] | null {
  if (
    bytes.length >= 8 &&
    bytes[0] === 0x89 &&
    bytes[1] === 0x50 &&
    bytes[2] === 0x4e &&
    bytes[3] === 0x47 &&
    bytes[4] === 0x0d &&
    bytes[5] === 0x0a &&
    bytes[6] === 0x1a &&
    bytes[7] === 0x0a
  ) {
    return "image/png";
  }
  if (
    bytes.length >= 3 &&
    bytes[0] === 0xff &&
    bytes[1] === 0xd8 &&
    bytes[2] === 0xff
  ) {
    return "image/jpeg";
  }
  if (
    bytes.length >= 12 &&
    String.fromCharCode(...bytes.slice(0, 4)) === "RIFF" &&
    String.fromCharCode(...bytes.slice(8, 12)) === "WEBP"
  ) {
    return "image/webp";
  }
  return null;
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary);
}

function attachmentId(): string {
  return globalThis.crypto?.randomUUID?.() ??
    `image-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}
