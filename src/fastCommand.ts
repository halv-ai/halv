export type FastCommandAction =
  | { kind: "status" }
  | { kind: "set"; enabled: boolean };

export function parseFastCommand(args: string): FastCommandAction {
  const requested = args.trim().toLowerCase();
  if (!requested || requested === "status") {
    return { kind: "status" };
  }
  if (requested === "on") {
    return { kind: "set", enabled: true };
  }
  if (requested === "off") {
    return { kind: "set", enabled: false };
  }
  throw new Error("Usage: /fast [on|off|status]");
}
