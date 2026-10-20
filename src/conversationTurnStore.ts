import { markConversationUnviewed, migrateConversationUnviewed } from "./conversationUnviewed.ts";
import { upsertStreamingAssistant } from "./messageStream.ts";
import { shouldAdoptPendingNewTurn } from "./conversationTurnRouting.ts";
import {
  NEW_THREAD,
  type BranchStatus,
  type ChatDeltaPayload,
  type ConversationCompaction,
  type TaskPlan,
  type TokenUsage,
  type ToolCallUi,
  type TurnDiffSummary,
  type UiMessage,
} from "./types.ts";

export type PendingNewTurn = {
  conversationId: string | null;
  freshViewRequested: boolean;
  cancelDispatched: boolean;
  navigationGeneration: number;
  knownConversationIds: Set<string>;
};

export type TurnState = {
  startedAt: number;
  pendingNewTurn: PendingNewTurn | null;
  cancelRequested: boolean;
  normalCancelRequested: boolean;
  cancelAndRevertRequested: boolean;
  cancelAndRevertDispatched: boolean;
  cancelAndRevertPending: boolean;
  cancelAndRevertTurn: boolean;
  cancelAndRevertDrain: boolean;
  stopAndRevertAvailable: boolean;
};

export type ConversationTurnStoreState = {
  toolCallsByConversation: Record<string, ToolCallUi[]>;
  reasoningByConversation: Record<string, string>;
  turnDiffsByConversation: Record<string, TurnDiffSummary[]>;
  taskPlansByConversation: Record<string, TaskPlan>;
  compactionsByConversation: Record<string, ConversationCompaction>;
  unviewedConversationIds: Set<string>;
  pendingApprovals: Record<string, ToolCallUi>;
  streamBuffersByConversation: Map<string, string>;
  runningConversationIds: Set<string>;
  conversationTurnStates: Map<string, TurnState>;
  lastNewThreadTurnId: string | null;
};

export type ConversationTurnContext = {
  currentConversationId: string | null;
  deletedConversationIds: ReadonlySet<string>;
  navigationGeneration: number;
  chatRequestFailedText: string;
  now: string;
};

export type EffectRequest =
  | {
      type: "setConversationRunning";
      conversationId: string;
      running: boolean;
    }
  | { type: "notifyTerminal"; delta: ChatDeltaPayload }
  | { type: "loadConversations" }
  | { type: "refreshStatus" }
  | { type: "refreshSavingsTotals" }
  | { type: "refreshAgentSessions" }
  | {
      type: "migrateConversationTurnState";
      fromConversationId: string | null;
      toConversationId: string;
      pendingNewTurn: PendingNewTurn;
    }
  | {
      type: "switchComposerDraft";
      conversationId: string;
      carryNewConversationDraft: boolean;
    }
  | { type: "openConversation"; conversationId: string }
  | { type: "requestChatCancel"; conversationId: string }
  | { type: "requestChatCancelAndRevert"; conversationId: string }
  | { type: "dispatchQueuedNewThreadMessage" }
  | { type: "notifyApproval"; delta: ChatDeltaPayload }
  | { type: "loadSubagents"; conversationId: string }
  | { type: "setBranchStatus"; delta: ChatDeltaPayload }
  | { type: "loadProjects" }
  | { type: "handleGoalSignal"; delta: ChatDeltaPayload }
  | {
      type: "appendStreamingAssistant";
      conversationId: string;
      text: string;
    }
  | { type: "finishStreamingAssistant"; usage?: TokenUsage }
  | { type: "setChatError"; message: string };

export type ReduceChatDeltaResult = {
  state: ConversationTurnStoreState;
  effects: EffectRequest[];
};

export type ConversationTurnEffects = {
  setMessages(updater: (current: UiMessage[]) => UiMessage[]): void;
  /** Shows the conversation that a new-thread turn created. */
  openConversation(conversationId: string): void;
  setChatError(message: string | null): void;
  setBranchStatus(
    updater: (current: BranchStatus | null) => BranchStatus,
  ): void;
  setConversationRunning(
    conversationId: string | null,
    running: boolean,
  ): void;
  loadConversations(): void;
  refreshStatus(): void;
  refreshSavingsTotals(): void;
  loadSubagents(conversationId: string): void;
  loadProjects(): void;
  refreshAgentSessions(): void;
  notifyTerminal(delta: ChatDeltaPayload): void;
  notifyApproval(delta: ChatDeltaPayload): void;
  handleGoalSignal(delta: ChatDeltaPayload): void;
  migrateConversationTurnState(
    fromConversationId: string | null,
    toConversationId: string,
    pendingNewTurn: PendingNewTurn,
  ): void;
  switchComposerDraft(
    conversationId: string,
    carryNewConversationDraft: boolean,
  ): void;
  requestChatCancel(conversationId: string): void;
  requestChatCancelAndRevert(conversationId: string): void;
  dispatchQueuedNewThreadMessage(): void;
};

type StateUpdater<T> = T | ((current: T) => T);

export type ChatDeltaSubscriber = (
  handler: (delta: ChatDeltaPayload) => void,
) => Promise<() => void> | (() => void);

export type ConversationTurnStore = ReturnType<
  typeof createConversationTurnStore
>;

export function emptyConversationTurnStoreState(): ConversationTurnStoreState {
  return {
    toolCallsByConversation: {},
    reasoningByConversation: {},
    turnDiffsByConversation: {},
    taskPlansByConversation: {},
    compactionsByConversation: {},
    unviewedConversationIds: new Set(),
    pendingApprovals: {},
    streamBuffersByConversation: new Map(),
    runningConversationIds: new Set(),
    conversationTurnStates: new Map(),
    lastNewThreadTurnId: null,
  };
}

export function finishStreamingAssistant(
  messages: UiMessage[],
  usage?: TokenUsage,
) {
  return messages.map((message) =>
    message.role === "assistant" && message.streaming
      ? { ...message, streaming: false, usage }
      : message,
  );
}

export function upsertToolCall(
  toolCalls: ToolCallUi[],
  payload: ChatDeltaPayload,
) {
  if (!payload.callId) {
    return toolCalls;
  }
  const existing = toolCalls.find(
    (toolCall) => toolCall.callId === payload.callId,
  );
  const next: ToolCallUi = {
    callId: payload.callId,
    toolKind: payload.toolKind ?? existing?.toolKind ?? "shell",
    command: payload.command ?? existing?.command ?? "",
    cwd: payload.cwd ?? existing?.cwd ?? "",
    status: payload.status ?? existing?.status ?? "running",
    justification: payload.justification ?? existing?.justification,
    tail: existing?.tail ?? "",
    exitCode: existing?.exitCode,
    wallTimeMs: existing?.wallTimeMs,
    truncatedOutput: existing?.truncatedOutput,
    rtkRoute: payload.rtkRoute ?? existing?.rtkRoute,
    fileChanges: payload.fileChanges ?? existing?.fileChanges,
    warning: payload.warning ?? existing?.warning,
    elicitationMode: payload.elicitationMode ?? existing?.elicitationMode,
    elicitationSchema: payload.elicitationSchema ?? existing?.elicitationSchema,
    elicitationUrl: payload.elicitationUrl ?? existing?.elicitationUrl,
  };
  return [
    ...toolCalls.filter((toolCall) => toolCall.callId !== next.callId),
    next,
  ];
}

export function appendToolOutput(
  toolCalls: ToolCallUi[],
  payload: ChatDeltaPayload,
) {
  if (!payload.callId || !payload.tail) {
    return toolCalls;
  }
  const tailLimit = 12_000;
  return toolCalls.map((toolCall) =>
    toolCall.callId === payload.callId
      ? {
          ...toolCall,
          tail: `${toolCall.tail}${payload.tail}`.slice(-tailLimit),
        }
      : toolCall,
  );
}

export function finishToolCall(
  toolCalls: ToolCallUi[],
  payload: ChatDeltaPayload,
) {
  if (!payload.callId) {
    return toolCalls;
  }
  return toolCalls.map((toolCall) =>
    toolCall.callId === payload.callId
      ? {
          ...toolCall,
          status: payload.status ?? toolCall.status,
          exitCode: payload.exitCode ?? toolCall.exitCode,
          wallTimeMs: payload.wallTimeMs ?? toolCall.wallTimeMs,
          truncatedOutput:
            payload.truncatedOutput ?? toolCall.truncatedOutput ?? false,
          rtkRoute: payload.rtkRoute ?? toolCall.rtkRoute,
        }
      : toolCall,
  );
}

export function isTerminalDelta(kind: ChatDeltaPayload["kind"]) {
  return kind === "done" || kind === "error";
}

export function upsertTurnDiff(
  turnDiffs: TurnDiffSummary[],
  payload: ChatDeltaPayload,
  createdAt = new Date().toISOString(),
) {
  if (!payload.turnId || payload.diff === undefined) {
    return turnDiffs;
  }
  if (payload.diff === "") {
    return turnDiffs.filter((turnDiff) => turnDiff.turnId !== payload.turnId);
  }

  const existing = turnDiffs.find(
    (turnDiff) => turnDiff.turnId === payload.turnId,
  );
  const next: TurnDiffSummary = {
    turnId: payload.turnId,
    diff: `${existing?.diff ?? ""}${payload.diff}`,
    cwd: existing?.cwd ?? payload.cwd ?? "",
    createdAt: existing?.createdAt ?? createdAt,
    revertible:
      existing?.revertible === false ? false : (payload.revertible ?? true),
  };
  return [
    ...turnDiffs.filter((turnDiff) => turnDiff.turnId !== payload.turnId),
    next,
  ];
}

function migrateTurnState(
  state: ConversationTurnStoreState,
  fromConversationId: string | null,
  toConversationId: string,
  currentConversationId?: string | null,
) {
  const fromKey = fromConversationId ?? NEW_THREAD;
  if (fromKey === toConversationId) {
    return state;
  }
  const turnState = state.conversationTurnStates.get(fromKey);
  if (!turnState) {
    return state;
  }

  const pendingNewTurn = turnState.pendingNewTurn
    ? { ...turnState.pendingNewTurn, conversationId: toConversationId }
    : null;
  const nextTurnState = { ...turnState, pendingNewTurn };
  const conversationTurnStates = new Map(state.conversationTurnStates);
  conversationTurnStates.delete(fromKey);
  conversationTurnStates.set(toConversationId, nextTurnState);
  const runningConversationIds = new Set(state.runningConversationIds);
  runningConversationIds.delete(fromKey);
  runningConversationIds.add(toConversationId);
  const streamBuffersByConversation = new Map(
    state.streamBuffersByConversation,
  );
  streamBuffersByConversation.set(toConversationId, "");
  return {
    ...state,
    conversationTurnStates,
    runningConversationIds,
    streamBuffersByConversation,
    reasoningByConversation: {
      ...state.reasoningByConversation,
      [toConversationId]: "",
    },
    unviewedConversationIds: migrateConversationUnviewed(
      state.unviewedConversationIds,
      fromKey,
      toConversationId,
    ),
    lastNewThreadTurnId:
      fromConversationId === null &&
      (pendingNewTurn?.freshViewRequested === true ||
        currentConversationId === null ||
        state.lastNewThreadTurnId === fromKey)
        ? toConversationId
        : state.lastNewThreadTurnId,
  };
}

function setRunning(
  state: ConversationTurnStoreState,
  conversationId: string | null,
  running: boolean,
) {
  const key = conversationId ?? NEW_THREAD;
  const isRunning = state.runningConversationIds.has(key);
  const clearsLastNewThread =
    !running && state.lastNewThreadTurnId === key;
  if (running === isRunning && !clearsLastNewThread) {
    return state;
  }
  const runningConversationIds = new Set(state.runningConversationIds);
  if (running) {
    runningConversationIds.add(key);
  } else {
    runningConversationIds.delete(key);
  }
  return {
    ...state,
    runningConversationIds,
    lastNewThreadTurnId:
      !running && state.lastNewThreadTurnId === key
        ? null
        : state.lastNewThreadTurnId,
  };
}

export function reduceChatDelta(
  state: ConversationTurnStoreState,
  delta: ChatDeltaPayload,
  context: ConversationTurnContext,
): ReduceChatDeltaResult {
  const effects: EffectRequest[] = [];
  const terminal = isTerminalDelta(delta.kind);
  if (context.deletedConversationIds.has(delta.conversationId)) {
    if (!terminal) {
      return { state, effects };
    }
    const conversationTurnStates = new Map(state.conversationTurnStates);
    conversationTurnStates.delete(delta.conversationId);
    return {
      state: {
        ...setRunning(state, delta.conversationId, false),
        conversationTurnStates,
      },
      effects: [
        {
          type: "setConversationRunning",
          conversationId: delta.conversationId,
          running: false,
        },
      ],
    };
  }

  if (terminal) {
    effects.push({ type: "notifyTerminal", delta });
  }

  if (delta.kind === "conversation_updated") {
    effects.push({ type: "loadConversations" });
    effects.push({ type: "refreshAgentSessions" });
    return { state, effects };
  }

  if (delta.kind === "context_compacted" && delta.compaction) {
    return {
      state: {
        ...state,
        compactionsByConversation: {
          ...state.compactionsByConversation,
          [delta.conversationId]: delta.compaction,
        },
      },
      effects,
    };
  }

  let nextState = state;
  let currentConversationId = context.currentConversationId;
  const pendingNewTurn = state.conversationTurnStates.get(NEW_THREAD)
    ?.pendingNewTurn;
  if (
    pendingNewTurn &&
    pendingNewTurn.conversationId === null &&
    shouldAdoptPendingNewTurn(
      delta.conversationId,
      pendingNewTurn.knownConversationIds,
      state.conversationTurnStates,
      state.runningConversationIds,
    )
  ) {
    nextState = migrateTurnState(
      state,
      null,
      delta.conversationId,
      context.currentConversationId,
    );
    effects.push({
      type: "migrateConversationTurnState",
      fromConversationId: null,
      toConversationId: delta.conversationId,
      pendingNewTurn,
    });
    effects.push({ type: "loadConversations" });
    const resolvedTurnState = nextState.conversationTurnStates.get(
      delta.conversationId,
    );
    if (
      !pendingNewTurn.freshViewRequested &&
      context.navigationGeneration === pendingNewTurn.navigationGeneration &&
      context.currentConversationId === null &&
      (!terminal || resolvedTurnState?.cancelAndRevertRequested)
    ) {
      effects.push({
        type: "switchComposerDraft",
        conversationId: delta.conversationId,
        carryNewConversationDraft: true,
      });
      effects.push({
        type: "openConversation",
        conversationId: delta.conversationId,
      });
      currentConversationId = delta.conversationId;
    }
    if (resolvedTurnState?.cancelAndRevertRequested) {
      effects.push({
        type: "requestChatCancelAndRevert",
        conversationId: delta.conversationId,
      });
    } else if (resolvedTurnState?.cancelRequested && !terminal) {
      effects.push({
        type: "requestChatCancel",
        conversationId: delta.conversationId,
      });
    } else if (!terminal) {
      effects.push({ type: "dispatchQueuedNewThreadMessage" });
    }
  }

  if (terminal) {
    const adoptedPendingNewTurn = nextState.conversationTurnStates.get(
      delta.conversationId,
    )?.pendingNewTurn;
    const selectedConversationId =
      currentConversationId ??
      (adoptedPendingNewTurn?.conversationId === delta.conversationId &&
      !adoptedPendingNewTurn.freshViewRequested &&
      context.navigationGeneration ===
        adoptedPendingNewTurn.navigationGeneration
        ? delta.conversationId
        : null);
    nextState = {
      ...nextState,
      unviewedConversationIds: markConversationUnviewed(
        nextState.unviewedConversationIds,
        delta.conversationId,
        selectedConversationId,
      ),
    };
  }

  if (delta.kind === "tool_call") {
    const toolCalls = upsertToolCall(
      nextState.toolCallsByConversation[delta.conversationId] ?? [],
      delta,
    );
    const active = toolCalls.find(
      (toolCall) => toolCall.callId === delta.callId,
    );
    const pendingApprovals =
      active?.status === "awaiting_approval"
        ? {
            ...nextState.pendingApprovals,
            [delta.conversationId]: active,
          }
        : nextState.pendingApprovals;
    nextState = {
      ...nextState,
      toolCallsByConversation: {
        ...nextState.toolCallsByConversation,
        [delta.conversationId]: toolCalls,
      },
      pendingApprovals,
    };
    if (delta.status === "awaiting_approval") {
      effects.push({ type: "notifyApproval", delta });
    }
    if (delta.toolKind === "subagent") {
      effects.push({
        type: "loadSubagents",
        conversationId: delta.conversationId,
      });
    }
    return { state: nextState, effects };
  }

  if (delta.kind === "tool_output") {
    return {
      state: {
        ...nextState,
        toolCallsByConversation: {
          ...nextState.toolCallsByConversation,
          [delta.conversationId]: appendToolOutput(
            nextState.toolCallsByConversation[delta.conversationId] ?? [],
            delta,
          ),
        },
      },
      effects,
    };
  }

  if (delta.kind === "tool_result") {
    const pendingApprovals = { ...nextState.pendingApprovals };
    if (
      delta.callId &&
      pendingApprovals[delta.conversationId]?.callId === delta.callId
    ) {
      delete pendingApprovals[delta.conversationId];
    }
    effects.push({
      type: "loadSubagents",
      conversationId: delta.conversationId,
    });
    return {
      state: {
        ...nextState,
        toolCallsByConversation: {
          ...nextState.toolCallsByConversation,
          [delta.conversationId]: finishToolCall(
            nextState.toolCallsByConversation[delta.conversationId] ?? [],
            delta,
          ),
        },
        pendingApprovals,
      },
      effects,
    };
  }

  if (delta.kind === "turn_diff") {
    if (!delta.turnId || delta.diff === undefined) {
      return { state: nextState, effects };
    }
    return {
      state: {
        ...nextState,
        turnDiffsByConversation: {
          ...nextState.turnDiffsByConversation,
          [delta.conversationId]: upsertTurnDiff(
            nextState.turnDiffsByConversation[delta.conversationId] ?? [],
            delta,
            context.now,
          ),
        },
      },
      effects,
    };
  }

  if (delta.kind === "task_list") {
    if (!delta.plan) {
      return { state: nextState, effects };
    }
    return {
      state: {
        ...nextState,
        taskPlansByConversation: {
          ...nextState.taskPlansByConversation,
          [delta.conversationId]: {
            explanation: delta.explanation,
            plan: delta.plan,
          },
        },
      },
      effects,
    };
  }

  if (delta.kind === "branch_status") {
    if (delta.conversationId === currentConversationId) {
      effects.push({ type: "setBranchStatus", delta });
    }
    effects.push({ type: "loadProjects" });
    return { state: nextState, effects };
  }

  if (delta.kind === "done") {
    effects.push({ type: "handleGoalSignal", delta });
  }

  if (delta.kind === "delta") {
    const streamBuffersByConversation = new Map(
      nextState.streamBuffersByConversation,
    );
    streamBuffersByConversation.set(
      delta.conversationId,
      `${streamBuffersByConversation.get(delta.conversationId) ?? ""}${delta.text ?? ""}`,
    );
    nextState = { ...nextState, streamBuffersByConversation };
  }

  if (delta.kind === "reasoning") {
    return {
      state: {
        ...nextState,
        reasoningByConversation: {
          ...nextState.reasoningByConversation,
          [delta.conversationId]: `${
            nextState.reasoningByConversation[delta.conversationId] ?? ""
          }${delta.text ?? ""}`,
        },
      },
      effects,
    };
  }

  if (terminal) {
    nextState = setRunning(nextState, delta.conversationId, false);
    effects.push({
      type: "setConversationRunning",
      conversationId: delta.conversationId,
      running: false,
    });
  }

  if (delta.conversationId !== currentConversationId) {
    if (terminal) {
      effects.push({ type: "loadConversations" });
      effects.push({ type: "refreshStatus" });
      effects.push({ type: "refreshSavingsTotals" });
    }
    return { state: nextState, effects };
  }

  if (delta.kind === "delta") {
    effects.push({
      type: "appendStreamingAssistant",
      conversationId: delta.conversationId,
      text: delta.text ?? "",
    });
    return { state: nextState, effects };
  }

  if (delta.kind === "done") {
    effects.push({ type: "finishStreamingAssistant", usage: delta.usage });
    effects.push({ type: "loadConversations" });
    effects.push({ type: "refreshStatus" });
    effects.push({ type: "refreshSavingsTotals" });
    return { state: nextState, effects };
  }

  if (delta.kind === "error") {
    effects.push({ type: "finishStreamingAssistant" });
    if (
      !nextState.conversationTurnStates.get(delta.conversationId)
        ?.cancelAndRevertTurn
    ) {
      effects.push({
        type: "setChatError",
        message: delta.text ?? context.chatRequestFailedText,
      });
    }
    effects.push({ type: "loadConversations" });
    effects.push({ type: "refreshStatus" });
    effects.push({ type: "refreshSavingsTotals" });
  }

  return { state: nextState, effects };
}

function resolveUpdater<T>(current: T, updater: StateUpdater<T>) {
  return typeof updater === "function"
    ? (updater as (current: T) => T)(current)
    : updater;
}

export function createConversationTurnStore({
  subscribe: subscribeToChatDeltas,
  effects,
  context,
  initialState = emptyConversationTurnStoreState(),
}: {
  subscribe: ChatDeltaSubscriber;
  effects: ConversationTurnEffects;
  context: () => ConversationTurnContext;
  initialState?: ConversationTurnStoreState;
}) {
  let state = initialState;
  let unlisten: (() => void) | null = null;
  let connectionGeneration = 0;
  const listeners = new Set<() => void>();

  function commit(next: ConversationTurnStoreState) {
    if (next === state) {
      return;
    }
    state = next;
    for (const listener of listeners) {
      listener();
    }
  }

  function dispatch(request: EffectRequest) {
    switch (request.type) {
      case "setConversationRunning":
        effects.setConversationRunning(
          request.conversationId,
          request.running,
        );
        return;
      case "notifyTerminal":
        effects.notifyTerminal(request.delta);
        return;
      case "loadConversations":
        effects.loadConversations();
        return;
      case "refreshStatus":
        effects.refreshStatus();
        return;
      case "refreshSavingsTotals":
        effects.refreshSavingsTotals();
        return;
      case "refreshAgentSessions":
        effects.refreshAgentSessions();
        return;
      case "migrateConversationTurnState":
        effects.migrateConversationTurnState(
          request.fromConversationId,
          request.toConversationId,
          request.pendingNewTurn,
        );
        return;
      case "switchComposerDraft":
        effects.switchComposerDraft(
          request.conversationId,
          request.carryNewConversationDraft,
        );
        return;
      case "openConversation":
        effects.openConversation(request.conversationId);
        return;
      case "requestChatCancel":
        effects.requestChatCancel(request.conversationId);
        return;
      case "requestChatCancelAndRevert":
        effects.requestChatCancelAndRevert(request.conversationId);
        return;
      case "dispatchQueuedNewThreadMessage":
        effects.dispatchQueuedNewThreadMessage();
        return;
      case "notifyApproval":
        effects.notifyApproval(request.delta);
        return;
      case "loadSubagents":
        effects.loadSubagents(request.conversationId);
        return;
      case "setBranchStatus":
        effects.setBranchStatus((current) => ({
          isGitRepo: current?.isGitRepo ?? true,
          branch: request.delta.branch ?? current?.branch ?? null,
          defaultBranch: current?.defaultBranch ?? null,
          clean: current?.clean ?? null,
          autoCreatedBranch:
            request.delta.branch ?? current?.autoCreatedBranch ?? null,
          warning: request.delta.warning ?? current?.warning ?? null,
        }));
        return;
      case "loadProjects":
        effects.loadProjects();
        return;
      case "handleGoalSignal":
        effects.handleGoalSignal(request.delta);
        return;
      case "appendStreamingAssistant":
        effects.setMessages((current) =>
          upsertStreamingAssistant(
            current,
            request.conversationId,
            request.text,
          ),
        );
        return;
      case "finishStreamingAssistant":
        effects.setMessages((current) =>
          finishStreamingAssistant(current, request.usage),
        );
        return;
      case "setChatError":
        effects.setChatError(request.message);
    }
  }

  function reduce(delta: ChatDeltaPayload) {
    const result = reduceChatDelta(state, delta, context());
    commit(result.state);
    for (const request of result.effects) {
      dispatch(request);
    }
  }

  const store = {
    getSnapshot: () => state,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    async connect() {
      const generation = ++connectionGeneration;
      unlisten?.();
      unlisten = null;
      const nextUnlisten = await subscribeToChatDeltas((delta) => {
        if (generation === connectionGeneration) {
          reduce(delta);
        }
      });
      if (generation !== connectionGeneration) {
        nextUnlisten();
        return;
      }
      unlisten = nextUnlisten;
    },
    disconnect() {
      connectionGeneration += 1;
      unlisten?.();
      unlisten = null;
    },
    setConversationRunning(conversationId: string | null, running: boolean) {
      commit(setRunning(state, conversationId, running));
    },
    getConversationTurnState(conversationId: string | null) {
      return state.conversationTurnStates.get(conversationId ?? NEW_THREAD);
    },
    setConversationTurnState(
      conversationId: string | null,
      turnState: TurnState,
    ) {
      const conversationTurnStates = new Map(state.conversationTurnStates);
      conversationTurnStates.set(conversationId ?? NEW_THREAD, turnState);
      commit({ ...state, conversationTurnStates });
    },
    updateConversationTurnState(
      conversationId: string | null,
      patch: Partial<TurnState>,
    ) {
      const key = conversationId ?? NEW_THREAD;
      const current = state.conversationTurnStates.get(key);
      if (!current) {
        return null;
      }
      const next = { ...current, ...patch };
      const conversationTurnStates = new Map(state.conversationTurnStates);
      conversationTurnStates.set(key, next);
      commit({ ...state, conversationTurnStates });
      return next;
    },
    migrateConversationTurnState(
      fromConversationId: string | null,
      toConversationId: string,
      currentConversationId?: string | null,
    ) {
      const fromKey = fromConversationId ?? NEW_THREAD;
      const source = state.conversationTurnStates.get(fromKey);
      const destination = state.conversationTurnStates.get(toConversationId);
      if (!source || (destination && destination !== source)) {
        return null;
      }
      const next = migrateTurnState(
        state,
        fromConversationId,
        toConversationId,
        currentConversationId,
      );
      if (next === state) {
        return null;
      }
      commit(next);
      return next.conversationTurnStates.get(toConversationId) ?? null;
    },
    clearConversationTurnState(conversationId: string | null) {
      const key = conversationId ?? NEW_THREAD;
      if (!state.conversationTurnStates.has(key)) {
        return;
      }
      const conversationTurnStates = new Map(state.conversationTurnStates);
      conversationTurnStates.delete(key);
      commit({ ...state, conversationTurnStates });
    },
    runningTurnKeyForView(conversationId: string | null) {
      const viewedKey = conversationId ?? NEW_THREAD;
      if (state.runningConversationIds.has(viewedKey)) {
        return viewedKey;
      }
      const migratedNewThreadId = state.lastNewThreadTurnId;
      if (
        conversationId === null &&
        migratedNewThreadId &&
        state.runningConversationIds.has(migratedNewThreadId)
      ) {
        return migratedNewThreadId;
      }
      return null;
    },
    setStreamBuffer(conversationId: string, value: string) {
      const streamBuffersByConversation = new Map(
        state.streamBuffersByConversation,
      );
      streamBuffersByConversation.set(conversationId, value);
      commit({ ...state, streamBuffersByConversation });
    },
    deleteStreamBuffer(conversationId: string) {
      if (!state.streamBuffersByConversation.has(conversationId)) {
        return;
      }
      const streamBuffersByConversation = new Map(
        state.streamBuffersByConversation,
      );
      streamBuffersByConversation.delete(conversationId);
      commit({ ...state, streamBuffersByConversation });
    },
    setToolCallsByConversation(
      updater: StateUpdater<Record<string, ToolCallUi[]>>,
    ) {
      commit({
        ...state,
        toolCallsByConversation: resolveUpdater(
          state.toolCallsByConversation,
          updater,
        ),
      });
    },
    setReasoningByConversation(
      updater: StateUpdater<Record<string, string>>,
    ) {
      commit({
        ...state,
        reasoningByConversation: resolveUpdater(
          state.reasoningByConversation,
          updater,
        ),
      });
    },
    setTurnDiffsByConversation(
      updater: StateUpdater<Record<string, TurnDiffSummary[]>>,
    ) {
      commit({
        ...state,
        turnDiffsByConversation: resolveUpdater(
          state.turnDiffsByConversation,
          updater,
        ),
      });
    },
    setTaskPlansByConversation(
      updater: StateUpdater<Record<string, TaskPlan>>,
    ) {
      commit({
        ...state,
        taskPlansByConversation: resolveUpdater(
          state.taskPlansByConversation,
          updater,
        ),
      });
    },
    setCompactionsByConversation(
      updater: StateUpdater<Record<string, ConversationCompaction>>,
    ) {
      commit({
        ...state,
        compactionsByConversation: resolveUpdater(
          state.compactionsByConversation,
          updater,
        ),
      });
    },
    setUnviewedConversationIds(updater: StateUpdater<Set<string>>) {
      commit({
        ...state,
        unviewedConversationIds: resolveUpdater(
          state.unviewedConversationIds,
          updater,
        ),
      });
    },
    setPendingApprovals(
      updater: StateUpdater<Record<string, ToolCallUi>>,
    ) {
      commit({
        ...state,
        pendingApprovals: resolveUpdater(state.pendingApprovals, updater),
      });
    },
  };

  return store;
}
