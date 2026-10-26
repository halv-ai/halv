import type { AgentLaunchCommand } from "./ipc.ts";
import { briefSharingAnswer } from "./preferences.ts";
import type { AppPreferences, BriefSharing, CloudAccess } from "./types.ts";

export { briefSharingAnswer };

// ADR 0023: a task brief leaves the device only after the user says yes once.
// - The prompt asks before a launch that the user started. The launch waits
//   for the answer.
// - The card asks after a launch without a user action, for example a
//   session restore at startup or a launch from the phone remote. That
//   launch starts without routing.
// - Settings > JEV changes the answer.
// Halv never asks a signed-out install or an account without Halv Cloud.

/**
 * Rust emits this event after a launch that started without routing only
 * because the user did not answer yet (`agent_sessions`).
 */
export const BRIEF_SHARING_NEEDED_EVENT = "brief-sharing-needed";

/** The README section that explains task briefs. */
export const BRIEF_SHARING_LEARN_MORE_URL =
  "https://github.com/halv-ai/halv#task-briefs";

/**
 * A launch counts as a user action when the user clicked or typed this
 * recently. A launch can wait for a lane refresh first, so the window is
 * longer than one click.
 */
export const UI_MOMENT_WINDOW_MS = 10_000;

/** The agents whose panes can route tasks. */
const ROUTING_AGENT_KINDS: ReadonlySet<string> = new Set(["claude", "codex"]);

/**
 * True when the pane of a launch can route tasks. A duplicate copies a pane
 * of an unknown agent, so it counts.
 */
export function launchCanRoute(
  name: AgentLaunchCommand,
  args: unknown,
): boolean {
  if (name === "agent_terminal_duplicate") return true;
  const agentKind =
    args && typeof args === "object" && "agentKind" in args
      ? (args as { agentKind?: unknown }).agentKind
      : null;
  return typeof agentKind === "string" && ROUTING_AGENT_KINDS.has(agentKind);
}

/** Where Halv asks the task brief question for one launch. */
export type BriefSharingAsk = "none" | "prompt" | "card";

export type BriefSharingAskInput = {
  taskRouting: boolean;
  briefSharing: BriefSharing;
  access: Pick<CloudAccess, "granted">;
  launchCanRoute: boolean;
  /** The user started the launch with a click or a key press. */
  uiMoment: boolean;
  /** The user closed the question in this app run without an answer. */
  dismissed: boolean;
};

/**
 * Halv asks only when routing is on, the user did not answer, the account
 * has Halv Cloud, and the pane can route. A launch that the user started
 * gets the prompt. Any other launch gets the card.
 */
export function briefSharingAsk(input: BriefSharingAskInput): BriefSharingAsk {
  if (
    !input.taskRouting ||
    briefSharingAnswer(input.briefSharing) !== "unset" ||
    !input.access.granted ||
    !input.launchCanRoute ||
    input.dismissed
  ) {
    return "none";
  }
  return input.uiMoment ? "prompt" : "card";
}

export type BriefSharingView = {
  /** The visible question. `none` hides both surfaces. */
  surface: BriefSharingAsk;
  /** An answer is saving. */
  pending: boolean;
  error: string | null;
};

export type BriefSharingGateDependencies = {
  loadPreferences: () => Promise<
    Pick<AppPreferences, "taskRouting" | "briefSharing">
  >;
  save: (
    answer: Exclude<BriefSharing, "unset">,
  ) => Promise<Pick<AppPreferences, "briefSharing">>;
  now: () => number;
};

const HIDDEN: BriefSharingView = { surface: "none", pending: false, error: null };

/**
 * Asks the task brief question at the right moment and holds a launch that
 * the user started until the answer is saved. The IPC layer calls
 * `beforeLaunch` for each agent launch command.
 */
export class BriefSharingGate {
  private view: BriefSharingView = HIDDEN;
  private readonly listeners = new Set<() => void>();
  private readonly waiting: Array<() => void> = [];
  /** `null` until the account status loads. */
  private access: Pick<CloudAccess, "granted"> | null = null;
  private lastUserInputAt = Number.NEGATIVE_INFINITY;
  private dismissed = false;
  /** A launch started while the account status was still loading. */
  private deferredCard = false;
  /** The mounted views that can show the prompt. */
  private hosts = 0;
  private readonly dependencies: BriefSharingGateDependencies;

  constructor(dependencies: BriefSharingGateDependencies) {
    this.dependencies = dependencies;
  }

  getSnapshot = () => this.view;

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /**
   * The Halv Cloud access of the account, or `null` while the status loads.
   * Without access, Halv never asks. A launch that started while the status
   * loaded gets the card when access arrives.
   */
  setAccess(access: Pick<CloudAccess, "granted"> | null) {
    this.access = access ? { granted: access.granted } : null;
    if (!access) return;
    if (!access.granted) {
      this.deferredCard = false;
      if (this.view.surface !== "none") this.close();
      return;
    }
    if (this.deferredCard && !this.dismissed && this.view.surface === "none") {
      this.publish({ ...HIDDEN, surface: "card" });
    }
    this.deferredCard = false;
  }

  /**
   * A view that shows the question calls this when it mounts. Without a
   * view, a launch never waits: it gets the card instead of the prompt. A
   * held launch starts when the last view unmounts.
   */
  attachHost() {
    this.hosts += 1;
    let attached = true;
    return () => {
      if (!attached) return;
      attached = false;
      this.hosts -= 1;
      if (this.hosts === 0 && this.view.surface === "prompt") {
        this.publish({ ...this.view, surface: "card" });
        this.releaseWaiting();
      }
    };
  }

  /** True after the account status loaded. */
  accessKnown() {
    return this.access !== null;
  }

  /** Records a click or a key press of the user. */
  noteUserInput(at = this.dependencies.now()) {
    this.lastUserInputAt = at;
  }

  /** Settings saved an answer. An open question closes. */
  applyPreferences(preferences: Pick<AppPreferences, "briefSharing">) {
    if (briefSharingAnswer(preferences.briefSharing) === "unset") return;
    this.deferredCard = false;
    this.close();
  }

  /**
   * Runs before an agent launch. It returns at once, unless the launch gets
   * the prompt. Then it returns after the user answers or closes it.
   */
  beforeLaunch = async (name: AgentLaunchCommand, args: unknown) => {
    const uiMoment =
      this.dependencies.now() - this.lastUserInputAt <= UI_MOMENT_WINDOW_MS;
    await this.ask(launchCanRoute(name, args), uiMoment);
  };

  /**
   * Rust started a pane without routing because the user did not answer
   * yet. A launch from the phone remote is an example. Such a launch has no
   * IPC gate, so it gets the card. It never waits. After the IPC gate
   * asked, a second call changes nothing.
   */
  launchedWithoutAnswer = async () => {
    await this.ask(true, false);
  };

  private async ask(canRoute: boolean, uiMoment: boolean) {
    let preferences: Pick<AppPreferences, "taskRouting" | "briefSharing">;
    try {
      preferences = await this.dependencies.loadPreferences();
    } catch {
      // Rust reads the saved answer at launch, so the launch stays safe.
      return;
    }
    const input = {
      taskRouting: preferences.taskRouting,
      briefSharing: preferences.briefSharing,
      launchCanRoute: canRoute,
      uiMoment,
      dismissed: this.dismissed,
    };
    if (this.access === null) {
      // Halv cannot tell a signed-out install yet, so it does not hold the
      // launch. The launch starts without routing.
      if (briefSharingAsk({ ...input, access: { granted: true } }) !== "none") {
        this.deferredCard = true;
      }
      return;
    }
    const asked = briefSharingAsk({ ...input, access: this.access });
    // Nobody can see a prompt without a view, so the launch must not wait.
    const ask = asked === "prompt" && this.hosts === 0 ? "card" : asked;
    if (ask === "none") return;
    if (ask === "card") {
      if (this.view.surface === "none") this.publish({ ...HIDDEN, surface: "card" });
      return;
    }
    // A card that is saving keeps its state when it becomes the prompt.
    if (this.view.surface !== "prompt") {
      this.publish({ ...this.view, surface: "prompt" });
    }
    await new Promise<void>((resolve) => this.waiting.push(resolve));
  }

  /** Saves an answer. A held launch continues after the save. */
  async answer(answer: Exclude<BriefSharing, "unset">) {
    if (this.view.pending || this.view.surface === "none") return;
    this.publish({ ...this.view, pending: true, error: null });
    try {
      const saved = await this.dependencies.save(answer);
      if (briefSharingAnswer(saved.briefSharing) === "unset") {
        throw new Error("The answer was not saved.");
      }
      this.close();
    } catch (error) {
      this.publish({
        ...this.view,
        pending: false,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }

  /**
   * Closes the question without an answer. A held launch starts without
   * routing. Halv does not ask again in this app run.
   */
  dismiss() {
    if (this.view.pending) return;
    this.dismissed = true;
    this.close();
  }

  private close() {
    this.publish(HIDDEN);
    this.releaseWaiting();
  }

  private releaseWaiting() {
    for (const resolve of this.waiting.splice(0)) resolve();
  }

  private publish(next: BriefSharingView) {
    this.view = next;
    for (const listener of this.listeners) listener();
  }
}
