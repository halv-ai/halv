import type { CommandRegistryEntry } from "./commands";

export type CommandMatch = {
  command: CommandRegistryEntry;
  score: number;
};

export function matchCommands(
  commands: CommandRegistryEntry[],
  query: string,
): CommandMatch[] {
  const normalized = query.trim().toLowerCase();
  if (!normalized) {
    return commands.map((command, index) => ({ command, score: index }));
  }

  const tokens = normalized.split(/\s+/).filter(Boolean);
  return commands
    .map((command, index) => {
      const name = command.name.toLowerCase();
      const aliases = (command.aliases ?? []).map((alias) => alias.toLowerCase());
      const haystack = [
        command.name,
        command.description,
        command.category,
        ...(command.aliases ?? []),
        ...(command.keywords ?? []),
      ]
        .join(" ")
        .toLowerCase();
      if (!tokens.every((token) => haystack.includes(token))) {
        return null;
      }

      let score = 500 + index;
      if (name === normalized) {
        score = 0;
      } else if (aliases.includes(normalized)) {
        score = 5;
      } else if (name.startsWith(normalized)) {
        score = 10 + name.length - normalized.length;
      } else if (aliases.some((alias) => alias.startsWith(normalized))) {
        score = 30;
      } else if (name.includes(normalized)) {
        score = 60 + name.indexOf(normalized);
      } else {
        score =
          100 +
          tokens.reduce((total, token) => total + haystack.indexOf(token), 0);
      }
      return { command, score };
    })
    .filter((match): match is CommandMatch => match !== null)
    .sort(
      (left, right) =>
        left.score - right.score ||
        left.command.name.localeCompare(right.command.name),
    );
}
