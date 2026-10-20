export type CustomPromptScope = "user" | "codex" | "project";

export type CustomPrompt = {
  name: string;
  body: string;
  scope: CustomPromptScope;
};

export type ResolvedCustomPrompt = CustomPrompt & {
  commandName: string;
};

const ARGUMENTS_PLACEHOLDER = "$ARGUMENTS";

export function resolveCustomPromptNames(
  prompts: CustomPrompt[],
  reservedNames: Iterable<string>,
): ResolvedCustomPrompt[] {
  const reserved = new Set(
    Array.from(reservedNames, (name) => name.toLowerCase()),
  );
  return prompts.map((prompt) => ({
    ...prompt,
    commandName: reserved.has(prompt.name.toLowerCase())
      ? `user:${prompt.name}`
      : prompt.name,
  }));
}

export function expandCustomPrompt(body: string, args: string): string {
  if (body.includes(ARGUMENTS_PLACEHOLDER)) {
    return body.split(ARGUMENTS_PLACEHOLDER).join(args);
  }
  if (!args) {
    return body;
  }
  if (!body) {
    return args;
  }
  return `${body}${body.endsWith("\n") ? "" : "\n"}${args}`;
}
