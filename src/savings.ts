export type InputSavingsCalculation = {
  compressedInputTokens: number;
  originalInputTokens: number;
  percent: number;
  savedTokens: number;
};

export function calculateInputSavings(
  compressedInputTokens: number | null | undefined,
  tokensSaved: number | null | undefined,
): InputSavingsCalculation | null {
  if (
    compressedInputTokens === null ||
    compressedInputTokens === undefined ||
    tokensSaved === null ||
    tokensSaved === undefined ||
    !Number.isFinite(compressedInputTokens) ||
    !Number.isFinite(tokensSaved) ||
    compressedInputTokens < 0
  ) {
    return null;
  }

  const savedTokens = Math.max(0, tokensSaved);
  const originalInputTokens = compressedInputTokens + savedTokens;
  if (originalInputTokens <= 0) {
    return null;
  }

  return {
    compressedInputTokens,
    originalInputTokens,
    percent: (savedTokens / originalInputTokens) * 100,
    savedTokens,
  };
}
