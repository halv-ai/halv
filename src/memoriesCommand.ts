export type MemoriesCommandAction =
  | { kind: "open" }
  | { kind: "set"; mode: "off" | "on" };

export function parseMemoriesCommand(args: string): MemoriesCommandAction {
  const requested = args.trim().toLowerCase();
  if (!requested) {
    return { kind: "open" };
  }
  if (requested !== "on" && requested !== "off") {
    throw new Error("Usage: /memories [on|off]");
  }
  return { kind: "set", mode: requested };
}
