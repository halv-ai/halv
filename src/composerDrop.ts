export type DroppedFileMention = {
  token: string;
  path: string;
};

export type DroppedMentionInsertion = {
  draft: string;
  cursor: number;
  mentions: DroppedFileMention[];
};

const DROPPED_IMAGE_EXTENSION = /\.(?:png|jpe?g|webp)$/iu;

export function isDroppedImagePath(path: string): boolean {
  return DROPPED_IMAGE_EXTENSION.test(path.trim());
}

export function partitionDroppedPaths(paths: readonly string[]) {
  const imagePaths: string[] = [];
  const mentionPaths: string[] = [];
  for (const path of paths) {
    (isDroppedImagePath(path) ? imagePaths : mentionPaths).push(path);
  }
  return { imagePaths, mentionPaths };
}

export function acceptedNativeDropPaths(
  paths: readonly string[],
  insideComposer: boolean,
): string[] {
  return insideComposer ? [...paths] : partitionDroppedPaths(paths).imagePaths;
}

export function droppedMentionPath(
  absolutePath: string,
  projectRoot: string | null,
): string {
  if (!projectRoot) {
    return absolutePath;
  }

  const normalizedPath = normalizePath(absolutePath);
  const normalizedRoot = trimTrailingSeparators(normalizePath(projectRoot));
  if (!normalizedRoot || normalizedPath === normalizedRoot) {
    return absolutePath;
  }

  const windowsPath = /^[a-z]:\//iu.test(normalizedRoot);
  const comparablePath = windowsPath ? normalizedPath.toLowerCase() : normalizedPath;
  const comparableRoot = windowsPath ? normalizedRoot.toLowerCase() : normalizedRoot;
  const rootHasSeparator = comparableRoot.endsWith("/");
  const relativeStart = rootHasSeparator ? comparableRoot : `${comparableRoot}/`;
  if (!comparablePath.startsWith(relativeStart)) {
    return absolutePath;
  }
  return normalizedPath.slice(normalizedRoot.length + (rootHasSeparator ? 0 : 1));
}

export function insertDroppedFileMentions(
  draft: string,
  cursor: number,
  absolutePaths: readonly string[],
  projectRoot: string | null,
): DroppedMentionInsertion {
  const seen = new Set<string>();
  const mentions = absolutePaths.flatMap((absolutePath) => {
    const path = droppedMentionPath(absolutePath, projectRoot);
    if (!path || seen.has(path)) {
      return [];
    }
    seen.add(path);
    return [{ token: `@{${path}}`, path }];
  });
  const boundedCursor = Math.max(0, Math.min(cursor, draft.length));
  if (mentions.length === 0) {
    return { draft, cursor: boundedCursor, mentions };
  }

  const before = draft.slice(0, boundedCursor);
  const leadingSpace = before && !/\s$/u.test(before) ? " " : "";
  const insertion = `${leadingSpace}${mentions.map(({ token }) => token).join(" ")} `;
  return {
    draft: `${before}${insertion}${draft.slice(boundedCursor)}`,
    cursor: boundedCursor + insertion.length,
    mentions,
  };
}

function normalizePath(path: string): string {
  return path.replace(/\\/gu, "/");
}

function trimTrailingSeparators(path: string): string {
  if (path === "/" || /^[a-z]:\/$/iu.test(path)) {
    return path;
  }
  return path.replace(/\/+$/u, "");
}
