import { ipc } from "../ipc.ts";
import { useEffect, useMemo, useRef, useState, type SetStateAction } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  appendImageFiles,
  imageAttachmentDataUrl,
  MAX_IMAGE_ATTACHMENTS_PER_MESSAGE,
  mergeImageAttachments,
  type ImageAttachmentData,
} from "../attachments";
import {
  appendDictation,
  createDictationRecognition,
  dictationTranscript,
  type DictationRecognition,
} from "../dictation";
import { matchCommands } from "../commandPalette";
import {
  commandDescriptionLabel,
  type CommandRegistryEntry,
} from "../commands";
import {
  acceptedNativeDropPaths,
  insertDroppedFileMentions,
  partitionDroppedPaths,
} from "../composerDrop";
import { terminalDropBridge } from "../terminalDrop";
import {
  eligibleCredentialProfiles,
} from "../credentialProfiles";
import {
  resolveComposerModel,
  visibleComposerModels,
} from "../claudeComposerModels";
import {
  composerModeAccessLabel,
  composerPrimaryAction,
  type ComposerInputRoute,
  type ComposerPrimaryAction,
} from "../composerPresentation";
import {
  exitShellModeDraft,
  resolveInputRoute,
} from "../shell";
import { chatModeFor, type ChatMode } from "../modes";
import { themePreferences, type ThemePreference } from "../theme";
import { useT, type Translate } from "../i18n/runtime";
import type {
  AgentBackend,
  ApprovalPolicy,
  ChatModel,
  ConversationSummary,
  CredentialProfile,
  ImageAttachment,
  Project,
  QueuedMessage,
  WorkflowMode,
} from "../types";

type ChatComposerProps = {
  draft: string;
  attachments: ImageAttachment[];
  commands: CommandRegistryEntry[];
  models: ChatModel[];
  directApiModelsAvailable: boolean;
  proMode: boolean;
  agentBackend: AgentBackend | null;
  selectedModel: string;
  resolvedModel: string | null;
  selectedEffort: string;
  fastMode: boolean;
  fastModeAvailable: boolean;
  currentProject: Project | null;
  workspaceId: string | null;
  agentMode: boolean;
  agentModeAvailable: boolean;
  agentModeStatus: string;
  approvalPolicy: ApprovalPolicy;
  workflowMode: WorkflowMode;
  proxyDown: boolean;
  sendPending: boolean;
  steerAvailable: boolean;
  cancelAndRevertPending: boolean;
  stopAndRevertAvailable: boolean;
  stopRequested: boolean;
  queuedMessages: QueuedMessage[];
  focusSignal: number;
  dictationSignal: number;
  dictationShortcut: string;
  mentionSeedPaths: string[];
  mentionSeedSignal: number;
  currentConversationId: string | null;
  credentialProfiles: CredentialProfile[];
  credentialProfilesLoaded: boolean;
  credentialProfileId: string | null;
  onDraftChange: (draft: string) => void;
  onAttachmentsChange: (next: SetStateAction<ImageAttachment[]>) => void;
  onModelChange: (model: string) => void;
  onEffortChange: (effort: string) => void;
  onFastModeChange: (enabled: boolean) => void;
  onOpenProjectSwitcher: () => void;
  onCommandPaletteOpen: () => void;
  onRunCommand: (command: CommandRegistryEntry, args: string) => Promise<void>;
  onModeChange: (mode: Exclude<ChatMode, "legacy_restricted">) => void;
  onWorkflowModeChange: (mode: WorkflowMode) => Promise<void>;
  onQueueMessage: (
    text: string,
    mentions?: string[],
    attachments?: ImageAttachment[],
  ) => void;
  onSteer: (text: string, mentions?: string[]) => Promise<void>;
  onRemoveQueued: (id: string) => void;
  onSend: (mentions?: string[], attachments?: ImageAttachment[]) => void;
  onCancel: () => void;
  onCancelAndRevert: () => void;
  onCredentialProfileChange: (
    credentialProfileId: string | null,
    conversation?: ConversationSummary,
  ) => void;
};

type SlashDraft = {
  commandQuery: string;
  commandName: string;
  args: string;
  hasArgs: boolean;
};

type PaletteItem =
  | { kind: "command"; command: CommandRegistryEntry }
  | {
      kind: "model" | "effort";
      command: CommandRegistryEntry;
      value: string;
      description: string;
    }
  | {
      kind: "theme";
      command: CommandRegistryEntry;
      value: ThemePreference;
      description: string;
    };

type FileMentionToken = {
  start: number;
  end: number;
  query: string;
  key: string;
};

type FileMention = {
  token: string;
  path: string;
};

type ProjectFileSearchResult = {
  path: string;
};

const defaultEfforts = ["low", "medium", "high", "xhigh"];

function composerModePresentationLabel(
  workflowMode: WorkflowMode,
  chatMode: ChatMode,
  t: Translate,
) {
  if (workflowMode === "plan") {
    return {
      label: t("Plan · read-only"),
      description: t("Explore and propose a plan without changing files."),
    };
  }
  switch (chatMode) {
    case "chat":
      return {
        label: t("Chat · no tools"),
        description: t("Answer without running project tools."),
      };
    case "sandbox":
      return {
        label: t("Agent · Auto"),
        description: t(
          "Work sandboxed in the workspace without approval prompts. Ask only when more access is needed.",
        ),
      };
    case "on_failure":
      return {
        label: t("Agent · On failure"),
        description: t("Work sandboxed and ask only when a command fails."),
      };
    case "ask":
      return {
        label: t("Agent · Ask first"),
        description: t("Use project tools and confirm write actions."),
      };
    case "full_access":
      return {
        label: t("Agent · Full access"),
        description: t("Use unrestricted machine access without approvals."),
      };
    case "legacy_restricted":
      return {
        label: t("Agent · Restricted"),
        description: t("Legacy policy; choose a current access mode."),
      };
  }
}

function composerModeAccessLabelTranslated(chatMode: ChatMode, t: Translate) {
  switch (composerModeAccessLabel(chatMode)) {
    case "Auto":
      return t("Auto");
    case "On failure":
      return t("On failure");
    case "Ask first":
      return t("Ask first");
    case "Full access":
      return t("Full access");
    case "Restricted (legacy)":
      return t("Restricted (legacy)");
    default:
      return null;
  }
}

function composerPlaceholderLabel(
  workflowMode: WorkflowMode,
  chatMode: ChatMode,
  projectName: string | null,
  t: Translate,
) {
  if (workflowMode === "plan") {
    return projectName
      ? t("Plan work in {project} — no changes will be made", {
          project: projectName,
        })
      : t("Describe what to plan — no changes will be made");
  }
  if (chatMode === "chat") {
    return projectName
      ? t("Ask about {project} — switch to Agent to use tools", {
          project: projectName,
        })
      : t("Ask anything — choose a project to use repository tools");
  }
  return projectName
    ? t("Describe a task for {project}", { project: projectName })
    : t("Choose a project before using Agent tools");
}

function shellModePresentationLabel(
  projectName: string | null,
  t: Translate,
) {
  return projectName
    ? {
        status: t("direct · no model"),
        placeholder: t("Run in {project} — model bypassed", {
          project: projectName,
        }),
      }
    : {
        status: t("select a project to run"),
        placeholder: t("Select a project before running a command"),
      };
}

function composerPrimaryActionLabel(
  action: ComposerPrimaryAction,
  t: Translate,
): ComposerPrimaryAction {
  return {
    ...action,
    label: translatePrimaryActionLabel(action.label, t),
    ariaLabel: translatePrimaryActionAriaLabel(action.ariaLabel, t),
    disabledReason: action.disabledReason
      ? translatePrimaryActionDisabledReason(action.disabledReason, t)
      : null,
  };
}

function translatePrimaryActionLabel(source: string, t: Translate) {
  switch (source) {
    case "Run":
      return t("Run");
    case "Run command":
      return t("Run command");
    case "Queue":
      return t("Queue");
    case "Stop":
      return t("Stop");
    case "Send":
      return t("Send");
    default:
      return source;
  }
}

function translatePrimaryActionAriaLabel(source: string, t: Translate) {
  switch (source) {
    case "Run slash command":
      return t("Run slash command");
    case "Run direct shell command":
      return t("Run direct shell command");
    case "Queue message after the current response":
      return t("Queue message after the current response");
    case "Stop response":
      return t("Stop response");
    case "Send message":
      return t("Send message");
    default:
      return source;
  }
}

function translatePrimaryActionDisabledReason(source: string, t: Translate) {
  switch (source) {
    case "Wait while Halv stops and reverts the current turn.":
      return t("Wait while Halv stops and reverts the current turn.");
    case "Remove image attachments before running a slash command.":
      return t("Remove image attachments before running a slash command.");
    case "Direct shell commands cannot include image attachments.":
      return t("Direct shell commands cannot include image attachments.");
    case "Type a command after !, or press Escape to leave shell mode.":
      return t("Type a command after !, or press Escape to leave shell mode.");
    case "Choose a project before running a direct shell command.":
      return t("Choose a project before running a direct shell command.");
    case "The proxy is unavailable. Run /restart-proxy or open Settings.":
      return t("The proxy is unavailable. Run /restart-proxy or open Settings.");
    case "Type a message or attach an image first.":
      return t("Type a message or attach an image first.");
    default:
      return source;
  }
}

function responseOptionsDisplayLabel(
  modelLabel: string,
  effort: string,
  fastMode: boolean,
  credentialProfileId: string | null,
  profiles: CredentialProfile[],
  t: Translate,
) {
  const base = fastMode
    ? t("{model} · {effort} · Fast", {
        model: modelLabel,
        effort: reasoningEffortLabel(effort, t),
      })
    : t("{model} · {effort}", {
        model: modelLabel,
        effort: reasoningEffortLabel(effort, t),
      });
  if (!credentialProfileId) {
    return base;
  }
  const profile = profiles.find((item) => item.id === credentialProfileId);
  return t("{options} · {profile}", {
    options: base,
    profile: profile?.name ?? t("Unknown profile"),
  });
}

function credentialProfilePinNoticeLabel(
  credentialProfileId: string | null,
  profiles: CredentialProfile[],
  t: Translate,
) {
  if (!credentialProfileId) {
    return null;
  }
  const profile = profiles.find((item) => item.id === credentialProfileId);
  if (!profile) {
    return t(
      "Pinned profile {profileId} is removed — requests will fail until you change it",
      { profileId: credentialProfileId },
    );
  }
  if (profile.status === "active") {
    return null;
  }
  return t(
    "Pinned profile {name} is {status} — requests will fail until you change it",
    {
      name: profile.name,
      status: credentialProfileStatusLabel(profile, true, t),
    },
  );
}

export function ChatComposer({
  draft,
  attachments,
  commands,
  models,
  directApiModelsAvailable,
  proMode,
  agentBackend,
  selectedModel,
  resolvedModel,
  selectedEffort,
  fastMode,
  fastModeAvailable,
  currentProject,
  workspaceId,
  agentMode,
  agentModeAvailable,
  agentModeStatus,
  approvalPolicy,
  workflowMode,
  proxyDown,
  sendPending,
  steerAvailable,
  cancelAndRevertPending,
  stopAndRevertAvailable,
  stopRequested,
  queuedMessages,
  focusSignal,
  dictationSignal,
  dictationShortcut,
  mentionSeedPaths,
  mentionSeedSignal,
  currentConversationId,
  credentialProfiles,
  credentialProfilesLoaded,
  credentialProfileId,
  onDraftChange,
  onAttachmentsChange,
  onModelChange,
  onEffortChange,
  onFastModeChange,
  onOpenProjectSwitcher,
  onCommandPaletteOpen,
  onRunCommand,
  onModeChange,
  onWorkflowModeChange,
  onQueueMessage,
  onSteer,
  onRemoveQueued,
  onSend,
  onCancel,
  onCancelAndRevert,
  onCredentialProfileChange,
}: ChatComposerProps) {
  const t = useT();
  const [selectedPaletteIndex, setSelectedPaletteIndex] = useState(0);
  const [slashDismissed, setSlashDismissed] = useState(false);
  const [dismissedDraft, setDismissedDraft] = useState<string | null>(null);
  const slashPaletteOpenRef = useRef(false);
  const sendPendingFlipAtRef = useRef(0);
  const prevSendPendingRef = useRef(sendPending);
  const [commandError, setCommandError] = useState<string | null>(null);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [dictationError, setDictationError] = useState<string | null>(null);
  const [steerError, setSteerError] = useState<string | null>(null);
  const [steerPending, setSteerPending] = useState(false);
  const [credentialProfileError, setCredentialProfileError] = useState<
    string | null
  >(null);
  const [credentialProfilePending, setCredentialProfilePending] =
    useState(false);
  const [dictationListening, setDictationListening] = useState(false);
  const dictationRef = useRef<DictationRecognition | null>(null);
  const dictationBaseRef = useRef("");
  const lastDictationSignalRef = useRef(dictationSignal);
  const [draggingImages, setDraggingImages] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const latestDraftRef = useRef(draft);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const composerDropZoneRef = useRef<HTMLDivElement>(null);
  const nativePathDropRef = useRef<(paths: string[]) => void>(() => undefined);
  const attachmentConversationIdRef = useRef(currentConversationId);
  const attachmentListRef = useRef(attachments);
  attachmentConversationIdRef.current = currentConversationId;
  attachmentListRef.current = attachments;
  const [cursorPosition, setCursorPosition] = useState(draft.length);
  const [fileMentionResults, setFileMentionResults] = useState<
    ProjectFileSearchResult[]
  >([]);
  const [fileMentionLoading, setFileMentionLoading] = useState(false);
  const [fileMentionError, setFileMentionError] = useState<string | null>(null);
  const [selectedMentionIndex, setSelectedMentionIndex] = useState(0);
  const [modeMenuOpen, setModeMenuOpen] = useState(false);
  const modeMenuRef = useRef<HTMLDivElement>(null);
  const lastAgentModeByConversationRef = useRef(
    new Map<string, Exclude<ChatMode, "chat" | "legacy_restricted">>(),
  );
  const [composerFocused, setComposerFocused] = useState(false);
  const [sendAttempted, setSendAttempted] = useState(false);
  const [responseMenuOpen, setResponseMenuOpen] = useState(false);
  const responseMenuRef = useRef<HTMLDivElement>(null);
  const [dismissedMentionKey, setDismissedMentionKey] = useState<string | null>(
    null,
  );
  const [fileMentions, setFileMentions] = useState<FileMention[]>([]);
  const fileMentionsRef = useRef<FileMention[]>([]);
  const fileMentionsByConversationRef = useRef(
    new Map<string, FileMention[]>(),
  );
  const mentionConversationIdRef = useRef(currentConversationId);
  fileMentionsRef.current = fileMentions;
  nativePathDropRef.current = (paths) => {
    void handleNativePathDrop(paths);
  };
  const visibleModels = useMemo(
    () =>
      visibleComposerModels(models, {
        proMode,
        agentBackend,
        directApiModelsAvailable,
      }),
    [agentBackend, directApiModelsAvailable, models, proMode],
  );
  const selectedModelInfo = resolveComposerModel(
    visibleModels,
    models,
    selectedModel,
  );
  const nativeModels = visibleModels.filter((model) => !model.agentBackend);
  const sessionModelGroups = Array.from(
    visibleModels
      .filter((model) => model.agentBackend)
      .reduce((groups, model) => {
        const label = model.modelGroup ?? t("Agent CLI");
        groups.set(label, [...(groups.get(label) ?? []), model]);
        return groups;
      }, new Map<string, ChatModel[]>()),
  );
  const selectedProvider = selectedModelInfo?.provider;
  const eligibleProfiles = eligibleCredentialProfiles(
    credentialProfiles,
    selectedProvider,
  );
  const pinnedProfile = credentialProfileId
    ? credentialProfiles.find((profile) => profile.id === credentialProfileId) ??
      null
    : null;
  const pinnedProfileIsEligible = eligibleProfiles.some(
    (profile) => profile.id === credentialProfileId,
  );
  const responseOptionsLabel = responseOptionsDisplayLabel(
    selectedModelInfo.label,
    selectedEffort,
    fastMode,
    credentialProfileId,
    credentialProfiles,
    t,
  );
  const credentialProfileNotice = credentialProfilesLoaded
    ? credentialProfilePinNoticeLabel(
        credentialProfileId,
        credentialProfiles,
        t,
      )
    : null;
  const chatMode = chatModeFor(agentMode, approvalPolicy);
  const conversationModeKey = currentConversationId ?? "new-thread";
  const accessLabel = composerModeAccessLabelTranslated(chatMode, t);
  const inputRoute = useMemo(
    () =>
      workflowMode === "plan"
        ? ({ kind: "chat" } as const)
        : resolveInputRoute(draft, currentProject?.id ?? null),
    [currentProject?.id, draft, workflowMode],
  );
  const shellMode = inputRoute.kind !== "chat";
  const shellPresentation = useMemo(
    () => shellModePresentationLabel(currentProject?.name ?? null, t),
    [currentProject?.name, t],
  );
  const slashDraft = useMemo(
    () => (shellMode ? null : parseSlashDraft(draft)),
    [draft, shellMode],
  );
  const slashActive = slashDraft !== null && !slashDismissed;
  const presentationRoute: ComposerInputRoute =
    inputRoute.kind === "blocked_shell"
      ? inputRoute.reason === "empty_command"
        ? "blocked-empty-shell"
        : "blocked-project-shell"
      : inputRoute.kind;
  const modePresentation = composerModePresentationLabel(
    workflowMode,
    chatMode,
    t,
  );
  const placeholder = shellMode
    ? shellPresentation.placeholder
    : composerPlaceholderLabel(
        workflowMode,
        chatMode,
        currentProject?.name ?? null,
        t,
      );
  const primaryAction = composerPrimaryActionLabel(
    composerPrimaryAction({
      attachmentCount: attachments.length,
      cancelAndRevertPending,
      hasText: draft.trim().length > 0,
      inputRoute: presentationRoute,
      proxyDown,
      sendPending,
      slashCommand: slashDraft !== null,
      steerAvailable,
    }),
    t,
  );
  const primaryDisabledReason =
    steerPending
      ? t("Halv is steering the current response.")
      : primaryAction.kind === "stop" && stopRequested
        ? t("Halv is stopping the current response.")
        : primaryAction.disabledReason;
  latestDraftRef.current = draft;
  const mentionToken = useMemo(
    () =>
      slashDraft || shellMode
        ? null
        : parseFileMentionToken(draft, cursorPosition),
    [cursorPosition, draft, shellMode, slashDraft],
  );
  const mentionActive =
    mentionToken !== null && dismissedMentionKey !== mentionToken.key;
  const paletteItems = useMemo(
    () =>
      buildPaletteItems(
        slashDraft,
        slashActive,
        commands,
        visibleModels,
        selectedModelInfo,
        t,
      ),
    [commands, selectedModelInfo, slashActive, slashDraft, t, visibleModels],
  );

  useEffect(() => {
    setSelectedPaletteIndex(0);
  }, [draft, paletteItems.length]);

  useEffect(() => {
    const opened = slashActive && !slashPaletteOpenRef.current;
    slashPaletteOpenRef.current = slashActive;
    if (opened) {
      onCommandPaletteOpen();
    }
  }, [onCommandPaletteOpen, slashActive]);

  useEffect(() => {
    setSelectedMentionIndex(0);
  }, [mentionToken?.key, fileMentionResults.length]);

  useEffect(() => {
    if (!slashDraft) {
      setSlashDismissed(false);
      setDismissedDraft(null);
      setCommandError(null);
    }
  }, [slashDraft]);

  useEffect(() => {
    if (slashDismissed && dismissedDraft !== null && draft !== dismissedDraft) {
      setSlashDismissed(false);
      setDismissedDraft(null);
    }
  }, [dismissedDraft, draft, slashDismissed]);

  useEffect(() => {
    if (dismissedMentionKey && mentionToken?.key !== dismissedMentionKey) {
      setDismissedMentionKey(null);
    }
  }, [dismissedMentionKey, mentionToken?.key]);

  useEffect(() => {
    const previousConversationId = mentionConversationIdRef.current;
    if (previousConversationId === currentConversationId) {
      return;
    }
    if (previousConversationId) {
      fileMentionsByConversationRef.current.set(
        previousConversationId,
        fileMentionsRef.current,
      );
    }
    mentionConversationIdRef.current = currentConversationId;
    const restoredMentions = currentConversationId
      ? (fileMentionsByConversationRef.current.get(currentConversationId) ??
        (previousConversationId === null ? fileMentionsRef.current : []))
      : [];
    const visibleMentions = restoredMentions.filter((mention) =>
      draft.includes(mention.token),
    );
    if (currentConversationId) {
      fileMentionsByConversationRef.current.set(
        currentConversationId,
        visibleMentions,
      );
    }
    setFileMentions(visibleMentions);
  }, [currentConversationId]);

  useEffect(() => {
    setFileMentions((current) =>
      current.filter((mention) => draft.includes(mention.token)),
    );
  }, [draft]);

  useEffect(() => {
    if (!mentionActive || !mentionToken) {
      setFileMentionResults([]);
      setFileMentionLoading(false);
      setFileMentionError(null);
      return;
    }
    if (!currentProject) {
      setFileMentionResults([]);
      setFileMentionLoading(false);
      setFileMentionError(null);
      return;
    }

    let cancelled = false;
    setFileMentionLoading(true);
    setFileMentionError(null);
    const timer = window.setTimeout(() => {
      ipc.project.searchFiles({
        projectId: currentProject.id,
        workspaceId,
        query: mentionToken.query,
      })
        .then((results) => {
          if (!cancelled) {
            setFileMentionResults(results);
          }
        })
        .catch((error) => {
          if (!cancelled) {
            setFileMentionResults([]);
            setFileMentionError(error instanceof Error ? error.message : String(error));
          }
        })
        .finally(() => {
          if (!cancelled) {
            setFileMentionLoading(false);
          }
        });
    }, 150);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [currentProject, mentionActive, mentionToken, workspaceId]);

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) {
      return;
    }

    const maxHeight = 8 * 24;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, maxHeight)}px`;
    textarea.style.overflowY = textarea.scrollHeight > maxHeight ? "auto" : "hidden";
  }, [draft]);

  useEffect(() => {
    if (prevSendPendingRef.current && !sendPending) {
      sendPendingFlipAtRef.current = Date.now();
    }
    prevSendPendingRef.current = sendPending;
  }, [sendPending]);

  useEffect(() => {
    if (focusSignal === 0) {
      return;
    }
    focusTextarea();
  }, [focusSignal]);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) {
      return;
    }

    let disposed = false;
    let unlisten: (() => void) | undefined;
    let nativeDragHasImages = false;
    try {
      void getCurrentWindow()
        .scaleFactor()
        .catch(() => window.devicePixelRatio || 1)
        .then((scaleFactor) =>
          getCurrentWebview().onDragDropEvent((event) => {
            const payload = event.payload;
            if (payload.type === "leave") {
              nativeDragHasImages = false;
              setDraggingImages(false);
              return;
            }

            const dropZone = composerDropZoneRef.current;
            const position = payload.position.toLogical(scaleFactor);
            const bounds = dropZone?.getBoundingClientRect();
            const inside = Boolean(
              bounds &&
                position.x >= bounds.left &&
                position.x <= bounds.right &&
                position.y >= bounds.top &&
                position.y <= bounds.bottom,
            );

            // A terminal pane under the pointer takes the drop instead.
            const overTerminal =
              !inside && terminalDropBridge.targetAt(position.x, position.y) !== null;

            if (payload.type === "enter") {
              nativeDragHasImages =
                partitionDroppedPaths(payload.paths).imagePaths.length > 0;
              setDraggingImages(nativeDragHasImages && !overTerminal);
              return;
            }
            if (payload.type === "drop") {
              const acceptedPaths = overTerminal
                ? []
                : acceptedNativeDropPaths(payload.paths, inside);
              nativeDragHasImages = false;
              setDraggingImages(false);
              if (acceptedPaths.length > 0) {
                nativePathDropRef.current(acceptedPaths);
              }
              return;
            }
            setDraggingImages(nativeDragHasImages && !overTerminal);
          }),
        )
        .then((stopListening) => {
          if (disposed) {
            stopListening();
          } else {
            unlisten = stopListening;
          }
        })
        .catch(() => {
          setDraggingImages(false);
        });
    } catch {
      setDraggingImages(false);
    }

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (
      dictationSignal === 0 ||
      dictationSignal === lastDictationSignalRef.current
    ) {
      return;
    }
    lastDictationSignalRef.current = dictationSignal;
    toggleDictation();
  }, [dictationSignal]);

  useEffect(
    () => () => {
      dictationRef.current?.abort();
      dictationRef.current = null;
    },
    [],
  );

  useEffect(() => {
    if (!modeMenuOpen && !responseMenuOpen) {
      return;
    }
    function closeComposerMenus(event: PointerEvent) {
      if (
        event.target instanceof Node &&
        !modeMenuRef.current?.contains(event.target) &&
        !responseMenuRef.current?.contains(event.target)
      ) {
        setModeMenuOpen(false);
        setResponseMenuOpen(false);
      }
    }
    function closeComposerMenusOnEscape(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setModeMenuOpen(false);
        setResponseMenuOpen(false);
      }
    }
    document.addEventListener("pointerdown", closeComposerMenus);
    document.addEventListener("keydown", closeComposerMenusOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeComposerMenus);
      document.removeEventListener("keydown", closeComposerMenusOnEscape);
    };
  }, [modeMenuOpen, responseMenuOpen]);

  useEffect(() => {
    setModeMenuOpen(false);
  }, [chatMode, sendPending, workflowMode]);

  useEffect(() => {
    if (chatMode !== "chat" && chatMode !== "legacy_restricted") {
      lastAgentModeByConversationRef.current.set(conversationModeKey, chatMode);
    }
  }, [chatMode, conversationModeKey]);

  useEffect(() => {
    setSendAttempted(false);
  }, [attachments.length, draft]);

  useEffect(() => {
    setCredentialProfileError(null);
  }, [currentConversationId]);

  useEffect(() => {
    if (shellMode) {
      setResponseMenuOpen(false);
    }
  }, [shellMode]);

  useEffect(() => {
    if (mentionSeedSignal === 0) {
      return;
    }
    setFileMentions(
      mentionSeedPaths.map((path) => ({
        token: `@{${path}}`,
        path,
      })),
    );
  }, [mentionSeedPaths, mentionSeedSignal]);

  function focusTextarea() {
    window.requestAnimationFrame(() => textareaRef.current?.focus());
  }

  function stopDictation() {
    dictationRef.current?.stop();
  }

  function toggleDictation() {
    if (dictationRef.current) {
      stopDictation();
      return;
    }
    if (shellMode) {
      setDictationError(
        t("Dictation is available for chat prompts, not direct shell commands."),
      );
      return;
    }
    const recognition = createDictationRecognition();
    if (!recognition) {
      setDictationError(
        t(
          "Speech dictation is unavailable in this webview. You can still use system dictation in the message field.",
        ),
      );
      return;
    }
    dictationBaseRef.current = draft;
    recognition.continuous = true;
    recognition.interimResults = true;
    recognition.lang = navigator.language || "en-US";
    recognition.onresult = (event) => {
      const { finalText, interimText } = dictationTranscript(event.results);
      onDraftChange(
        appendDictation(dictationBaseRef.current, finalText, interimText),
      );
    };
    recognition.onerror = (event) => {
      if (dictationRef.current !== recognition) {
        return;
      }
      dictationRef.current = null;
      setDictationListening(false);
      try {
        recognition.abort();
      } catch {}
      if (event.error !== "aborted") {
        setDictationError(
          event.error === "not-allowed"
            ? t(
                "Microphone access was denied. Allow microphone access for Halv in system settings.",
              )
            : t("Dictation stopped: {message}", {
                message: event.message || event.error,
              }),
        );
      }
    };
    recognition.onend = () => {
      if (dictationRef.current === recognition) {
        dictationRef.current = null;
        setDictationListening(false);
      }
    };
    dictationRef.current = recognition;
    setDictationError(null);
    setDictationListening(true);
    focusTextarea();
    try {
      recognition.start();
    } catch (error) {
      dictationRef.current = null;
      setDictationListening(false);
      setDictationError(
        error instanceof Error
          ? error.message
          : t("Could not start dictation."),
      );
    }
  }

  function currentMentionPaths() {
    return fileMentions
      .filter((mention) => draft.includes(mention.token))
      .map((mention) => mention.path);
  }

  function updateCursorFromTextarea(textarea: HTMLTextAreaElement) {
    setCursorPosition(textarea.selectionStart ?? textarea.value.length);
  }

  function handleDraftChange(event: React.ChangeEvent<HTMLTextAreaElement>) {
    if (dictationRef.current) {
      dictationRef.current.stop();
    }
    onDraftChange(event.target.value);
    updateCursorFromTextarea(event.target);
  }

  function handleTextareaSelect(event: React.SyntheticEvent<HTMLTextAreaElement>) {
    updateCursorFromTextarea(event.currentTarget);
  }

  function selectFileMention(result: ProjectFileSearchResult) {
    if (!mentionToken) {
      return;
    }
    const token = `@{${result.path}}`;
    const nextDraft = `${draft.slice(0, mentionToken.start)}${token} ${draft.slice(
      mentionToken.end,
    )}`;
    const nextCursor = mentionToken.start + token.length + 1;
    onDraftChange(nextDraft);
    setCursorPosition(nextCursor);
    setFileMentions((current) => [
      ...current.filter((mention) => mention.path !== result.path),
      { token, path: result.path },
    ]);
    window.requestAnimationFrame(() => {
      const textarea = textareaRef.current;
      if (!textarea) {
        return;
      }
      textarea.focus();
      textarea.setSelectionRange(nextCursor, nextCursor);
    });
  }

  async function selectComposerMode(
    nextWorkflowMode: WorkflowMode,
    nextChatMode?: Exclude<ChatMode, "legacy_restricted">,
  ) {
    setModeMenuOpen(false);
    if (workflowMode !== nextWorkflowMode) {
      await onWorkflowModeChange(nextWorkflowMode);
    }
    if (nextChatMode && chatMode !== nextChatMode) {
      onModeChange(nextChatMode);
    }
    focusTextarea();
  }

  function selectAgentSegment() {
    if (!currentProject || !agentModeAvailable || sendPending) {
      return;
    }
    const effectiveMode = chatModeFor(true, approvalPolicy);
    const fallbackMode =
      effectiveMode === "legacy_restricted" ? "ask" : effectiveMode;
    const nextMode =
      lastAgentModeByConversationRef.current.get(conversationModeKey) ??
      fallbackMode;
    void selectComposerMode("default", nextMode);
  }

  async function executeCommand(command: CommandRegistryEntry, args: string) {
    try {
      await onRunCommand(command, args);
      onDraftChange("");
      setCommandError(null);
      setSlashDismissed(false);
      setDismissedDraft(null);
      focusTextarea();
    } catch (error) {
      setCommandError(error instanceof Error ? error.message : String(error));
    }
  }

  async function executePaletteItem(item: PaletteItem) {
    if (item.kind === "command") {
      if (item.command.argsHint && !slashDraft?.hasArgs) {
        onDraftChange(`/${item.command.name} `);
        focusTextarea();
        return;
      }
      await executeCommand(
        item.command,
        slashDraft?.commandName === item.command.name ? slashDraft.args : "",
      );
      return;
    }

    await executeCommand(item.command, item.value);
  }

  async function executeDraftCommand() {
    if (!slashDraft) {
      return;
    }

    const selectedItem = paletteItems[selectedPaletteIndex];
    if (selectedItem && selectedItem.kind !== "command") {
      await executePaletteItem(selectedItem);
      return;
    }

    const command = commands.find(
      (item) =>
        item.name.toLowerCase() === slashDraft.commandName.toLowerCase(),
    );
    if (command) {
      await executeCommand(command, slashDraft.args);
      return;
    }

    if (selectedItem) {
      await executePaletteItem(selectedItem);
      return;
    }

    setCommandError(
      t("Unknown command: /{command}", {
        command: slashDraft.commandName,
      }),
    );
  }

  async function addImageFiles(files: Iterable<File>) {
    if (shellMode) {
      setAttachmentError(
        t(
          "Image attachments are only available for chat messages, not shell commands.",
        ),
      );
      return;
    }
    const conversationIdAtStart = attachmentConversationIdRef.current;
    const attachmentsAtStart = attachmentListRef.current;
    try {
      const next = await appendImageFiles(attachmentsAtStart, files);
      if (
        attachmentConversationIdRef.current !== conversationIdAtStart ||
        attachmentListRef.current !== attachmentsAtStart
      ) {
        return;
      }
      attachmentListRef.current = next;
      onAttachmentsChange(next);
      setAttachmentError(null);
    } catch (error) {
      setAttachmentError(
        error instanceof Error
          ? error.message
          : t("Could not attach that image."),
      );
    }
  }

  function removeImageAttachment(id: string) {
    const next = attachmentListRef.current.filter(
      (attachment) => attachment.id !== id,
    );
    attachmentListRef.current = next;
    onAttachmentsChange(next);
    setAttachmentError(null);
  }

  function handlePaste(event: React.ClipboardEvent<HTMLTextAreaElement>) {
    const images = Array.from(event.clipboardData.items)
      .filter((item) => item.kind === "file" && item.type.startsWith("image/"))
      .map((item) => item.getAsFile())
      .filter((file): file is File => file !== null);
    if (images.length === 0) {
      return;
    }
    event.preventDefault();
    void addImageFiles(images);
  }

  function handleImageDrop(event: React.DragEvent<HTMLDivElement>) {
    event.preventDefault();
    setDraggingImages(false);
    if (event.dataTransfer.files.length > 0) {
      void addImageFiles(event.dataTransfer.files);
    }
  }

  async function handleNativePathDrop(paths: string[]) {
    const { imagePaths, mentionPaths } = partitionDroppedPaths(paths);
    if (mentionPaths.length > 0) {
      insertNativePathMentions(mentionPaths);
    }
    if (imagePaths.length === 0) {
      return;
    }
    if (shellMode) {
      setAttachmentError(
        t(
          "Image attachments are only available for chat messages, not shell commands.",
        ),
      );
      return;
    }

    const conversationIdAtStart = attachmentConversationIdRef.current;
    const attachmentsAtStart = attachmentListRef.current;
    let next = attachmentsAtStart;
    const additions: ImageAttachmentData[] = [];
    let firstError: string | null = null;
    for (let index = 0; index < imagePaths.length; index += 1) {
      if (next.length >= MAX_IMAGE_ATTACHMENTS_PER_MESSAGE) {
        firstError ??= t("Attach at most {count} images to one message.", {
          count: MAX_IMAGE_ATTACHMENTS_PER_MESSAGE,
        });
        break;
      }
      try {
        const image = await ipc.composer.readDroppedImage({ path: imagePaths[index] });
        const merged = mergeImageAttachments(next, [image]);
        if (merged.firstError) {
          firstError ??= merged.firstError;
        } else {
          next = merged.attachments;
          additions.push(image);
        }
      } catch (error) {
        firstError ??= error instanceof Error ? error.message : String(error);
      }
    }
    if (attachmentConversationIdRef.current !== conversationIdAtStart) {
      if (firstError) {
        setAttachmentError(firstError);
      }
      return;
    }
    if (attachmentListRef.current !== attachmentsAtStart) {
      const merged = mergeImageAttachments(
        attachmentListRef.current,
        additions,
      );
      next = merged.attachments;
      firstError ??= merged.firstError;
    }
    if (next !== attachmentListRef.current) {
      attachmentListRef.current = next;
      onAttachmentsChange(next);
    }
    setAttachmentError(firstError);
  }

  function insertNativePathMentions(paths: string[]) {
    const insertion = insertDroppedFileMentions(
      draft,
      cursorPosition,
      paths,
      currentProject?.rootPath ?? null,
    );
    if (insertion.mentions.length === 0) {
      return;
    }
    onDraftChange(insertion.draft);
    setCursorPosition(insertion.cursor);
    setFileMentions((current) => [
      ...current.filter(
        (mention) =>
          !insertion.mentions.some((inserted) => inserted.path === mention.path),
      ),
      ...insertion.mentions,
    ]);
    window.requestAnimationFrame(() => {
      const textarea = textareaRef.current;
      if (!textarea) {
        return;
      }
      textarea.focus();
      textarea.setSelectionRange(insertion.cursor, insertion.cursor);
    });
  }

  async function submitDraft() {
    if (primaryDisabledReason) {
      setSendAttempted(true);
      return;
    }
    if (slashDraft) {
      if (attachments.length > 0) {
        setAttachmentError(
          t("Remove image attachments before running a slash command."),
        );
        return;
      }
      await executeDraftCommand();
      return;
    }

    if (inputRoute.kind === "blocked_shell") {
      return;
    }

    if (inputRoute.kind === "shell" && attachments.length > 0) {
      setAttachmentError(
        t(
          "Image attachments are only available for chat messages, not shell commands.",
        ),
      );
      return;
    }

    const trimmed = draft.trim();
    if (sendPending && primaryAction.kind === "steer") {
      const submittedDraft = draft;
      setSteerError(null);
      setSteerPending(true);
      try {
        await onSteer(trimmed, currentMentionPaths());
        const latestDraft = latestDraftRef.current;
        if (latestDraft === submittedDraft) {
          onDraftChange("");
        } else if (latestDraft.startsWith(submittedDraft)) {
          onDraftChange(latestDraft.slice(submittedDraft.length));
        }
        setFileMentions([]);
      } catch (error) {
        setSteerError(errorMessage(error));
      } finally {
        setSteerPending(false);
      }
      return;
    }
    if (sendPending && (trimmed || attachments.length > 0)) {
      onQueueMessage(trimmed, currentMentionPaths(), attachments);
      onDraftChange("");
      attachmentListRef.current = [];
      onAttachmentsChange([]);
      setFileMentions([]);
      setAttachmentError(null);
      return;
    }

    onSend(currentMentionPaths(), attachments);
    attachmentListRef.current = [];
    setFileMentions([]);
  }

  function handleSubmit(event: React.FormEvent) {
    event.preventDefault();
    // A click aimed at Stop can land after the stream ends and the button
    // has flipped back to Send — swallow submits in that grace window.
    if (Date.now() - sendPendingFlipAtRef.current < 300) {
      return;
    }
    void submitDraft();
  }

  function handleKeyDown(event: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Tab" && event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      if (!sendPending && !shellMode) {
        void onWorkflowModeChange(
          workflowMode === "plan" ? "default" : "plan",
        );
      }
      return;
    }

    if (event.key === "Escape") {
      if (shellMode) {
        event.preventDefault();
        onDraftChange(exitShellModeDraft(draft));
        focusTextarea();
        return;
      }
      if (mentionActive && mentionToken) {
        event.preventDefault();
        setDismissedMentionKey(mentionToken.key);
        return;
      }
      if (slashDraft && !slashDismissed) {
        event.preventDefault();
        setSlashDismissed(true);
        setDismissedDraft(draft);
      }
      return;
    }

    if (mentionActive && event.key === "ArrowDown") {
      event.preventDefault();
      setSelectedMentionIndex((current) =>
        fileMentionResults.length === 0
          ? 0
          : (current + 1) % fileMentionResults.length,
      );
      return;
    }

    if (mentionActive && event.key === "ArrowUp") {
      event.preventDefault();
      setSelectedMentionIndex((current) =>
        fileMentionResults.length === 0
          ? 0
          : (current - 1 + fileMentionResults.length) % fileMentionResults.length,
      );
      return;
    }

    if (slashActive && event.key === "ArrowDown") {
      event.preventDefault();
      setSelectedPaletteIndex((current) =>
        paletteItems.length === 0 ? 0 : (current + 1) % paletteItems.length,
      );
      return;
    }

    if (slashActive && event.key === "ArrowUp") {
      event.preventDefault();
      setSelectedPaletteIndex((current) =>
        paletteItems.length === 0
          ? 0
          : (current - 1 + paletteItems.length) % paletteItems.length,
      );
      return;
    }

    if (
      event.key === "Enter" &&
      !event.shiftKey &&
      !event.nativeEvent.isComposing &&
      event.keyCode !== 229
    ) {
      event.preventDefault();
      if (mentionActive) {
        const selectedResult = fileMentionResults[selectedMentionIndex];
        if (selectedResult) {
          selectFileMention(selectedResult);
        }
        return;
      }
      void submitDraft();
    }
  }

  async function changeCredentialProfile(nextId: string | null) {
    setCredentialProfileError(null);
    if (!currentConversationId) {
      onCredentialProfileChange(nextId);
      return;
    }
    setCredentialProfilePending(true);
    try {
      const conversation = await ipc.storage.setConversationCredentialProfile({
          conversationId: currentConversationId,
          credentialProfileId: nextId,
        });
      onCredentialProfileChange(nextId, conversation);
    } catch (error) {
      setCredentialProfileError(errorMessage(error));
    } finally {
      setCredentialProfilePending(false);
    }
  }

  return (
    <form
      className="flex-none bg-surface-base px-5 pb-5 pt-2"
      onSubmit={handleSubmit}
    >
      {/* The update card moves above this box, and it hides under the
          menus that open from it (see AppUpdateCard). */}
      <div
        className="relative mx-auto flex max-w-5xl flex-col gap-2"
        data-update-card-avoid=""
      >
        {slashActive ? (
          <CommandPalette
            items={paletteItems}
            onSelect={(item) => void executePaletteItem(item)}
            selectedIndex={selectedPaletteIndex}
            slashDraft={slashDraft}
          />
        ) : null}

        {mentionActive ? (
          <FileMentionPalette
            currentProject={currentProject}
            error={fileMentionError}
            loading={fileMentionLoading}
            onSelect={selectFileMention}
            results={fileMentionResults}
            selectedIndex={selectedMentionIndex}
          />
        ) : null}

        {queuedMessages.length > 0 ? (
          <div className="space-y-1.5">
            <div className="space-y-1.5">
              {queuedMessages.map((message) => (
                <div
                  className="flex items-center gap-2 rounded-lg border border-surface-border/40 bg-surface-raised/50 px-3 py-2 text-xs text-content-secondary"
                  key={message.id}
                >
                  <span className="shrink-0 rounded-full bg-accent/10 px-2 py-0.5 font-mono text-[10px] font-semibold uppercase tracking-[0.08em] text-accent">
                    {t("Queued")}
                  </span>
                  <span className="min-w-0 flex-1 truncate" title={message.text}>
                    {message.text || t("Image message")}
                  </span>
                  {message.text && message.attachments?.length ? (
                    <span className="shrink-0 text-content-muted">
                      {message.attachments.length === 1
                        ? t("{count} image", {
                            count: message.attachments.length,
                          })
                        : t("{count} images", {
                            count: message.attachments.length,
                          })}
                    </span>
                  ) : null}
                  <button
                    aria-label={t("Remove queued message")}
                    className="shrink-0 rounded-full px-1.5 text-content-muted transition-colors duration-150 hover:bg-surface-panel hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
                    onClick={() => onRemoveQueued(message.id)}
                    type="button"
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
            <div className="px-1 text-[11px] text-content-muted">
              {t(
                "Sends after the current response. Press Esc to stop and continue with the next item.",
              )}
            </div>
          </div>
        ) : null}

        {shellMode ? (
          <div
            className={`flex w-fit items-center gap-2 rounded-full border px-3 py-1 font-mono text-[11px] ${
              currentProject
                ? "border-accent/55 bg-accent/10 text-accent"
                : "border-danger/40 bg-danger/10 text-danger"
            }`}
            role="status"
          >
            <span className="font-semibold">!</span>
            <span className="font-semibold uppercase tracking-[0.12em]">
              {t("Shell mode")}
            </span>
            <span className="opacity-80">{shellPresentation.status}</span>
            <span className="opacity-60">{t("Esc exits")}</span>
          </div>
        ) : null}

        <div
          ref={composerDropZoneRef}
          className={`rounded-xl border bg-surface-raised/75 shadow-sm shadow-[rgb(var(--shadow-color)/0.06)] transition-colors focus-within:border-accent/60 focus-within:ring-2 focus-within:ring-accent/15 ${
            draggingImages
              ? "border-accent ring-2 ring-accent/20"
              : "border-surface-border/30"
          }`}
          onDragEnter={(event) => {
            if (event.dataTransfer.types.includes("Files")) {
              event.preventDefault();
              setDraggingImages(true);
            }
          }}
          onDragLeave={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
              setDraggingImages(false);
            }
          }}
          onDragOver={(event) => {
            if (event.dataTransfer.types.includes("Files")) {
              event.preventDefault();
              event.dataTransfer.dropEffect = "copy";
            }
          }}
          onDrop={handleImageDrop}
          onBlurCapture={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
              setComposerFocused(false);
            }
          }}
          onFocusCapture={() => setComposerFocused(true)}
        >
          {attachments.length > 0 ? (
            <div className="flex flex-wrap gap-2 px-3 pt-3">
              {attachments.map((attachment) => (
                <div
                  className="group/image flex min-w-0 max-w-64 items-center gap-2 rounded-lg border border-surface-border/40 bg-surface-sunken p-1.5 pr-2"
                  key={attachment.id}
                >
                  <img
                    alt={attachment.name}
                    className="h-12 w-12 shrink-0 rounded-md object-cover"
                    src={imageAttachmentDataUrl(attachment)}
                  />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-xs font-medium text-content-secondary">
                      {attachment.name}
                    </span>
                    <span className="block font-mono text-[10px] text-content-muted">
                      {t("{size} KB", {
                        size: Math.max(
                          1,
                          Math.round(attachment.sizeBytes / 1024),
                        ),
                      })}
                    </span>
                  </span>
                  <button
                    aria-label={t("Remove {name}", {
                      name: attachment.name,
                    })}
                    className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-base text-content-muted transition-colors hover:bg-danger/10 hover:text-danger focus:outline-none focus:ring-2 focus:ring-accent/40"
                    onClick={() => removeImageAttachment(attachment.id)}
                    type="button"
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
          ) : null}
          <div className="relative">
            {shellMode ? (
              <span className="pointer-events-none absolute left-4 top-3 font-mono text-sm font-semibold leading-6 text-accent">
                $
              </span>
            ) : null}
            <textarea
              aria-label={
                shellMode ? t("Direct shell command") : t("Message")
              }
              className={`max-h-48 min-h-14 w-full resize-none border-0 bg-transparent pb-2 pt-3 text-sm leading-6 text-content-primary outline-none placeholder:text-content-muted focus:ring-0 ${
                shellMode
                  ? "pl-9 pr-4 font-mono"
                  : "px-4"
              }`}
              onChange={handleDraftChange}
              onKeyDown={handleKeyDown}
              onPaste={handlePaste}
              onSelect={handleTextareaSelect}
              placeholder={placeholder}
              ref={textareaRef}
              rows={1}
              value={draft}
            />
          </div>
          {(commandError ||
            attachmentError ||
            dictationError ||
            steerError ||
            credentialProfileError) ? (
            <div className="space-y-1.5 px-3 pb-1">
              {commandError ? (
                <ComposerErrorNotice
                  message={commandError}
                  onDismiss={() => setCommandError(null)}
                />
              ) : null}
              {attachmentError ? (
                <ComposerErrorNotice
                  message={attachmentError}
                  onDismiss={() => setAttachmentError(null)}
                />
              ) : null}
              {dictationError ? (
                <ComposerErrorNotice
                  message={dictationError}
                  onDismiss={() => setDictationError(null)}
                />
              ) : null}
              {steerError ? (
                <ComposerErrorNotice
                  message={steerError}
                  onDismiss={() => setSteerError(null)}
                />
              ) : null}
              {credentialProfileError ? (
                <ComposerErrorNotice
                  message={credentialProfileError}
                  onDismiss={() => setCredentialProfileError(null)}
                />
              ) : null}
            </div>
          ) : null}
          {credentialProfileNotice ? (
            <p
              className="mx-3 mb-1 rounded-md border border-warning/30 bg-warning-surface/45 px-2.5 py-1.5 text-[11px] text-warning"
              role="status"
            >
              {credentialProfileNotice}
            </p>
          ) : null}

          <div className="flex flex-wrap items-center gap-2 border-t border-surface-border/20 px-3 py-2.5">
            <input
              accept="image/png,image/jpeg,image/webp"
              className="hidden"
              multiple
              onChange={(event) => {
                if (event.target.files?.length) {
                  void addImageFiles(event.target.files);
                }
                event.target.value = "";
              }}
              ref={fileInputRef}
              type="file"
            />
            <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
              <button
                aria-label={t("Attach PNG, JPEG, or WebP images")}
                className="rounded-md px-2 py-1.5 text-xs font-medium text-content-muted transition-colors hover:bg-surface-sunken hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
                disabled={shellMode}
                onClick={() => fileInputRef.current?.click()}
                title={
                  shellMode
                    ? t("Attachments are unavailable in direct shell mode")
                    : t("Attach PNG, JPEG, or WebP images")
                }
                type="button"
              >
                <span aria-hidden>＋</span> {t("Attach")}
              </button>
              <button
                aria-label={
                  dictationListening
                    ? t("Stop dictation")
                    : t("Start dictation")
                }
                aria-pressed={dictationListening}
                className={`rounded-md px-2 py-1.5 text-xs font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-40 ${
                  dictationListening
                    ? "bg-danger/10 text-danger"
                    : "text-content-muted hover:bg-surface-sunken hover:text-content-primary"
                }`}
                disabled={shellMode}
                onClick={toggleDictation}
                title={
                  shellMode
                    ? t("Dictation is unavailable in direct shell mode")
                    : t("Dictate prompt · {shortcut}", {
                        shortcut: dictationShortcut,
                      })
                }
                type="button"
              >
                {dictationListening ? t("■ Listening") : t("◉ Dictate")}
              </button>

              <button
                aria-label={
                  currentProject
                    ? t("Project: {name}. Choose project", {
                        name: currentProject.name,
                      })
                    : t("Project: none. Choose project")
                }
                className={`flex max-w-48 items-center gap-1.5 rounded-md border px-2 py-1.5 text-xs transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 ${
                  currentProject
                    ? "border-accent/25 bg-accent/10 text-accent"
                    : "border-surface-border/35 bg-surface-sunken text-content-muted"
                }`}
                onClick={onOpenProjectSwitcher}
                title={
                  currentProject?.rootPath ??
                  t("Choose a project for repository tools")
                }
                type="button"
              >
                <span className="shrink-0 font-mono text-[9px] uppercase tracking-[0.1em]">
                  {t("Project")}
                </span>
                <span className="min-w-0 truncate font-semibold">
                  {currentProject?.name ?? t("Choose")}
                </span>
              </button>

              {!shellMode ? (
                workflowMode === "plan" ? (
                  <div className="relative" ref={modeMenuRef}>
                    <button
                      aria-expanded={modeMenuOpen}
                      aria-haspopup="listbox"
                      aria-label={t("Workflow and access: {label}", {
                        label: modePresentation.label,
                      })}
                      className="inline-flex max-w-52 items-center gap-1.5 rounded-md border border-warning/30 bg-warning-surface/55 px-2 py-1.5 text-xs font-semibold text-warning outline-none transition-colors focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                      disabled={sendPending}
                      onClick={() => {
                        setResponseMenuOpen(false);
                        setModeMenuOpen((open) => !open);
                      }}
                      title={t(
                        "{description} Shift+Tab toggles Plan mode.",
                        { description: modePresentation.description },
                      )}
                      type="button"
                    >
                      <span className="truncate">{modePresentation.label}</span>
                      <span aria-hidden className="text-[9px]">▾</span>
                    </button>
                    {modeMenuOpen ? (
                      <div
                        aria-label={t("Workflow and access: {label}", {
                          label: modePresentation.label,
                        })}
                        className="absolute bottom-full left-0 z-30 mb-2 w-80 max-w-[calc(100vw-3rem)] overflow-hidden rounded-xl border border-surface-border/35 bg-surface-panel p-1.5 text-xs text-content-secondary shadow-xl shadow-[rgb(var(--shadow-color)/0.24)]"
                        role="listbox"
                      >
                        <button
                          aria-selected="true"
                          className="flex w-full items-start gap-2 rounded-lg bg-warning-surface/55 px-2.5 py-2 text-left text-warning outline-none focus:bg-surface-raised"
                          onClick={() => setModeMenuOpen(false)}
                          role="option"
                          type="button"
                        >
                          <span className="w-3 pt-0.5 text-center" aria-hidden>✓</span>
                          <span>
                            <span className="block font-semibold">{t("Plan · read-only")}</span>
                            <span className="mt-0.5 block text-[11px] text-content-muted">
                              {t("Explore and propose a plan without changing files.")}
                            </span>
                          </span>
                        </button>
                        <div className="my-1 border-t border-surface-border/25" />
                        {(["chat", "sandbox", "on_failure", "ask", "full_access"] as const).map((mode) => {
                          const option = composerModePresentationLabel("default", mode, t);
                          const disabled = mode !== "chat" && (!currentProject || !agentModeAvailable);
                          const disabledReason = !currentProject
                            ? t("Choose a project before enabling Agent tools.")
                            : !agentModeAvailable
                              ? agentModeStatus
                              : null;
                          return (
                            <button
                              aria-selected="false"
                              className="flex w-full items-start gap-2 rounded-lg px-2.5 py-2 text-left outline-none transition-colors hover:bg-surface-raised hover:text-content-primary focus:bg-surface-raised disabled:cursor-not-allowed disabled:opacity-45"
                              disabled={disabled}
                              key={mode}
                              onClick={() => void selectComposerMode("default", mode)}
                              role="option"
                              title={disabled ? disabledReason ?? undefined : option.description}
                              type="button"
                            >
                              <span className="w-3 pt-0.5 text-center" aria-hidden />
                              <span>
                                <span className="block font-semibold">{option.label}</span>
                                <span className="mt-0.5 block text-[11px] text-content-muted">
                                  {disabled ? disabledReason : option.description}
                                </span>
                              </span>
                            </button>
                          );
                        })}
                      </div>
                    ) : null}
                  </div>
                ) : (
                  <div className="flex items-center gap-1.5" ref={modeMenuRef}>
                    <div
                      aria-label={t("Chat mode")}
                      className="flex rounded-lg border border-surface-border/20 bg-surface-sunken p-0.5"
                      role="radiogroup"
                    >
                      <button
                        aria-checked={chatMode === "chat"}
                        aria-label={t("Chat")}
                        className={`rounded-md px-2 py-1 text-xs outline-none transition-colors focus:ring-2 focus:ring-accent/30 ${
                          chatMode === "chat"
                            ? "bg-surface-raised font-semibold text-accent"
                            : "text-content-muted"
                        }`}
                        disabled={sendPending}
                        onClick={() => void selectComposerMode("default", "chat")}
                        role="radio"
                        type="button"
                      >
                        {t("Chat")}
                      </button>
                      <button
                        aria-checked={chatMode !== "chat"}
                        aria-label={t("Agent")}
                        className={`rounded-md px-2 py-1 text-xs outline-none transition-colors focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-45 ${
                          chatMode !== "chat"
                            ? "bg-surface-raised font-semibold text-accent"
                            : "text-content-muted"
                        }`}
                        disabled={!currentProject || !agentModeAvailable || sendPending}
                        onClick={selectAgentSegment}
                        role="radio"
                        title={
                          !currentProject
                            ? t("Choose a project before enabling Agent tools.")
                            : !agentModeAvailable
                              ? agentModeStatus
                              : undefined
                        }
                        type="button"
                      >
                        {t("Agent")}
                      </button>
                    </div>
                    {chatMode !== "chat" && accessLabel ? (
                      <div className="relative">
                        <button
                          aria-expanded={modeMenuOpen}
                          aria-haspopup="listbox"
                          aria-label={t("Agent access: {access}", {
                            access: accessLabel,
                          })}
                          className={`inline-flex items-center gap-1.5 rounded-md border px-2 py-1.5 text-xs font-semibold outline-none transition-colors focus:ring-2 focus:ring-accent/30 ${
                            chatMode === "full_access"
                              ? "border-danger/25 bg-danger/10 text-danger"
                              : chatMode === "legacy_restricted"
                                ? "border-warning/30 bg-warning-surface/55 text-warning"
                                : "border-accent/25 bg-accent/10 text-accent"
                          }`}
                          disabled={sendPending}
                          onClick={() => {
                            setResponseMenuOpen(false);
                            setModeMenuOpen((open) => !open);
                          }}
                          type="button"
                        >
                          <span>{accessLabel}</span>
                          <span aria-hidden className="text-[9px]">▾</span>
                        </button>
                        {modeMenuOpen ? (
                          <div
                            aria-label={t("Agent access")}
                            className="absolute bottom-full left-0 z-30 mb-2 w-80 max-w-[calc(100vw-3rem)] overflow-hidden rounded-xl border border-surface-border/35 bg-surface-panel p-1.5 text-xs text-content-secondary shadow-xl shadow-[rgb(var(--shadow-color)/0.24)]"
                            role="listbox"
                          >
                            {(["sandbox", "on_failure", "ask", "full_access"] as const).map((mode) => {
                              const option = composerModePresentationLabel("default", mode, t);
                              const selected = chatMode === mode;
                              return (
                                <button
                                  aria-selected={selected}
                                  className={`flex w-full items-start gap-2 rounded-lg px-2.5 py-2 text-left outline-none transition-colors focus:bg-surface-raised ${
                                    selected ? "bg-accent/10 text-accent" : "hover:bg-surface-raised hover:text-content-primary"
                                  }`}
                                  key={mode}
                                  onClick={() => void selectComposerMode("default", mode)}
                                  role="option"
                                  title={option.description}
                                  type="button"
                                >
                                  <span className="w-3 pt-0.5 text-center" aria-hidden>{selected ? "✓" : ""}</span>
                                  <span>
                                    <span className="block font-semibold">
                                      {composerModeAccessLabelTranslated(mode, t)}
                                    </span>
                                    <span className="mt-0.5 block text-[11px] text-content-muted">{option.description}</span>
                                  </span>
                                </button>
                              );
                            })}
                            {chatMode === "legacy_restricted" ? (
                              <div
                                aria-selected="true"
                                className="flex items-start gap-2 rounded-lg bg-warning-surface/55 px-2.5 py-2 text-warning opacity-75"
                                role="option"
                              >
                                <span className="w-3 text-center" aria-hidden>✓</span>
                                <span>
                                  <span className="block font-semibold">{t("Restricted (legacy)")}</span>
                                  <span className="mt-0.5 block text-[11px]">{t("Legacy policy; choose a current access mode.")}</span>
                                </span>
                              </div>
                            ) : null}
                          </div>
                        ) : null}
                      </div>
                    ) : null}
                  </div>
                )
              ) : null}

              {!shellMode ? (
                <div className="relative" ref={responseMenuRef}>
                  <button
                    aria-label={t("Response options: {options}", {
                      options: responseOptionsLabel,
                    })}
                    aria-expanded={responseMenuOpen}
                    aria-haspopup="dialog"
                    className="inline-flex max-w-56 items-center gap-1.5 rounded-md border border-surface-border/35 bg-surface-sunken px-2 py-1.5 text-xs text-content-secondary outline-none transition-colors hover:text-content-primary focus:ring-2 focus:ring-accent/30"
                    onClick={() => {
                      setModeMenuOpen(false);
                      setResponseMenuOpen((open) => !open);
                    }}
                    title={responseOptionsLabel}
                    type="button"
                  >
                    <span className="truncate">{responseOptionsLabel}</span>
                    <span aria-hidden className="text-[9px]">▾</span>
                  </button>
                  {responseMenuOpen ? (
                    <div
                      aria-label={t("Response options")}
                      className="absolute bottom-full right-0 z-30 mb-2 w-72 max-w-[calc(100vw-3rem)] space-y-3 rounded-xl border border-surface-border/35 bg-surface-panel p-3 text-xs text-content-secondary shadow-xl shadow-[rgb(var(--shadow-color)/0.24)]"
                      data-update-card-yield=""
                      role="dialog"
                    >
                      <label className="block space-y-1.5">
                        <span className="font-semibold text-content-primary">
                          {t("Model")}
                        </span>
                        <select
                          className="w-full rounded-lg border border-surface-border/40 bg-surface-raised px-2.5 py-2 text-sm text-content-primary outline-none focus:border-accent/60 focus:ring-2 focus:ring-accent/20"
                          onChange={(event) => onModelChange(event.target.value)}
                          value={selectedModelInfo.id}
                        >
                  {nativeModels.length > 0 ? (
                    <optgroup label={t("Direct API")}>
                      {nativeModels.map((model) => (
                        <option key={model.id} value={model.id}>
                          {model.label}
                        </option>
                      ))}
                    </optgroup>
                  ) : null}
                  {sessionModelGroups.map(([group, groupModels]) => (
                    <optgroup key={group} label={group}>
                      {groupModels.map((model) => (
                        <option key={model.id} value={model.id}>
                          {model.label}
                          {model.modelBadge
                            ? ` · ${modelBadgeLabel(model.modelBadge, t)}`
                            : ""}
                        </option>
                      ))}
                    </optgroup>
                  ))}
                </select>
                {selectedModelInfo.modelBadge ? (
                  <span className="inline-flex w-fit rounded-full border border-accent/30 bg-accent/10 px-2 py-0.5 text-[10px] font-semibold text-accent-strong">
                    {modelBadgeLabel(selectedModelInfo.modelBadge, t)}
                  </span>
                ) : null}
                {selectedModelInfo.agentBackend && resolvedModel ? (
                  <span className="font-mono text-[10px] text-content-muted">
                    {t("Resolved: {model}", { model: resolvedModel })}
                  </span>
                ) : null}
              </label>
                      <label className="block space-y-1.5">
                        <span className="font-semibold text-content-primary">
                          {t("Credential profile")}
                        </span>
                        <select
                          className="w-full rounded-lg border border-surface-border/40 bg-surface-raised px-2.5 py-2 text-sm text-content-primary outline-none focus:border-accent/60 focus:ring-2 focus:ring-accent/20 disabled:cursor-not-allowed disabled:opacity-60"
                          disabled={credentialProfilePending}
                          onChange={(event) =>
                            void changeCredentialProfile(
                              event.target.value || null,
                            )
                          }
                          value={credentialProfileId ?? ""}
                        >
                          <option value="">{t("Default (no pin)")}</option>
                          {credentialProfileId && !pinnedProfileIsEligible ? (
                            <option disabled value={credentialProfileId}>
                              {t("Pinned: {name} ({status})", {
                                name:
                                  pinnedProfile?.name ?? t("Unknown profile"),
                                status: credentialProfileStatusLabel(
                                  pinnedProfile,
                                  credentialProfilesLoaded,
                                  t,
                                ),
                              })}
                            </option>
                          ) : null}
                          {eligibleProfiles.map((profile) => (
                            <option key={profile.id} value={profile.id}>
                              {profile.name}
                            </option>
                          ))}
                        </select>
                      </label>
                      <label className="block space-y-1.5">
                        <span className="font-semibold text-content-primary">
                          {t("Reasoning effort")}
                        </span>
                        <select
                          className="w-full rounded-lg border border-surface-border/40 bg-surface-raised px-2.5 py-2 text-sm text-content-primary outline-none focus:border-accent/60 focus:ring-2 focus:ring-accent/20"
                          onChange={(event) => onEffortChange(event.target.value)}
                          value={selectedEffort}
                        >
                          {selectedModelInfo.reasoningEfforts.map((effort) => (
                            <option key={effort} value={effort}>
                              {reasoningEffortLabel(effort, t)}
                            </option>
                          ))}
                        </select>
                      </label>
                      {fastModeAvailable ? (
                        <button
                          aria-pressed={fastMode}
                          className={`flex w-full items-center justify-between rounded-lg border px-2.5 py-2 text-left transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 ${
                            fastMode ? "border-accent/40 bg-accent/10 text-accent" : "border-surface-border/40 hover:bg-surface-raised"
                          }`}
                          onClick={() => onFastModeChange(!fastMode)}
                          type="button"
                        >
                          <span>
                            <span className="block font-semibold">
                              {t("Fast responses")}
                            </span>
                            <span className="mt-0.5 block text-[11px] text-content-muted">
                              {t("Uses more ChatGPT credits.")}
                            </span>
                          </span>
                          <span className="font-mono text-[10px] font-semibold uppercase">
                            {fastMode ? t("On") : t("Off")}
                          </span>
                        </button>
                      ) : null}
                    </div>
                  ) : null}
                </div>
              ) : null}
            </div>

            <div className="ml-auto flex shrink-0 flex-wrap items-center justify-end gap-1.5">
              {sendPending && primaryAction.kind !== "stop" ? (
                <button
                  className="rounded-lg border border-danger/35 px-3 py-2 text-xs font-semibold text-danger transition-colors hover:bg-danger/10 focus:outline-none focus:ring-2 focus:ring-danger/30 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={cancelAndRevertPending || stopRequested}
                  onClick={onCancel}
                  type="button"
                >
                  {t("Stop")}
                </button>
              ) : null}
              {sendPending && stopAndRevertAvailable ? (
                <button
                  className="rounded-lg border border-danger/35 px-3 py-2 text-xs font-semibold text-danger transition-colors hover:bg-danger/10 focus:outline-none focus:ring-2 focus:ring-danger/30 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={cancelAndRevertPending || stopRequested}
                  onClick={onCancelAndRevert}
                  title={
                    !currentConversationId
                      ? t(
                          "Stopping and reverting as soon as the conversation id is available",
                        )
                      : undefined
                  }
                  type="button"
                >
                  {t("Stop & revert")}
                </button>
              ) : null}
              <span
                className="inline-flex"
                onPointerDown={() => {
                  if (primaryDisabledReason) {
                    setSendAttempted(true);
                  }
                }}
              >
              <button
                aria-describedby={
                  primaryDisabledReason && (composerFocused || sendAttempted)
                    ? "composer-primary-help"
                    : undefined
                }
                aria-label={primaryAction.ariaLabel}
                className={`inline-flex min-h-9 items-center justify-center rounded-lg px-4 py-2 text-xs font-bold transition-colors focus:outline-none focus:ring-2 disabled:cursor-not-allowed disabled:opacity-45 ${
                  primaryAction.kind === "stop"
                    ? "border border-danger/35 bg-danger/10 text-danger hover:bg-danger/15 focus:ring-danger/30"
                    : "bg-accent text-accent-contrast hover:bg-accent-strong focus:ring-accent/40"
                }`}
                disabled={primaryDisabledReason !== null}
                onClick={primaryAction.kind === "stop" ? onCancel : undefined}
                title={
                  primaryAction.kind === "stop" && !currentConversationId
                    ? t(
                        "Stopping as soon as the conversation id is available",
                      )
                    : undefined
                }
                type={primaryAction.kind === "stop" ? "button" : "submit"}
              >
                {primaryAction.label}
              </button>
              </span>
            </div>
          </div>
          {primaryDisabledReason && (composerFocused || sendAttempted) ? (
            <p
              aria-live="polite"
              className="px-3 pb-2 text-[11px] text-content-muted"
              id="composer-primary-help"
            >
              {primaryDisabledReason}
            </p>
          ) : null}
        </div>
      </div>
    </form>
  );
}

function ComposerErrorNotice({
  message,
  onDismiss,
}: {
  message: string;
  onDismiss: () => void;
}) {
  const t = useT();
  return (
    <div
      className="flex items-start gap-2 rounded-lg border border-danger/25 bg-danger/10 px-3 py-2 text-xs text-danger"
      role="alert"
    >
      <span className="min-w-0 flex-1 whitespace-pre-wrap break-words">{message}</span>
      <button
        aria-label={t("Dismiss error")}
        className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-base transition-colors hover:bg-danger/10 focus:outline-none focus:ring-2 focus:ring-danger/30"
        onClick={onDismiss}
        type="button"
      >
        ×
      </button>
    </div>
  );
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function CommandPalette({
  items,
  onSelect,
  selectedIndex,
  slashDraft,
}: {
  items: PaletteItem[];
  onSelect: (item: PaletteItem) => void;
  selectedIndex: number;
  slashDraft: SlashDraft;
}) {
  const t = useT();
  return (
    <div
      className="absolute bottom-[calc(100%+0.75rem)] left-0 z-20 w-full max-w-2xl overflow-hidden rounded-lg border border-surface-border/30 bg-surface-glass/95 shadow-2xl shadow-[rgb(var(--shadow-color)/0.22)] backdrop-blur-xl"
      data-update-card-yield=""
    >
      <div className="border-b border-surface-border/25 px-3 py-2 font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted">
        {slashDraft.hasArgs ? `/${slashDraft.commandName}` : t("Commands")}
      </div>
      {items.length === 0 ? (
        <div className="px-3 py-4 text-sm text-content-muted">
          {t("No matches")}
        </div>
      ) : (
        <div className="max-h-72 overflow-y-auto p-1">
          {items.map((item, index) => (
            <button
              className={`flex w-full items-start justify-between gap-3 rounded-md px-3 py-2 text-left transition duration-150 ${
                index === selectedIndex
                  ? "bg-accent/10 text-content-primary"
                  : "text-content-secondary hover:bg-surface-raised/70"
              }`}
              key={`${item.kind}-${itemLabel(item)}`}
              onMouseDown={(event) => {
                event.preventDefault();
                onSelect(item);
              }}
              type="button"
            >
              <span className="min-w-0">
                <span className="block truncate text-sm font-medium">
                  {paletteItemDisplayLabel(item, t)}
                </span>
                <span className="mt-0.5 block truncate font-mono text-[11px] text-content-muted">
                  {itemDescription(item, t)}
                </span>
              </span>
              {index === selectedIndex ? (
                <span className="shrink-0 rounded-full border border-accent/25 bg-accent/10 px-2 py-0.5 font-mono text-[11px] text-accent">
                  {t("Enter")}
                </span>
              ) : null}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function FileMentionPalette({
  currentProject,
  error,
  loading,
  onSelect,
  results,
  selectedIndex,
}: {
  currentProject: Project | null;
  error: string | null;
  loading: boolean;
  onSelect: (result: ProjectFileSearchResult) => void;
  results: ProjectFileSearchResult[];
  selectedIndex: number;
}) {
  const t = useT();
  const disabledLabel = !currentProject
    ? t("No project")
    : error
      ? t("Could not search files")
      : loading && results.length === 0
        ? t("Searching files")
        : results.length === 0
          ? t("No matches")
          : null;

  return (
    <div
      className="absolute bottom-[calc(100%+0.75rem)] left-0 z-20 w-full max-w-2xl overflow-hidden rounded-lg border border-surface-border/30 bg-surface-glass/95 shadow-2xl shadow-[rgb(var(--shadow-color)/0.22)] backdrop-blur-xl"
      data-update-card-yield=""
    >
      <div className="border-b border-surface-border/25 px-3 py-2 font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted">
        {t("Files")}
      </div>
      {disabledLabel ? (
        <div className="px-3 py-4 text-sm text-content-muted" title={error ?? undefined}>
          {disabledLabel}
        </div>
      ) : (
        <div className="max-h-72 overflow-y-auto p-1">
          {results.map((result, index) => (
            <button
              className={`flex w-full items-start justify-between gap-3 rounded-md px-3 py-2 text-left transition duration-150 ${
                index === selectedIndex
                  ? "bg-accent/10 text-content-primary"
                  : "text-content-secondary hover:bg-surface-raised/70"
              }`}
              key={result.path}
              onMouseDown={(event) => {
                event.preventDefault();
                onSelect(result);
              }}
              type="button"
            >
              <span className="min-w-0">
                <span className="block truncate font-mono text-sm font-medium">
                  {result.path}
                </span>
                <span className="mt-0.5 block truncate font-mono text-[11px] text-content-muted">
                  {t("Reference this file")}
                </span>
              </span>
              {index === selectedIndex ? (
                <span className="shrink-0 rounded-full border border-accent/25 bg-accent/10 px-2 py-0.5 font-mono text-[11px] text-accent">
                  {t("Enter")}
                </span>
              ) : null}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function buildPaletteItems(
  slashDraft: SlashDraft | null,
  slashActive: boolean,
  commands: CommandRegistryEntry[],
  models: ChatModel[],
  selectedModelInfo: ChatModel,
  t: Translate,
): PaletteItem[] {
  if (!slashDraft || !slashActive) {
    return [];
  }

  const modelCommand = commands.find((command) => command.name === "model");
  if (slashDraft.hasArgs && slashDraft.commandName === "model" && modelCommand) {
    const query = slashDraft.args.trim().toLowerCase();
    return models
      .filter((model) => model.id.toLowerCase().includes(query))
      .map((model) => ({
        kind: "model" as const,
        command: modelCommand,
        value: model.id,
        description: modelPaletteDescription(model, t),
      }));
  }

  const effortCommand = commands.find(
    (command) => command.name === "effort",
  );
  if (slashDraft.hasArgs && slashDraft.commandName === "effort" && effortCommand) {
    const query = slashDraft.args.trim().toLowerCase();
    const efforts = selectedModelInfo.reasoningEfforts.length
      ? selectedModelInfo.reasoningEfforts
      : defaultEfforts;
    return efforts
      .filter((effort) => effort.toLowerCase().includes(query))
      .map((effort) => ({
        kind: "effort" as const,
        command: effortCommand,
        value: effort,
        description: effortDescription(effort, t),
      }));
  }

  const themeCommand = commands.find((command) => command.name === "theme");
  if (slashDraft.hasArgs && slashDraft.commandName === "theme" && themeCommand) {
    const query = slashDraft.args.trim().toLowerCase();
    return themePreferences
      .filter((theme) => theme.includes(query))
      .map((theme) => ({
        kind: "theme" as const,
        command: themeCommand,
        value: theme,
        description: t("Switch to this theme"),
      }));
  }

  const query = slashDraft.commandQuery.toLowerCase();
  return matchCommands(commands, query).map(({ command }) => ({
    kind: "command" as const,
    command,
  }));
}

function modelPaletteDescription(model: ChatModel, t: Translate) {
  if (model.reasoningEfforts.includes("ultra")) {
    return t("supports ultra");
  }
  if (model.reasoningEfforts.includes("max")) {
    return t("supports max");
  }
  const levelCount = model.reasoningEfforts.length;
  return levelCount === 1
    ? t("default effort {effort} · {count} level", {
        effort: model.defaultEffort,
        count: levelCount,
      })
    : t("default effort {effort} · {count} levels", {
        effort: model.defaultEffort,
        count: levelCount,
      });
}

function modelBadgeLabel(modelBadge: string, t: Translate) {
  return modelBadge === "via Codex CLI" ? t("via Codex CLI") : modelBadge;
}

function effortDescription(effort: string, t: Translate) {
  switch (effort) {
    case "low":
      return t("fastest, lightest");
    case "medium":
      return t("balanced (default)");
    case "high":
      return t("deeper reasoning");
    case "xhigh":
      return t("extra high — deeper still");
    case "max":
      return t("deepest reasoning, slowest");
    case "ultra":
      return t("multi-agent, most capable");
    default:
      return t("reasoning level");
  }
}

function reasoningEffortLabel(effort: string, t: Translate) {
  switch (effort) {
    case "low":
      return t("low");
    case "medium":
      return t("medium");
    case "high":
      return t("high");
    case "xhigh":
      return t("xhigh");
    case "max":
      return t("max");
    case "ultra":
      return t("ultra");
    default:
      return effort;
  }
}

function credentialProfileStatusLabel(
  profile: CredentialProfile | null,
  profilesLoaded: boolean,
  t: Translate,
) {
  if (!profile) {
    return profilesLoaded ? t("removed") : t("unavailable");
  }
  switch (profile.status) {
    case "active":
      return profile.provider;
    case "disabled":
      return t("disabled");
    case "invalid":
      return t("invalid");
  }
}

function parseSlashDraft(draft: string): SlashDraft | null {
  if (!draft.startsWith("/")) {
    return null;
  }

  // Command name ends at the first whitespace; args keep the rest of the
  // draft verbatim, including newlines (multi-line /goal text).
  const body = draft.slice(1);
  const separator = body.search(/\s/);
  if (separator < 0) {
    return {
      commandQuery: body,
      commandName: body,
      args: "",
      hasArgs: false,
    };
  }

  const commandName = body.slice(0, separator);
  return {
    commandQuery: commandName,
    commandName,
    args: body.slice(separator + 1),
    hasArgs: true,
  };
}

function parseFileMentionToken(
  draft: string,
  cursorPosition: number,
): FileMentionToken | null {
  const cursor = Math.min(Math.max(cursorPosition, 0), draft.length);
  const beforeCursor = draft.slice(0, cursor);
  let tokenStart = beforeCursor.length;
  while (tokenStart > 0 && !/\s/.test(beforeCursor[tokenStart - 1])) {
    tokenStart -= 1;
  }

  if (beforeCursor[tokenStart] !== "@") {
    return null;
  }

  const query = beforeCursor.slice(tokenStart + 1);
  if (/\s/.test(query)) {
    return null;
  }

  return {
    start: tokenStart,
    end: cursor,
    query,
    key: `${tokenStart}:${query}`,
  };
}

function itemLabel(item: PaletteItem) {
  return item.kind === "command" ? item.command.name : item.value;
}

function paletteItemDisplayLabel(item: PaletteItem, t: Translate) {
  switch (item.kind) {
    case "command":
      return `/${item.command.name} ${item.command.argsHint}`.trim();
    case "model":
      return item.value;
    case "effort":
      return reasoningEffortLabel(item.value, t);
    case "theme":
      return themePreferenceLabel(item.value, t);
  }
}

function themePreferenceLabel(theme: ThemePreference, t: Translate) {
  switch (theme) {
    case "system":
      return t("system");
    case "light":
      return t("light");
    case "dark":
      return t("dark");
  }
}

function itemDescription(item: PaletteItem, t: Translate) {
  return item.kind === "command"
    ? commandDescriptionLabel(item.command, t)
    : item.description;
}
