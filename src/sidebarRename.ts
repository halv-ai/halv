export function inlineRenameCommitValue(
  currentTitle: string,
  draft: string,
): string | null {
  const title = draft.trim();
  return title && title !== currentTitle ? title : null;
}
