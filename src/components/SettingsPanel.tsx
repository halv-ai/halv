import {
  SettingsConnections,
  type SettingsConnectionsRequest,
} from "./SettingsConnections";
import { SettingsCategoryIcon } from "./SettingsCategoryIcon";
import { SettingsMobile } from "./SettingsMobile";
import { featureFlagStore } from "../featureFlags";
import { canManageBilling } from "../billingAvailability";
import { ipc, type UnlistenFn } from "../ipc.ts";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { appUpdateCommandErrorLabel } from "../appUpdater";
import {
  appUpdateBlockerLines,
  appUpdateCannotInstallCopy,
  appUpdateCanInstall,
  appUpdateProgressLabel,
  appUpdateProgressPercent,
  appUpdateSettingsStatus,
} from "../appUpdateCard";
import { appUpdateStore } from "../appUpdateStore";
import {
  attemptSupervisorRestart,
  confirmSupervisorRestart,
  type SupervisorRestartConfirmation,
} from "../terminalSupervisorRestart";
import { useAppUpdateView } from "./AppUpdateCard";
import { accountPlanLabel } from "../accountPlan";
import { analyticsInstallId } from "../analytics";
import { billingPortal } from "../billing";
import { paymentIssueMessage } from "../billingIssue";
import { copyText } from "../clipboard";
import { confirmDialog } from "../dialogs";
import {
  CREDENTIAL_PROFILE_PROVIDERS,
  credentialProfileKeyHint,
  credentialProfileKeyPlaceholder,
  credentialProfileProviderLabel,
  nextMissingCredentialProfileProvider,
  type CredentialProfileProvider,
} from "../credentialProfiles";
import {
  authPrimaryLabel,
  authSecondaryLabel,
  formatEngineStatus,
} from "../format";
import { parseSubagentConcurrencyLimit } from "../preferences";
import type { WalletProvider } from "../accountWallet";
import { setNotificationPreferencesCache } from "../notifications";
import {
  DEFAULT_CATALOG,
  DEFAULT_TIER_AGENTS,
  type LadderTier,
  type TierAgentAssignment,
} from "../taskRouting";
import { themePreferences, type ThemePreference } from "../theme";
import { localeDefinitions } from "../i18n/config";
import type { LanguagePreference } from "../i18n/language";
import {
  formatDateTime,
  formatNumber,
  useT,
  type Translate,
} from "../i18n/runtime";
import type {
  AgentMemory,
  AccountAuthStatus,
  AppPreferences,
  CredentialProfile,
  SavingsToday,
  SavingsTotals,
  SidecarInstallError,
  SidecarInstallProgress,
  SidecarStatus,
  StatusState,
  SupportDiagnostics,
  SavingsQueueStatus,
  TerminalSupervisorStatus,
} from "../types";
import { useBillingCheckout } from "../useBillingCheckout";
import { SavingsDashboard } from "./SavingsDashboard";
import { LifetimeOffer } from "./LifetimeOffer";
import { AnonymousUsagePreference } from "./AnonymousUsagePreference";

const HEADROOM_INSTALL_COMMAND = 'pipx install "headroom-ai[proxy]"';
const HEADROOM_PIP_FALLBACK_COMMAND =
  'python3 -m pip install --user "headroom-ai[proxy]"';

const SETTINGS_CATEGORY_IDS = [
  "account",
  "connections",
  "jev",
  "preferences",
  "agent",
  "mobile",
  "system",
] as const;

type SettingsCategory = (typeof SETTINGS_CATEGORY_IDS)[number];

type SettingsPanelProps = {
  open: boolean;
  status: StatusState;
  loginPending: boolean;
  savingsTotals: SavingsTotals | null;
  todaySavings: SavingsToday | null;
  languagePreference: LanguagePreference;
  themePreference: ThemePreference;
  onClose: () => void;
  onLanguageChange: (language: LanguagePreference) => void;
  onThemeChange: (theme: ThemePreference) => void;
  onOpenBrowserSettings: () => void;
  onOpenKeyboardShortcuts: () => void;
  onAccountSignOut: () => Promise<void>;
  onAccountRefresh: () => Promise<void>;
  onAccountStatusChange: (status: AccountAuthStatus) => void;
  onLoginChatGpt: () => Promise<void>;
  onLogout: () => Promise<void>;
  onRestartSidecar: () => Promise<void>;
  onInstallHeadroom: () => Promise<SidecarStatus>;
  onUpdateHeadroom: () => Promise<SidecarStatus>;
  onCredentialProfilesChanged: () => void;
  credentialProfilesFocusSignal?: number;
  memoriesFocusSignal?: number;
  privacyFocusSignal?: number;
  /** Opens System and shows "How Halv estimates savings" (ADR 0015). */
  savingsMethodFocusSignal?: number;
  connectionsRequest?: SettingsConnectionsRequest | null;
};

export function SettingsPanel({
  open,
  status,
  loginPending,
  savingsTotals,
  todaySavings,
  languagePreference,
  themePreference,
  onClose,
  onLanguageChange,
  onThemeChange,
  onOpenBrowserSettings,
  onOpenKeyboardShortcuts,
  onAccountSignOut,
  onAccountRefresh,
  onAccountStatusChange,
  onLoginChatGpt,
  onLogout,
  onRestartSidecar,
  onInstallHeadroom,
  onUpdateHeadroom,
  onCredentialProfilesChanged,
  credentialProfilesFocusSignal = 0,
  memoriesFocusSignal = 0,
  privacyFocusSignal = 0,
  savingsMethodFocusSignal = 0,
  connectionsRequest = null,
}: SettingsPanelProps) {
  const t = useT();
  const allSettingsCategories = [
    {
      id: "account",
      label: t("Account & plan"),
      description: t("Your Halv account, subscription, and billing."),
    },
    {
      id: "connections",
      label: t("AI connections"),
      description: t("Codex, Claude, and your model providers."),
    },
    {
      id: "jev",
      label: "JEV",
      description: t("Task routing"),
    },
    {
      id: "preferences",
      label: t("Preferences"),
      description: t("Appearance, response tone, alerts, and shortcuts."),
    },
    {
      id: "agent",
      label: t("Agent & privacy"),
      description: t(
        "Memory, browser access, concurrency, and local diagnostics.",
      ),
    },
    {
      id: "mobile",
      label: t("Mobile"),
      description: t("Pair a phone and follow your terminals from it."),
    },
    {
      id: "system",
      label: t("System"),
      description: t("App updates, the proxy, and savings data."),
    },
  ] as const;
  // Mobile shows only while the mobileRemote feature flag is on (ADR 0009).
  const mobileRemote = useSyncExternalStore(
    featureFlagStore.subscribe,
    () => featureFlagStore.getSnapshot().mobileRemote,
  );
  const settingsCategories = allSettingsCategories.filter(
    (category) => category.id !== "mobile" || mobileRemote,
  );
  const [activeCategory, setActiveCategory] =
    useState<SettingsCategory>("account");
  const [credentialProfiles, setCredentialProfiles] = useState<
    CredentialProfile[]
  >([]);
  const [credentialProfilesLoadState, setCredentialProfilesLoadState] =
    useState<"idle" | "loading" | "loaded" | "error">("idle");
  const [credentialProfilesRefreshAttempt, setCredentialProfilesRefreshAttempt] =
    useState(0);
  const [credentialProfileProvider, setCredentialProfileProvider] =
    useState<CredentialProfileProvider>("openai");
  const [credentialProfileName, setCredentialProfileName] = useState("");
  const [credentialProfileSecret, setCredentialProfileSecret] = useState("");
  const [credentialProfileAddPending, setCredentialProfileAddPending] =
    useState(false);
  const [credentialProfileActionPendingId, setCredentialProfileActionPendingId] =
    useState<string | null>(null);
  const [credentialProfileRenameId, setCredentialProfileRenameId] = useState<
    string | null
  >(null);
  const [credentialProfileRenameName, setCredentialProfileRenameName] =
    useState("");
  const [credentialProfilePriorityDrafts, setCredentialProfilePriorityDrafts] =
    useState<Record<string, string>>({});
  const [credentialProfilesError, setCredentialProfilesError] = useState<
    string | null
  >(null);
  const [credentialProfilesNotice, setCredentialProfilesNotice] = useState<
    string | null
  >(null);
  const [logoutPending, setLogoutPending] = useState(false);
  const [restartPending, setRestartPending] = useState(false);
  const [accountRefreshPending, setAccountRefreshPending] = useState(false);
  const [accountSignOutPending, setAccountSignOutPending] = useState(false);
  const [billingPending, setBillingPending] = useState<"portal" | null>(null);
  const checkout = useBillingCheckout(onAccountStatusChange);
  const [accountError, setAccountError] = useState<string | null>(null);
  const [referralLinkCopied, setReferralLinkCopied] = useState(false);
  const referralCopiedTimer = useRef<number | undefined>(undefined);
  const [installCommandCopied, setInstallCommandCopied] = useState(false);
  const installCopiedTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    // Clear the copied reset timer on unmount.
    return () => {
      window.clearTimeout(installCopiedTimer.current);
      window.clearTimeout(referralCopiedTimer.current);
    };
  }, []);
  const [copyError, setCopyError] = useState<string | null>(null);
  const [installPending, setInstallPending] = useState(false);
  const [installLog, setInstallLog] = useState<string[]>([]);
  const [installError, setInstallError] = useState<string | null>(null);
  const [manualInstallOpen, setManualInstallOpen] = useState(false);
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const appUpdate = useAppUpdateView();
  const [supervisorStatus, setSupervisorStatus] =
    useState<TerminalSupervisorStatus | null>(null);
  const [savingsQueue, setSavingsQueue] = useState<SavingsQueueStatus | null>(null);
  const [supervisorRestartConfirmation, setSupervisorRestartConfirmation] =
    useState<SupervisorRestartConfirmation | null>(null);
  const [supervisorRestartPending, setSupervisorRestartPending] =
    useState(false);
  const [supervisorRestartError, setSupervisorRestartError] = useState<
    string | null
  >(null);
  const [headroomUpdatePending, setHeadroomUpdatePending] = useState(false);
  const [subagentConcurrencyInput, setSubagentConcurrencyInput] = useState("");
  const [savedSubagentConcurrencyLimit, setSavedSubagentConcurrencyLimit] =
    useState<number | null>(null);
  const [preferencesLoadState, setPreferencesLoadState] =
    useState<"idle" | "loading" | "loaded" | "error">("idle");
  const [preferencesLoadAttempt, setPreferencesLoadAttempt] = useState(0);
  const [preferencesSavePending, setPreferencesSavePending] = useState(false);
  const [preferencesError, setPreferencesError] = useState<string | null>(null);
  const [preferencesNotice, setPreferencesNotice] = useState<string | null>(
    null,
  );
  const [personality, setPersonality] =
    useState<AppPreferences["personality"]>("none");
  const [personalitySavePending, setPersonalitySavePending] = useState(false);
  const [personalityError, setPersonalityError] = useState<string | null>(null);
  const [personalityNotice, setPersonalityNotice] = useState<string | null>(null);
  const [ponytail, setPonytail] = useState<AppPreferences["ponytail"]>("off");
  const [ponytailSavePending, setPonytailSavePending] = useState(false);
  const [ponytailError, setPonytailError] = useState<string | null>(null);
  const [ponytailNotice, setPonytailNotice] = useState<string | null>(null);
  const [taskRouting, setTaskRouting] = useState(true);
  const [taskRoutingSavePending, setTaskRoutingSavePending] = useState(false);
  const [taskRoutingError, setTaskRoutingError] = useState<string | null>(null);
  const [taskRoutingNotice, setTaskRoutingNotice] = useState<string | null>(null);
  const [workflowRouting, setWorkflowRouting] = useState(true);
  const [claudeUltracode, setClaudeUltracode] = useState(false);
  const [workflowSavePending, setWorkflowSavePending] = useState(false);
  const [workflowError, setWorkflowError] = useState<string | null>(null);
  const [workflowNotice, setWorkflowNotice] = useState<string | null>(null);
  const [routingTierAgents, setRoutingTierAgents] = useState<
    AppPreferences["routingTierAgents"]
  >(defaultRoutingTierAgents());
  const [routingTierAgentsSavePending, setRoutingTierAgentsSavePending] =
    useState(false);
  const [routingTierAgentsError, setRoutingTierAgentsError] = useState<
    string | null
  >(null);
  const [routingTierAgentsNotice, setRoutingTierAgentsNotice] = useState<
    string | null
  >(null);
  const [claudeSubscriptionProxy, setClaudeSubscriptionProxy] = useState(true);
  const [claudeSubscriptionProxySavePending, setClaudeSubscriptionProxySavePending] =
    useState(false);
  const [claudeSubscriptionProxyError, setClaudeSubscriptionProxyError] =
    useState<string | null>(null);
  const [claudeSubscriptionProxyNotice, setClaudeSubscriptionProxyNotice] =
    useState<string | null>(null);
  const [memoriesEnabled, setMemoriesEnabled] = useState(true);
  const [memoriesSavePending, setMemoriesSavePending] = useState(false);
  const [memoriesError, setMemoriesError] = useState<string | null>(null);
  const [memoriesNotice, setMemoriesNotice] = useState<string | null>(null);
  const [installId, setInstallId] = useState<string | null>(null);
  const [memories, setMemories] = useState<AgentMemory[]>([]);
  const [memoriesLoadState, setMemoriesLoadState] =
    useState<"idle" | "loading" | "loaded" | "error">("idle");
  const [memoriesRefreshAttempt, setMemoriesRefreshAttempt] = useState(0);
  const [memoryDeletePendingId, setMemoryDeletePendingId] = useState<
    number | null
  >(null);
  const [memoryRebuildPending, setMemoryRebuildPending] = useState(false);
  const [memoriesClearPending, setMemoriesClearPending] = useState(false);
  const credentialProfilesSectionRef = useRef<HTMLElement>(null);
  const handledConnectionsSignalRef = useRef(0);
  const [pendingWalletAdd, setPendingWalletAdd] =
    useState<WalletProvider | null>(null);
  const memoriesSectionRef = useRef<HTMLElement>(null);
  const privacySectionRef = useRef<HTMLElement>(null);
  const [completionNotifications, setCompletionNotifications] = useState<
    AppPreferences["completionNotifications"]
  >("background");
  const [permissionNotifications, setPermissionNotifications] = useState(true);
  const [questionNotifications, setQuestionNotifications] = useState(true);
  const [savedNotificationPreferences, setSavedNotificationPreferences] =
    useState<Pick<
      AppPreferences,
      | "completionNotifications"
      | "permissionNotifications"
      | "questionNotifications"
    > | null>(null);
  const [notificationSavePending, setNotificationSavePending] = useState(false);
  const [notificationError, setNotificationError] = useState<string | null>(null);
  const [notificationNotice, setNotificationNotice] = useState<string | null>(null);
  const [diagnostics, setDiagnostics] = useState<SupportDiagnostics | null>(null);
  const [diagnosticsPending, setDiagnosticsPending] = useState(false);
  const [diagnosticsExportPending, setDiagnosticsExportPending] = useState(false);
  const [diagnosticsError, setDiagnosticsError] = useState<string | null>(null);
  const [diagnosticsNotice, setDiagnosticsNotice] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      return;
    }

    let disposed = false;
    void getVersion()
      .then((version) => {
        if (!disposed) {
          setAppVersion(version);
        }
      })
      .catch(() => {
        if (!disposed) {
          setAppVersion(null);
        }
      });

    return () => {
      disposed = true;
    };
  }, [open]);

  useEffect(() => {
    if (!open) {
      return;
    }

    let disposed = false;
    void analyticsInstallId()
      .then((value) => {
        if (!disposed) {
          setInstallId(value);
        }
      })
      .catch(() => {
        if (!disposed) {
          setInstallId(null);
        }
      });

    return () => {
      disposed = true;
    };
  }, [open]);

  useEffect(() => {
    if (!open) {
      return;
    }

    let disposed = false;
    setPreferencesLoadState("loading");
    setPreferencesError(null);
    setPreferencesNotice(null);

    void ipc.preferences.get()
      .then((preferences) => {
        if (disposed) {
          return;
        }
        setSavedSubagentConcurrencyLimit(
          preferences.subagentConcurrencyLimit,
        );
        setSubagentConcurrencyInput(
          preferences.subagentConcurrencyLimit === null
            ? ""
            : String(preferences.subagentConcurrencyLimit),
        );
        setCompletionNotifications(preferences.completionNotifications);
        setPermissionNotifications(preferences.permissionNotifications);
        setQuestionNotifications(preferences.questionNotifications);
        setPersonality(preferences.personality ?? "none");
        setPonytail(preferences.ponytail ?? "off");
        setTaskRouting(preferences.taskRouting ?? true);
        setWorkflowRouting(preferences.workflowRouting ?? true);
        setClaudeUltracode(preferences.claudeUltracode ?? false);
        const legacyLane = preferences.workerLane ?? "auto";
        setRoutingTierAgents(
          legacyLane === "auto"
            ? (preferences.routingTierAgents ?? defaultRoutingTierAgents())
            : routingTierAgentsForLane(legacyLane),
        );
        setClaudeSubscriptionProxy(preferences.claudeSubscriptionProxy ?? true);
        setMemoriesEnabled(preferences.memoriesEnabled ?? true);
        setSavedNotificationPreferences({
          completionNotifications: preferences.completionNotifications,
          permissionNotifications: preferences.permissionNotifications,
          questionNotifications: preferences.questionNotifications,
        });
        setNotificationPreferencesCache(preferences);
        setPreferencesLoadState("loaded");
      })
      .catch((error) => {
        if (disposed) {
          return;
        }
        setPreferencesLoadState("error");
        setPreferencesError(
          t("Could not load the subagent concurrency preference: {error}", {
            error: errorMessage(error),
          }),
        );
      });

    return () => {
      disposed = true;
    };
  }, [open, preferencesLoadAttempt, t]);

  useEffect(() => {
    if (!open) {
      return;
    }
    let disposed = false;
    setCredentialProfilesLoadState("loading");
    setCredentialProfilesError(null);
    void ipc.storage.listCredentialProfiles()
      .then((profiles) => {
        if (disposed) {
          return;
        }
        setCredentialProfiles(profiles);
        setCredentialProfilePriorityDrafts(
          Object.fromEntries(
            profiles.map((profile) => [profile.id, String(profile.priority)]),
          ),
        );
        setCredentialProfilesLoadState("loaded");
      })
      .catch((error) => {
        if (!disposed) {
          setCredentialProfilesLoadState("error");
          setCredentialProfilesError(
            t("Could not load credential profiles: {error}", {
              error: errorMessage(error),
            }),
          );
        }
      });
    return () => {
      disposed = true;
    };
  }, [credentialProfilesRefreshAttempt, open, t]);

  useEffect(() => {
    if (!open) {
      return;
    }
    let disposed = false;
    setMemoriesLoadState("loading");
    setMemoriesError(null);
    void ipc.storage.listAgentMemories()
      .then((rows) => {
        if (!disposed) {
          setMemories(rows);
          setMemoriesLoadState("loaded");
        }
      })
      .catch((error) => {
        if (!disposed) {
          setMemoriesLoadState("error");
          setMemoriesError(
            t("Could not load memories: {error}", {
              error: errorMessage(error),
            }),
          );
        }
      });
    return () => {
      disposed = true;
    };
  }, [memoriesRefreshAttempt, open, t]);

  useEffect(() => {
    if (!open || credentialProfilesFocusSignal === 0) {
      return;
    }
    setActiveCategory("connections");
    const frame = window.requestAnimationFrame(() => {
      credentialProfilesSectionRef.current?.scrollIntoView({
        behavior: "smooth",
        block: "start",
      });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [credentialProfilesFocusSignal, open]);

  useEffect(() => {
    if (
      !open ||
      !connectionsRequest ||
      handledConnectionsSignalRef.current === connectionsRequest.signal
    ) {
      return;
    }
    handledConnectionsSignalRef.current = connectionsRequest.signal;
    setActiveCategory("connections");
    setPendingWalletAdd(connectionsRequest.addProvider);
  }, [connectionsRequest, open]);

  useEffect(() => {
    if (!open) setPendingWalletAdd(null);
  }, [open]);

  useEffect(() => {
    if (!open || memoriesFocusSignal === 0) {
      return;
    }
    setActiveCategory("agent");
    const frame = window.requestAnimationFrame(() => {
      memoriesSectionRef.current?.scrollIntoView({
        behavior: "smooth",
        block: "start",
      });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [open, memoriesFocusSignal]);

  useEffect(() => {
    if (!open || privacyFocusSignal === 0) {
      return;
    }
    setActiveCategory("agent");
    const frame = window.requestAnimationFrame(() => {
      privacySectionRef.current?.scrollIntoView({
        behavior: "smooth",
        block: "start",
      });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [open, privacyFocusSignal]);

  useEffect(() => {
    if (!open || savingsMethodFocusSignal === 0) {
      return;
    }
    setActiveCategory("system");
    const frame = window.requestAnimationFrame(() => {
      document.getElementById("savings-method")?.scrollIntoView({
        behavior: "smooth",
        block: "start",
      });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [open, savingsMethodFocusSignal]);

  // The savings queue line in System loads each time the category opens.
  useEffect(() => {
    if (!open || activeCategory !== "system") {
      return;
    }
    let cancelled = false;
    void ipc.account.savingsQueueStatus().then(
      (status) => {
        if (!cancelled) setSavingsQueue(status);
      },
      () => {
        if (!cancelled) setSavingsQueue(null);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [activeCategory, open]);

  // The supervisor line in System loads each time the category opens.
  useEffect(() => {
    if (!open || activeCategory !== "system") {
      return;
    }
    let cancelled = false;
    void ipc.terminal
      .supervisorStatus()
      .then(
        (supervisor) => {
          if (!cancelled) setSupervisorStatus(supervisor);
        },
        () => {
          if (!cancelled) setSupervisorStatus(null);
        },
      );
    return () => {
      cancelled = true;
    };
  }, [activeCategory, open]);

  useEffect(() => {
    if (!open) {
      return;
    }

    let disposed = false;
    let unlisten: UnlistenFn | null = null;

    void ipc.events.listen<SidecarInstallProgress>(
      "sidecar_install_progress",
      (payload) => {
        setInstallLog((current) => [...current, payload.line].slice(-80));
        if (payload.stage === "error") {
          setInstallError(payload.line);
        }
      },
    ).then((listener) => {
      if (disposed) {
        listener();
      } else {
        unlisten = listener;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [open]);

  useEffect(() => {
    if (!open || activeCategory !== "agent") return;
    const refresh = () => { void ipc.storage.listAgentMemories().then(setMemories).catch(() => {}); };
    window.addEventListener("focus", refresh);
    const timer = window.setInterval(refresh, 10000);
    return () => { window.removeEventListener("focus", refresh); window.clearInterval(timer); };
  }, [open, activeCategory]);

  if (!open) {
    return null;
  }

  function finishCredentialProfileAction(notice: string) {
    setCredentialProfilesNotice(notice);
    setCredentialProfilesRefreshAttempt((attempt) => attempt + 1);
    onCredentialProfilesChanged();
  }

  async function addCredentialProfile(event: React.FormEvent) {
    event.preventDefault();
    const name = credentialProfileName.trim();
    const secret = credentialProfileSecret.trim();
    setCredentialProfilesError(null);
    setCredentialProfilesNotice(null);
    if (!name || !secret) {
      setCredentialProfilesError(t("Enter a profile name and API key."));
      return;
    }
    setCredentialProfileAddPending(true);
    try {
      const profile = await ipc.credential.profileAdd({
          provider: credentialProfileProvider,
          name,
          kind: "api_key",
          secret,
        });
      setCredentialProfileName("");
      setCredentialProfileSecret("");
      setCredentialProfileProvider(
        nextMissingCredentialProfileProvider(
          [...credentialProfiles, profile],
          credentialProfileProvider,
        ),
      );
      finishCredentialProfileAction(
        t("Added {name}.", { name: profile.name }),
      );
    } catch (error) {
      setCredentialProfilesError(t(errorMessage(error)));
    } finally {
      setCredentialProfileAddPending(false);
    }
  }

  async function testCredentialProfile(profile: CredentialProfile) {
    setCredentialProfileActionPendingId(profile.id);
    setCredentialProfilesError(null);
    setCredentialProfilesNotice(null);
    try {
      const tested = await ipc.credential.profileTest({ id: profile.id });
      finishCredentialProfileAction(
        tested.status === "active"
          ? t("{name} is valid.", { name: tested.name })
          : t("{name} is invalid.", { name: tested.name }),
      );
    } catch (error) {
      setCredentialProfilesError(t(errorMessage(error)));
    } finally {
      setCredentialProfileActionPendingId(null);
    }
  }

  async function renameCredentialProfile(
    event: React.FormEvent,
    profile: CredentialProfile,
  ) {
    event.preventDefault();
    const name = credentialProfileRenameName.trim();
    setCredentialProfilesError(null);
    setCredentialProfilesNotice(null);
    if (!name) {
      setCredentialProfilesError(t("Enter a profile name."));
      return;
    }
    setCredentialProfileActionPendingId(profile.id);
    try {
      const renamed = await ipc.credential.profileRename({ id: profile.id, name });
      setCredentialProfileRenameId(null);
      setCredentialProfileRenameName("");
      finishCredentialProfileAction(
        t("Renamed profile to {name}.", { name: renamed.name }),
      );
    } catch (error) {
      setCredentialProfilesError(t(errorMessage(error)));
    } finally {
      setCredentialProfileActionPendingId(null);
    }
  }

  async function setCredentialProfileEnabled(
    profile: CredentialProfile,
    enabled: boolean,
  ) {
    setCredentialProfileActionPendingId(profile.id);
    setCredentialProfilesError(null);
    setCredentialProfilesNotice(null);
    try {
      const updated = await ipc.credential.profileSetEnabled({ id: profile.id, enabled });
      finishCredentialProfileAction(
        updated.status === "active"
          ? t("{name} enabled.", { name: updated.name })
          : t("{name} disabled.", { name: updated.name }),
      );
    } catch (error) {
      setCredentialProfilesError(t(errorMessage(error)));
    } finally {
      setCredentialProfileActionPendingId(null);
    }
  }

  async function saveCredentialProfilePriority(profile: CredentialProfile) {
    const priorityInput = credentialProfilePriorityDrafts[profile.id]?.trim();
    const priority = Number(priorityInput);
    setCredentialProfilesError(null);
    setCredentialProfilesNotice(null);
    if (!priorityInput || !Number.isSafeInteger(priority)) {
      setCredentialProfilesError(t("Priority must be a whole number."));
      return;
    }
    setCredentialProfileActionPendingId(profile.id);
    try {
      const updated = await ipc.credential.profileSetPriority({ id: profile.id, priority });
      finishCredentialProfileAction(
        t("Saved priority for {name}.", { name: updated.name }),
      );
    } catch (error) {
      setCredentialProfilesError(t(errorMessage(error)));
    } finally {
      setCredentialProfileActionPendingId(null);
    }
  }

  async function removeCredentialProfile(profile: CredentialProfile) {
    if (
      !(await confirmDialog({
        danger: true,
        message: t("Remove credential profile {name}?", {
          name: profile.name,
        }),
      }))
    ) {
      return;
    }
    setCredentialProfileActionPendingId(profile.id);
    setCredentialProfilesError(null);
    setCredentialProfilesNotice(null);
    try {
      await ipc.credential.profileRemove({ id: profile.id });
      if (credentialProfileRenameId === profile.id) {
        setCredentialProfileRenameId(null);
        setCredentialProfileRenameName("");
      }
      finishCredentialProfileAction(
        t("Removed {name}.", { name: profile.name }),
      );
    } catch (error) {
      setCredentialProfilesError(t(errorMessage(error)));
    } finally {
      setCredentialProfileActionPendingId(null);
    }
  }

  async function saveSubagentConcurrency(event: React.FormEvent) {
    event.preventDefault();
    const parsed = parseSubagentConcurrencyLimit(subagentConcurrencyInput);
    if (!parsed.valid) {
      setPreferencesNotice(null);
      setPreferencesError(
        t("Enter a positive whole number, or leave it empty for Unlimited."),
      );
      return;
    }

    setPreferencesSavePending(true);
    setPreferencesError(null);
    setPreferencesNotice(null);
    try {
      const preferences = await ipc.preferences.setSubagentConcurrencyLimit({ limit: parsed.limit });
      setSavedSubagentConcurrencyLimit(preferences.subagentConcurrencyLimit);
      setSubagentConcurrencyInput(
        preferences.subagentConcurrencyLimit === null
          ? ""
          : String(preferences.subagentConcurrencyLimit),
      );
      setPreferencesNotice(
        preferences.subagentConcurrencyLimit === null
          ? t("Saved. Concurrent child agents are Unlimited.")
          : t("Saved. Up to {count} child agents can run at once.", {
              count: preferences.subagentConcurrencyLimit,
            }),
      );
    } catch (error) {
      setPreferencesError(
        t("Could not save the subagent concurrency preference: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setPreferencesSavePending(false);
    }
  }

  async function savePersonality(nextPersonality: AppPreferences["personality"]) {
    setPersonalitySavePending(true);
    setPersonalityError(null);
    setPersonalityNotice(null);
    try {
      const preferences = await ipc.preferences.setPersonality({ personality: nextPersonality });
      setPersonality(preferences.personality);
      setPersonalityNotice(
        t("Personality set to {personality}.", {
          personality: preferences.personality,
        }),
      );
    } catch (error) {
      setPersonalityError(
        t("Could not save the personality preference: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setPersonalitySavePending(false);
    }
  }

  async function savePonytail(nextMode: AppPreferences["ponytail"]) {
    setPonytailSavePending(true);
    setPonytailError(null);
    setPonytailNotice(null);
    try {
      const preferences = await ipc.preferences.setPonytail({ mode: nextMode });
      setPonytail(preferences.ponytail);
      setPonytailNotice(ponytailNoticeLabel(preferences.ponytail, t));
    } catch (error) {
      setPonytailError(
        t("Could not save the Ponytail preference: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setPonytailSavePending(false);
    }
  }

  async function saveClaudeSubscriptionProxy(nextEnabled: boolean) {
    setClaudeSubscriptionProxySavePending(true);
    setClaudeSubscriptionProxyError(null);
    setClaudeSubscriptionProxyNotice(null);
    try {
      const preferences = await ipc.preferences.setClaudeSubscriptionProxy({ claudeSubscriptionProxy: nextEnabled });
      setClaudeSubscriptionProxy(preferences.claudeSubscriptionProxy);
      setClaudeSubscriptionProxyNotice(
        preferences.claudeSubscriptionProxy
          ? t(
              "Claude subscription routing is enabled for new terminal sessions.",
            )
          : t("Claude subscription routing is disabled."),
      );
    } catch (error) {
      setClaudeSubscriptionProxyError(
        t("Could not save the Claude subscription routing preference: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setClaudeSubscriptionProxySavePending(false);
    }
  }

  async function rebuildRecentMemories() {
    setMemoryRebuildPending(true);
    setMemoriesNotice(null);
    setMemoriesError(null);
    try {
      await ipc.storage.rebuildAgentMemories();
      setMemories(await ipc.storage.listAgentMemories());
      setMemoriesLoadState("loaded");
      setMemoriesNotice(t("Recent history checked. Only durable facts are saved."));
    } catch (error) {
      setMemoriesError(errorMessage(error));
    } finally {
      setMemoryRebuildPending(false);
    }
  }



  async function saveTaskRouting(nextEnabled: boolean) {
    setTaskRoutingSavePending(true);
    setTaskRoutingError(null);
    setTaskRoutingNotice(null);
    try {
      const preferences = await ipc.preferences.setTaskRouting({
        taskRouting: nextEnabled,
      });
      setTaskRouting(preferences.taskRouting);
      setTaskRoutingNotice(
        preferences.taskRouting
          ? t("Task routing enabled for new agent sessions.")
          : t("Task routing disabled."),
      );
    } catch (error) {
      setTaskRoutingError(
        t("Could not save task routing preference: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setTaskRoutingSavePending(false);
    }
  }

  async function saveWorkflowRouting(nextEnabled: boolean) {
    setWorkflowSavePending(true);
    setWorkflowError(null);
    setWorkflowNotice(null);
    try {
      const preferences = await ipc.preferences.setWorkflowRouting({
        workflowRouting: nextEnabled,
      });
      setWorkflowRouting(preferences.workflowRouting);
      setWorkflowNotice(
        preferences.workflowRouting
          ? t("Workflow routing enabled for new Claude panes.")
          : t("Workflow routing disabled."),
      );
    } catch (error) {
      setWorkflowError(
        t("Could not save workflow routing preference: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setWorkflowSavePending(false);
    }
  }

  async function saveClaudeUltracode(nextEnabled: boolean) {
    setWorkflowSavePending(true);
    setWorkflowError(null);
    setWorkflowNotice(null);
    try {
      const preferences = await ipc.preferences.setClaudeUltracode({
        claudeUltracode: nextEnabled,
      });
      setClaudeUltracode(preferences.claudeUltracode);
      setWorkflowNotice(
        preferences.claudeUltracode
          ? t("New Claude panes start in ultracode.")
          : t("New Claude panes start without ultracode."),
      );
    } catch (error) {
      setWorkflowError(
        t("Could not save ultracode preference: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setWorkflowSavePending(false);
    }
  }

  async function saveRoutingTierAgent(
    tier: Exclude<LadderTier, "max">,
    assignment: TierAgentAssignment,
  ) {
    const nextTierAgents = { ...routingTierAgents, [tier]: assignment };
    setRoutingTierAgentsSavePending(true);
    setRoutingTierAgentsError(null);
    setRoutingTierAgentsNotice(null);
    try {
      const preferences = await ipc.preferences.setRoutingTierAgents({
        routingTierAgents: nextTierAgents,
      });
      setRoutingTierAgents(preferences.routingTierAgents);
      setRoutingTierAgentsNotice(
        t("Saved {model} for {tier}.", {
          model:
            assignment.model ?? workerLaneLabel(assignment.lane, t),
          tier: routingTierLabel(tier, t),
        }),
      );
    } catch (error) {
      setRoutingTierAgentsError(
        t("Could not save routing agent: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setRoutingTierAgentsSavePending(false);
    }
  }

  async function saveMemoriesEnabled(nextEnabled: boolean) {
    setMemoriesSavePending(true);
    setMemoriesError(null);
    setMemoriesNotice(null);
    try {
      const preferences = await ipc.preferences.setMemoriesEnabled({ memoriesEnabled: nextEnabled });
      setMemoriesEnabled(preferences.memoriesEnabled);
      setMemoriesNotice(
        preferences.memoriesEnabled
          ? t("Memory generation and injection are enabled.")
          : t("Memory generation and injection are disabled globally."),
      );
    } catch (error) {
      setMemoriesError(
        t("Could not save the memories preference: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setMemoriesSavePending(false);
    }
  }

  async function deleteMemory(id: number) {
    setMemoryDeletePendingId(id);
    setMemoriesError(null);
    setMemoriesNotice(null);
    try {
      await ipc.storage.deleteAgentMemory({ id });
      setMemories((current) => current.filter((memory) => memory.id !== id));
      setMemoriesNotice(t("Memory deleted."));
    } catch (error) {
      setMemoriesError(
        t("Could not delete memory: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setMemoryDeletePendingId(null);
    }
  }

  async function clearMemories() {
    if (
      !(await confirmDialog({
        danger: true,
        message: t(
          "Clear all memories? This removes global and project memories and cannot be undone.",
        ),
      }))
    ) {
      return;
    }
    setMemoriesClearPending(true);
    setMemoriesError(null);
    setMemoriesNotice(null);
    try {
      await ipc.storage.clearAgentMemories();
      setMemories([]);
      setMemoriesNotice(t("All memories cleared."));
    } catch (error) {
      setMemoriesError(
        t("Could not clear memories: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setMemoriesClearPending(false);
    }
  }

  async function saveNotificationPreferences(event: React.FormEvent) {
    event.preventDefault();
    setNotificationSavePending(true);
    setNotificationError(null);
    setNotificationNotice(null);
    try {
      const preferences = await ipc.preferences.setNotifications({
          completionNotifications,
          permissionNotifications,
          questionNotifications,
        });
      const saved = {
        completionNotifications: preferences.completionNotifications,
        permissionNotifications: preferences.permissionNotifications,
        questionNotifications: preferences.questionNotifications,
      };
      setCompletionNotifications(saved.completionNotifications);
      setPermissionNotifications(saved.permissionNotifications);
      setQuestionNotifications(saved.questionNotifications);
      setSavedNotificationPreferences(saved);
      setNotificationPreferencesCache(preferences);
      setNotificationNotice(t("Notification preferences saved."));
    } catch (error) {
      setNotificationError(
        t("Could not save notification preferences: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setNotificationSavePending(false);
    }
  }

  async function runLocalDiagnostics() {
    setDiagnosticsPending(true);
    setDiagnosticsError(null);
    setDiagnosticsNotice(null);
    try {
      const snapshot = await ipc.diagnostics.snapshot();
      setDiagnostics(snapshot);
    } catch (error) {
      setDiagnosticsError(
        t("Could not run local diagnostics: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setDiagnosticsPending(false);
    }
  }

  async function exportSupportReport() {
    setDiagnosticsExportPending(true);
    setDiagnosticsError(null);
    setDiagnosticsNotice(null);
    try {
      const exported = await ipc.diagnostics.exportSupportReport();
      if (exported) {
        setDiagnosticsNotice(
          t(
            "Support report saved locally. Review it before choosing to share it.",
          ),
        );
      }
    } catch (error) {
      setDiagnosticsError(
        t("Could not export the support report: {error}", {
          error: errorMessage(error),
        }),
      );
    } finally {
      setDiagnosticsExportPending(false);
    }
  }

  async function logout() {
    setLogoutPending(true);
    try {
      await onLogout();
    } finally {
      setLogoutPending(false);
    }
  }

  async function refreshAccount() {
    setAccountRefreshPending(true);
    setAccountError(null);
    try {
      await onAccountRefresh();
    } catch (error) {
      setAccountError(errorMessage(error));
    } finally {
      setAccountRefreshPending(false);
    }
  }

  async function accountSignOut() {
    setAccountSignOutPending(true);
    setAccountError(null);
    try {
      await onAccountSignOut();
    } catch (error) {
      setAccountError(errorMessage(error));
    } finally {
      setAccountSignOutPending(false);
    }
  }

  async function openBillingPortal() {
    setBillingPending("portal");
    setAccountError(null);
    try {
      await billingPortal();
    } catch (error) {
      setAccountError(errorMessage(error));
    } finally {
      setBillingPending(null);
    }
  }

  async function restartSidecar() {
    setRestartPending(true);
    try {
      await onRestartSidecar();
    } finally {
      setRestartPending(false);
    }
  }

  async function installHeadroom() {
    setInstallPending(true);
    setInstallError(null);
    setCopyError(null);
    setManualInstallOpen(false);
    setInstallLog([t("Setting up the proxy.")]);
    try {
      const sidecar = await onInstallHeadroom();
      if (sidecar.state === "running") {
        setInstallLog((current) => [
          ...current,
          t("The proxy is running on port {port}.", { port: sidecar.port }),
        ]);
      } else if (sidecar.state === "error") {
        setInstallError(sidecar.message);
      }
    } catch (error) {
      const parsed = parseInstallError(error);
      setInstallError(parsed.message);
      if (parsed.details.length > 0) {
        setInstallLog((current) => [...current, ...parsed.details].slice(-80));
      }
      setManualInstallOpen(true);
    } finally {
      setInstallPending(false);
    }
  }

  async function updateHeadroom() {
    setHeadroomUpdatePending(true);
    setInstallError(null);
    setInstallLog([t("Starting proxy update.")]);
    try {
      const sidecar = await onUpdateHeadroom();
      if (sidecar.state === "running") {
        setInstallLog((current) => [
          ...current,
          t("Proxy {version} is running on port {port}.", {
            version: sidecar.version ?? t("unknown"),
            port: sidecar.port,
          }),
        ]);
      } else if (sidecar.state === "error") {
        setInstallError(sidecar.message);
      }
    } catch (error) {
      const parsed = parseInstallError(error);
      setInstallError(parsed.message);
      if (parsed.details.length > 0) {
        setInstallLog((current) => [...current, ...parsed.details].slice(-80));
      }
    } finally {
      setHeadroomUpdatePending(false);
    }
  }

  async function copyInstallCommand() {
    if (await copyText(HEADROOM_INSTALL_COMMAND)) {
      setCopyError(null);
      setInstallCommandCopied(true);
      window.clearTimeout(installCopiedTimer.current);
      installCopiedTimer.current = window.setTimeout(
        () => setInstallCommandCopied(false),
        1800,
      );
    } else {
      setInstallCommandCopied(false);
      setCopyError(t("Could not copy the install command."));
    }
  }

  async function copyReferralLink(link: string) {
    setAccountError(null);
    if (!(await copyText(link))) {
      setReferralLinkCopied(false);
      setAccountError(t("Could not copy invite link."));
      return;
    }
    setReferralLinkCopied(true);
    window.clearTimeout(referralCopiedTimer.current);
    referralCopiedTimer.current = window.setTimeout(
      () => setReferralLinkCopied(false),
      1_800,
    );
  }

  async function finishSupervisorRestart(next: TerminalSupervisorStatus) {
    if (!next.available || next.supervisorStale) {
      throw new Error(
        next.message ?? t("Could not restart the terminal supervisor."),
      );
    }
    setSupervisorStatus(await ipc.terminal.supervisorStatus());
    setSupervisorRestartConfirmation(null);
  }

  // The first request never ends sessions. When sessions run, the user
  // confirms, and the second request ends them.
  async function restartSupervisor(confirmed: boolean) {
    if (supervisorRestartPending) return;
    setSupervisorRestartPending(true);
    setSupervisorRestartError(null);
    const invoke = (killSessions: boolean) =>
      ipc.terminal.supervisorRestart({ killSessions });
    try {
      if (confirmed) {
        await finishSupervisorRestart(
          await confirmSupervisorRestart(supervisorRestartConfirmation, invoke),
        );
        return;
      }
      const attempt = await attemptSupervisorRestart(invoke);
      if (attempt.confirmation) {
        setSupervisorRestartConfirmation(attempt.confirmation);
        return;
      }
      await finishSupervisorRestart(attempt.status);
    } catch (restartError) {
      setSupervisorRestartError(
        restartError instanceof Error && restartError.message.trim()
          ? restartError.message
          : t("Could not restart the terminal supervisor."),
      );
    } finally {
      setSupervisorRestartPending(false);
    }
  }

  const authDetail = authSecondaryLabel(status.auth);
  const chatGptConnected = Boolean(
    status.auth &&
      "chatgpt" in status.auth &&
      status.auth.chatgpt.accessTokenValid,
  );
  const account = status.account;
  const accountEntitlements = account?.entitlements ?? null;
  const accountReferral =
    account?.referral ?? accountEntitlements?.referral ?? null;
  const accountPlan = account?.plan ?? accountEntitlements?.plan;
  const paymentIssue = accountEntitlements?.paymentIssue ?? null;
  const billingPortalAvailable = canManageBilling(accountEntitlements);
  const billingCancellationAvailable =
    billingPortalAvailable &&
    !paymentIssue &&
    (accountEntitlements?.plan === "basic" ||
      accountEntitlements?.plan === "unlimited");
  const sidecar = status.sidecar;
  const sidecarInstalling = sidecar?.state === "installing";
  const headroomUpdateAvailable =
    sidecar?.state === "running" &&
    sidecar.managedByHalv &&
    sidecar.version !== null &&
    sidecar.version !== sidecar.pinnedVersion;
  const showInstallPanel =
    sidecar?.state === "notInstalled" ||
    sidecarInstalling ||
    installPending ||
    installLog.length > 0 ||
    installError !== null;
  const showManualInstall = manualInstallOpen || installError !== null;
  const updateSnapshot = appUpdate.snapshot;
  const updateStatus = updateSnapshot?.status ?? null;
  const updateStatusLine = updateSnapshot
    ? appUpdateSettingsStatus(updateSnapshot, {
        t,
        debug: import.meta.env.DEV,
      })
    : null;
  const updateBusy =
    updateStatus?.state === "downloading" ||
    updateStatus?.state === "installing" ||
    updateStatus?.state === "restarting";
  const updateCanInstall = updateSnapshot
    ? appUpdateCanInstall(updateSnapshot.installMode)
    : false;
  const updateBlockers =
    updateStatus?.state === "ready" && updateSnapshot?.blockers
      ? updateSnapshot.blockers
      : null;
  const parsedSubagentConcurrency = parseSubagentConcurrencyLimit(
    subagentConcurrencyInput,
  );
  const subagentConcurrencyUnchanged =
    parsedSubagentConcurrency.valid &&
    parsedSubagentConcurrency.limit === savedSubagentConcurrencyLimit;
  const notificationPreferencesUnchanged =
    savedNotificationPreferences !== null &&
    savedNotificationPreferences.completionNotifications ===
      completionNotifications &&
    savedNotificationPreferences.permissionNotifications ===
      permissionNotifications &&
    savedNotificationPreferences.questionNotifications === questionNotifications;
  const activeCategoryDetails =
    settingsCategories.find((category) => category.id === activeCategory) ??
    settingsCategories[0];
  const accountPlanName = accountPlanLabel(accountPlan);
  const accountSyncedAt = account?.syncedAt
    ? formatDateTime(new Date(account.syncedAt))
    : null;
  const accountPlanSummary = accountSyncedAt
    ? account?.stale
      ? t("Plan {plan} / synced {date} / stale", {
          plan: accountPlanName,
          date: accountSyncedAt,
        })
      : t("Plan {plan} / synced {date}", {
          plan: accountPlanName,
          date: accountSyncedAt,
        })
    : account?.stale
      ? t("Plan {plan} / stale", { plan: accountPlanName })
      : t("Plan {plan}", { plan: accountPlanName });

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-surface-sunken/70 p-0 backdrop-blur-sm sm:px-4 sm:py-6">
      <section
        aria-labelledby="settings-title"
        aria-modal="true"
        className="settings-dialog flex w-full max-w-5xl flex-col overflow-hidden border-0 bg-surface-panel shadow-2xl shadow-[rgb(var(--shadow-color)/0.24)] sm:rounded-lg sm:border sm:border-surface-border/30"
        role="dialog"
      >
        <div className="flex items-center justify-between border-b border-surface-border/25 px-5 py-4">
          <div>
            <h2
              className="text-base font-semibold text-content-primary"
              id="settings-title"
            >
              {t("Settings")}
            </h2>
            <p className="mt-1 text-sm text-content-muted">
              {t("Manage your account, preferences, and local tools.")}
            </p>
          </div>
          <button
            className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={onClose}
            type="button"
          >
            {t("Close")}
          </button>
        </div>

        <nav
          aria-label={t("Settings categories")}
          className="flex shrink-0 gap-1 overflow-x-auto border-b border-surface-border/25 px-3 py-2 sm:hidden"
        >
          {settingsCategories.map((category) => (
            <button
              aria-current={activeCategory === category.id ? "page" : undefined}
              className={`shrink-0 rounded-md px-3 py-2 text-left text-xs font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 ${
                activeCategory === category.id
                  ? "bg-accent/10 text-accent"
                  : "text-content-muted hover:bg-surface-raised hover:text-content-primary"
              }`}
              key={category.id}
              onClick={() => setActiveCategory(category.id)}
              type="button"
            >
              {category.label}
            </button>
          ))}
        </nav>

        <div className="flex min-h-0 flex-1">
          <nav
            aria-label={t("Settings categories")}
            className="settings-navigation hidden w-56 shrink-0 flex-col gap-1 border-r border-surface-border/25 p-3 sm:flex"
          >
            {settingsCategories.map((category) => (
              <button
                aria-current={activeCategory === category.id ? "page" : undefined}
                className={`settings-category flex items-start gap-3 rounded-xl px-3 py-3 text-left transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 ${
                  activeCategory === category.id
                    ? "bg-accent/10 text-accent"
                    : "text-content-muted hover:bg-surface-raised hover:text-content-primary"
                }`}
                key={category.id}
                onClick={() => setActiveCategory(category.id)}
                type="button"
              >
                <SettingsCategoryIcon category={category.id} />
                <span><span className="block text-sm font-medium">{category.label}</span>
                <span className="mt-1 block text-[11px] leading-4 opacity-75">
                  {category.description}
                </span></span>
              </button>
            ))}
          </nav>

          <div className="settings-content min-h-0 min-w-0 flex-1 space-y-5 overflow-y-auto p-4 sm:p-7">
          {status.error ? (
            <div className="rounded-md border border-danger/40 bg-danger-surface/55 px-3 py-2 text-sm text-danger">
              {status.error}
            </div>
          ) : null}

          <div className="border-b border-surface-border/25 pb-4">
            <h3 className="text-lg font-semibold text-content-primary">
              {activeCategoryDetails.label}
            </h3>
            <p className="mt-1 text-sm text-content-muted">
              {activeCategoryDetails.description}
            </p>
          </div>

          <section className="space-y-3" hidden={activeCategory !== "account"}>
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Halv account")}
              </h4>
              <p className="mt-1 text-xs text-content-muted">
                {t("Your Halv license")}
              </p>
              <p className="mt-2 text-sm text-content-secondary">
                {account?.email ?? t("Signed in")}
              </p>
              <p className="mt-1 text-xs text-content-muted">
                {accountPlanSummary}
              </p>
            </div>

            {paymentIssue ? (
              <div className="rounded-md border border-warning/35 bg-warning-surface/45 px-3 py-3">
                <h5 className="text-sm font-medium text-warning">
                  {t("Payment needs attention")}
                </h5>
                <p className="mt-1 text-xs leading-5 text-content-secondary">
                  {paymentIssueMessage(paymentIssue, t)}
                </p>
                <button
                  className="mt-3 rounded-md bg-accent px-3 py-2 text-sm font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
                  disabled={billingPending !== null}
                  onClick={() => void openBillingPortal()}
                  type="button"
                >
                  {billingPending === "portal"
                    ? t("Opening billing...")
                    : t("Fix payment")}
                </button>
              </div>
            ) : null}

            <LifetimeOffer account={account ?? null} checkout={checkout} />
            <div className="flex flex-wrap gap-2">
              <button
                className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                disabled={accountRefreshPending}
                onClick={() => void refreshAccount()}
                type="button"
              >
                {accountRefreshPending
                  ? t("Refreshing...")
                  : t("Refresh status")}
              </button>
              <button
                className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                disabled={accountSignOutPending}
                onClick={() => void accountSignOut()}
                type="button"
              >
                {accountSignOutPending
                  ? t("Signing out of Halv...")
                  : t("Sign out of Halv")}
              </button>
              {billingPortalAvailable ? (
                <button
                  className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={billingPending !== null}
                  onClick={() => void openBillingPortal()}
                  type="button"
                >
                  {billingPending === "portal"
                    ? t("Opening billing...")
                    : t("Manage billing")}
                </button>
              ) : null}
            </div>

            {billingCancellationAvailable ? (
              <div>
                <button
                  className="inline-flex text-xs text-content-muted underline-offset-4 hover:text-content-primary hover:underline focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={billingPending !== null}
                  onClick={() => void openBillingPortal()}
                  type="button"
                >
                  {t("Cancel subscription")}
                </button>
                <p className="mt-1 text-[11px] leading-4 text-content-muted">
                  {t("Review or cancel your subscription securely in Stripe.")}
                </p>
              </div>
            ) : null}

              {account?.signedIn && accountReferral ? (
                <div className="rounded-md border border-surface-border/30 bg-surface-raised/45 px-3 py-3">
                  <h5 className="text-sm font-medium text-content-primary">
                    {t("Invite a friend")}
                  </h5>
                  <div className="mt-2 flex flex-wrap items-center gap-2">
                    <code className="rounded-md bg-surface-sunken px-2 py-1 font-mono text-xs tracking-[0.12em] text-content-primary">
                      {accountReferral.code}
                    </code>
                    <button
                      className="rounded-md border border-surface-border/30 px-2.5 py-1.5 text-xs text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
                      onClick={() =>
                        void copyReferralLink(accountReferral.link)
                      }
                      type="button"
                    >
                      {referralLinkCopied ? t("Copied") : t("Copy link")}
                    </button>
                  </div>
                  <p className="mt-2 text-xs leading-5 text-content-secondary">
                    {t(
                      "When someone you invite makes their first payment, you get one month free.",
                    )}
                  </p>
                  <p className="text-xs leading-5 text-content-muted">
                    {t("They get a {trialDays}-day trial with your code.", {
                      trialDays: accountReferral.trialDays,
                    })}
                  </p>
                  {accountReferral.invited > 0 ? (
                    <p className="text-xs leading-5 text-content-muted">
                      {t("{invited} invited \u00b7 {rewarded} rewarded", {
                        invited: formatNumber(
                          accountReferral.invited,
                        ),
                        rewarded: formatNumber(
                          accountReferral.rewarded,
                        ),
                      })}
                    </p>
                  ) : null}
                </div>
              ) : null}

              {accountError ? (
              <div className="rounded-md border border-danger/40 bg-danger-surface/55 px-3 py-2 text-sm text-danger">
                {accountError}
              </div>
            ) : null}
          </section>

          <section
            className="scroll-mt-4 space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "agent"}
            id="settings-memories"
            ref={memoriesSectionRef}
          >
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <h4 className="text-sm font-medium text-content-primary">
                  {t("Memories")}
                </h4>
                <p className="mt-1 text-sm text-content-secondary">
                  {t(
                    "Let the agent retain durable preferences, project conventions, and decisions across conversations.",
                  )}
                </p>
              </div>
              <button
                className="rounded-md border border-danger/40 px-3 py-1.5 text-xs font-medium text-danger transition-colors hover:bg-danger/10 disabled:opacity-50"
                disabled={
                  memories.length === 0 ||
                  memoriesClearPending ||
                  memoriesLoadState === "loading"
                }
                onClick={() => void clearMemories()}
                type="button"
              >
                {memoriesClearPending ? t("Clearing...") : t("Clear all")}
              </button>
            </div>

            <button
              aria-pressed={memoriesEnabled}
              className={`flex w-full items-center justify-between rounded-md border px-3 py-2 text-left text-sm transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 ${
                memoriesEnabled
                  ? "border-accent/60 bg-accent/10 text-content-primary"
                  : "border-surface-border/30 text-content-secondary hover:bg-surface-raised"
              }`}
              disabled={
                preferencesLoadState !== "loaded" || memoriesSavePending
              }
              onClick={() => void saveMemoriesEnabled(!memoriesEnabled)}
              type="button"
            >
              <span>{t("Use memories globally")}</span>
              <span className="text-xs font-semibold uppercase tracking-wide">
                {memoriesEnabled ? t("On") : t("Off")}
              </span>
            </button>

            <div className="flex flex-wrap items-center justify-between gap-3">
              <p className="text-xs text-content-muted">{t("Extract durable facts from recent completed Codex and Claude turns.")}</p>
              <button type="button" className="settings-secondary-action shrink-0" disabled={!memoriesEnabled || memoryRebuildPending || memoriesLoadState !== "loaded"} onClick={() => void rebuildRecentMemories()}>
                {memoryRebuildPending ? t("Extracting memories…") : t("Extract recent history")}
              </button>
            </div>
            {memoriesLoadState === "loading" ? (
              <p className="text-xs text-content-muted" role="status">
                {t("Loading memories...")}
              </p>
            ) : null}
            {memoriesLoadState === "error" ? (
              <button
                className="text-xs font-medium text-accent hover:underline"
                onClick={() =>
                  setMemoriesRefreshAttempt((current) => current + 1)
                }
                type="button"
              >
                {t("Retry loading memories")}
              </button>
            ) : null}
            {memoriesLoadState === "loaded" && memories.length === 0 ? (
              <p className="rounded-md border border-dashed border-surface-border/30 px-3 py-4 text-sm text-content-muted">
                {t(
                  "No memories yet. Extract recent history or finish a new conversation to get started.",
                )}
              </p>
            ) : null}
            {memories.length > 0 ? (
              <div className="max-h-72 space-y-2 overflow-y-auto pr-1">
                {memories.map((memory) => (
                  <article
                    className="rounded-md border border-surface-border/30 bg-surface-raised/55 p-3"
                    key={memory.id}
                  >
                    <div className="flex items-start justify-between gap-3">
                      <p className="min-w-0 whitespace-pre-wrap text-sm text-content-primary">
                        {memory.content}
                      </p>
                      <button
                        aria-label={t("Delete memory")}
                        className="shrink-0 text-xs font-medium text-danger hover:underline disabled:opacity-50"
                        disabled={
                          memoryDeletePendingId === memory.id ||
                          memoriesClearPending
                        }
                        onClick={() => void deleteMemory(memory.id)}
                        type="button"
                      >
                        {memoryDeletePendingId === memory.id
                          ? t("Deleting...")
                          : t("Delete")}
                      </button>
                    </div>
                    <p className="mt-2 text-xs text-content-muted">
                      {t("{project} · {date}", {
                        project: memory.projectName ?? t("Global"),
                        date: formatDateTime(
                          new Date(memory.updatedAt * 1_000),
                          {
                            day: "numeric",
                            month: "numeric",
                            year: "numeric",
                          },
                        ),
                      })}
                    </p>
                  </article>
                ))}
              </div>
            ) : null}
            {memoriesError ? (
              <p className="text-xs text-danger" role="alert">
                {memoriesError}
              </p>
            ) : null}
            {memoriesNotice ? (
              <p className="text-xs text-accent" role="status">
                {memoriesNotice}
              </p>
            ) : null}
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "agent"}
          >
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Browser")}
              </h4>
              <p className="mt-1 text-sm text-content-secondary">
                {t(
                  "Configure site permissions, raw CDP access, and the dedicated browser profile used by agents and the shared browser surface.",
                )}
              </p>
            </div>
            <button
              className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
              onClick={onOpenBrowserSettings}
              type="button"
            >
              {t("Open browser settings")}
            </button>
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "preferences"}
          >
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Keyboard shortcuts")}
              </h4>
              <p className="mt-1 text-sm text-content-secondary">
                {t(
                  "Find commands by name or keystroke, change bindings, and reset custom shortcuts to their defaults.",
                )}
              </p>
            </div>
            <button
              className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
              onClick={onOpenKeyboardShortcuts}
              type="button"
            >
              {t("Open keyboard shortcuts")}
            </button>
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "preferences"}
          >
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Notifications")}
              </h4>
              <p className="mt-1 text-sm text-content-secondary">
                {t(
                  "Choose when completed work should alert you, and whether Halv should notify you when an agent needs input.",
                )}
              </p>
            </div>
            <form className="space-y-3" onSubmit={saveNotificationPreferences}>
              <fieldset
                className="grid gap-2 sm:grid-cols-3"
                disabled={preferencesLoadState !== "loaded" || notificationSavePending}
              >
                <legend className="mb-2 text-xs font-medium text-content-secondary">
                  {t("Turn completion")}
                </legend>
                {(["never", "background", "always"] as const).map((mode) => (
                  <label
                    className={`flex cursor-pointer items-center gap-2 rounded-md border px-3 py-2 text-sm capitalize transition-colors ${
                      completionNotifications === mode
                        ? "border-accent/60 bg-accent/10 text-content-primary"
                        : "border-surface-border/30 text-content-secondary hover:bg-surface-raised"
                    }`}
                    key={mode}
                  >
                    <input
                      checked={completionNotifications === mode}
                      className="accent-accent"
                      name="completion-notifications"
                      onChange={() => {
                        setCompletionNotifications(mode);
                        setNotificationError(null);
                        setNotificationNotice(null);
                      }}
                      type="radio"
                    />
                    {notificationModeLabel(mode, t)}
                  </label>
                ))}
              </fieldset>
              <div className="grid gap-2 sm:grid-cols-2">
                <label className="flex items-center gap-2 rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary">
                  <input
                    checked={permissionNotifications}
                    className="accent-accent"
                    disabled={preferencesLoadState !== "loaded" || notificationSavePending}
                    onChange={(event) => {
                      setPermissionNotifications(event.target.checked);
                      setNotificationError(null);
                      setNotificationNotice(null);
                    }}
                    type="checkbox"
                  />
                  {t("Permission requests")}
                </label>
                <label className="flex items-center gap-2 rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary">
                  <input
                    checked={questionNotifications}
                    className="accent-accent"
                    disabled={preferencesLoadState !== "loaded" || notificationSavePending}
                    onChange={(event) => {
                      setQuestionNotifications(event.target.checked);
                      setNotificationError(null);
                      setNotificationNotice(null);
                    }}
                    type="checkbox"
                  />
                  {t("Agent questions")}
                </label>
              </div>
              <button
                className="rounded-md bg-content-primary px-3 py-2 text-sm font-semibold text-surface-base transition-colors duration-150 hover:bg-content-secondary disabled:cursor-not-allowed disabled:opacity-60"
                disabled={
                  preferencesLoadState !== "loaded" ||
                  notificationSavePending ||
                  notificationPreferencesUnchanged
                }
                type="submit"
              >
                {notificationSavePending
                  ? t("Saving...")
                  : t("Save notifications")}
              </button>
            </form>
            {notificationError ? (
              <p className="text-xs text-danger" role="alert">
                {notificationError}
              </p>
            ) : null}
            {notificationNotice ? (
              <p className="text-xs text-accent" role="status">
                {notificationNotice}
              </p>
            ) : null}
          </section>

          <section className="space-y-3" hidden={activeCategory !== "connections"}>
            {activeCategory === "connections" ? <SettingsConnections
              chatGptConnected={chatGptConnected}
              chatGptPending={loginPending}
              logoutPending={logoutPending}
              onChatGptLogin={() => void onLoginChatGpt()}
              onChatGptLogout={() => void logout()}
              onConnectionsChanged={onCredentialProfilesChanged}
              onWalletAddRequestHandled={() => setPendingWalletAdd(null)}
              walletAddRequest={pendingWalletAdd}
            /> : null}
          </section>

          <section className="space-y-3" hidden={activeCategory !== "mobile"}>
            {activeCategory === "mobile" && mobileRemote ? <SettingsMobile /> : null}
          </section>

        <section
          className="scroll-mt-4 space-y-3 border-t border-surface-border/25 pt-5"
          hidden={activeCategory !== "connections"}
          id="settings-credential-profiles"
          ref={credentialProfilesSectionRef}
        >
          <div>
            <h4 className="text-sm font-medium text-content-primary">
              {t("Credential profiles")}
            </h4>
            <p className="mt-1 text-xs text-content-muted">
              {t(
                "Add API keys and choose which credential each conversation uses. OAuth profiles use the existing ChatGPT login.",
              )}
            </p>
          </div>

          <form
            className="grid gap-2 rounded-lg border border-surface-border/25 bg-surface-raised/35 p-3 sm:grid-cols-2"
            onSubmit={addCredentialProfile}
          >
            <label className="space-y-1 text-xs text-content-muted">
              <span>{t("Provider")}</span>
              <select
                className="w-full rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none focus:border-accent focus:ring-2 focus:ring-accent/30"
                disabled={credentialProfileAddPending}
                onChange={(event) =>
                  setCredentialProfileProvider(
                    event.target.value as CredentialProfileProvider,
                  )
                }
                value={credentialProfileProvider}
              >
                {CREDENTIAL_PROFILE_PROVIDERS.map((provider) => (
                  <option key={provider} value={provider}>
                    {credentialProfileProviderLabel(provider)}
                  </option>
                ))}
              </select>
            </label>
            <label className="space-y-1 text-xs text-content-muted">
              <span>{t("Name")}</span>
              <input
                className="w-full rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
                disabled={credentialProfileAddPending}
                onChange={(event) => setCredentialProfileName(event.target.value)}
                placeholder={t("Work account")}
                value={credentialProfileName}
              />
            </label>
            <label className="space-y-1 text-xs text-content-muted sm:col-span-2">
              <span>{t("API key")}</span>
              <input
                className="w-full rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
                disabled={credentialProfileAddPending}
                onChange={(event) =>
                  setCredentialProfileSecret(event.target.value)
                }
                placeholder={t(
                  credentialProfileKeyPlaceholder(credentialProfileProvider),
                )}
                type="password"
                value={credentialProfileSecret}
              />
              {credentialProfileKeyHint(credentialProfileProvider) ? (
                <small>
                  {t(credentialProfileKeyHint(credentialProfileProvider) ?? "")}
                </small>
              ) : null}
            </label>
            <button
              className="w-fit rounded-md bg-content-primary px-3 py-2 text-sm font-semibold text-surface-base transition-colors duration-150 hover:bg-content-secondary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60 sm:col-span-2"
              disabled={credentialProfileAddPending}
              type="submit"
            >
              {credentialProfileAddPending
                ? t("Validating and adding...")
                : t("Add profile")}
            </button>
          </form>

          {credentialProfilesError ? (
            <p className="text-xs text-danger" role="alert">
              {credentialProfilesError}
            </p>
          ) : null}
          {credentialProfilesNotice ? (
            <p className="text-xs text-accent" role="status">
              {credentialProfilesNotice}
            </p>
          ) : null}

          {credentialProfilesLoadState === "loading" ? (
            <p className="text-xs text-content-muted">
              {t("Loading profiles...")}
            </p>
          ) : null}
          {credentialProfilesLoadState === "loaded" &&
          credentialProfiles.length === 0 ? (
            <p className="text-xs text-content-muted">
              {t("No credential profiles yet.")}
            </p>
          ) : null}
          {credentialProfiles.length > 0 ? (
            <div className="space-y-2">
              {credentialProfiles.map((profile) => {
                const busy = credentialProfileActionPendingId === profile.id;
                const anotherProfileBusy =
                  credentialProfileActionPendingId !== null && !busy;
                const renaming = credentialProfileRenameId === profile.id;
                const lastValidated = formatCredentialProfileLastValidated(
                  profile.lastValidatedAt,
                  t,
                );
                return (
                  <article
                    className="space-y-3 rounded-lg border border-surface-border/30 bg-surface-raised/35 p-3"
                    key={profile.id}
                  >
                    <div className="flex flex-wrap items-start justify-between gap-2">
                      <div className="min-w-0">
                        <p className="truncate text-sm font-semibold text-content-primary">
                          {profile.name}
                        </p>
                        <p className="mt-1 text-xs text-content-muted">
                          {t("{provider} · {kind}", {
                            provider: credentialProfileProviderLabel(
                              profile.provider,
                            ),
                            kind:
                              profile.kind === "oauth"
                                ? t("OAuth")
                                : t("API key"),
                          })}
                        </p>
                      </div>
                      <span
                        className={`rounded-full border px-2 py-0.5 font-mono text-[10px] font-semibold uppercase tracking-[0.08em] ${
                          profile.status === "active"
                            ? "border-accent/35 bg-accent/10 text-accent"
                            : profile.status === "invalid"
                              ? "border-danger/35 bg-danger/10 text-danger"
                              : "border-surface-border/35 bg-surface-sunken text-content-muted"
                        }`}
                      >
                        {credentialProfileStatusLabel(profile.status, t)}
                      </span>
                    </div>

                    <div className="grid gap-2 text-xs text-content-muted sm:grid-cols-2">
                      <label className="flex items-center gap-2">
                        <span>{t("Priority")}</span>
                        <input
                          aria-label={t("Priority for {name}", {
                            name: profile.name,
                          })}
                          className="w-20 rounded-md border border-surface-border/30 bg-surface-raised px-2 py-1.5 text-content-primary outline-none focus:border-accent focus:ring-2 focus:ring-accent/30"
                          disabled={busy || anotherProfileBusy}
                          inputMode="numeric"
                          onChange={(event) =>
                            setCredentialProfilePriorityDrafts((current) => ({
                              ...current,
                              [profile.id]: event.target.value,
                            }))
                          }
                          value={
                            credentialProfilePriorityDrafts[profile.id] ??
                            String(profile.priority)
                          }
                        />
                        <button
                          className="rounded-md border border-surface-border/30 px-2 py-1.5 text-content-secondary transition-colors hover:bg-surface-panel hover:text-content-primary disabled:cursor-not-allowed disabled:opacity-50"
                          disabled={busy || anotherProfileBusy}
                          onClick={() =>
                            void saveCredentialProfilePriority(profile)
                          }
                          type="button"
                        >
                          {t("Save")}
                        </button>
                      </label>
                      <span className="sm:text-right">
                        {t("Last validated: {date}", {
                          date: lastValidated,
                        })}
                      </span>
                    </div>

                    {renaming ? (
                      <form
                        className="flex flex-col gap-2 sm:flex-row"
                        onSubmit={(event) =>
                          void renameCredentialProfile(event, profile)
                        }
                      >
                        <input
                          aria-label={t("New name for {name}", {
                            name: profile.name,
                          })}
                          autoFocus
                          className="min-w-0 flex-1 rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none focus:border-accent focus:ring-2 focus:ring-accent/30"
                          disabled={busy}
                          onChange={(event) =>
                            setCredentialProfileRenameName(event.target.value)
                          }
                          value={credentialProfileRenameName}
                        />
                        <button
                          className="rounded-md bg-content-primary px-3 py-2 text-xs font-semibold text-surface-base disabled:opacity-60"
                          disabled={busy}
                          type="submit"
                        >
                          {t("Save name")}
                        </button>
                        <button
                          className="rounded-md border border-surface-border/30 px-3 py-2 text-xs text-content-secondary"
                          disabled={busy}
                          onClick={() => {
                            setCredentialProfileRenameId(null);
                            setCredentialProfileRenameName("");
                          }}
                          type="button"
                        >
                          {t("Cancel")}
                        </button>
                      </form>
                    ) : null}

                    <div className="flex flex-wrap gap-2">
                      <button
                        className="rounded-md border border-surface-border/30 px-2.5 py-1.5 text-xs text-content-secondary transition-colors hover:bg-surface-panel hover:text-content-primary disabled:cursor-not-allowed disabled:opacity-50"
                        disabled={busy || anotherProfileBusy}
                        onClick={() => void testCredentialProfile(profile)}
                        type="button"
                      >
                        {busy ? t("Working...") : t("Test")}
                      </button>
                      <button
                        className="rounded-md border border-surface-border/30 px-2.5 py-1.5 text-xs text-content-secondary transition-colors hover:bg-surface-panel hover:text-content-primary disabled:cursor-not-allowed disabled:opacity-50"
                        disabled={busy || anotherProfileBusy || renaming}
                        onClick={() => {
                          setCredentialProfileRenameId(profile.id);
                          setCredentialProfileRenameName(profile.name);
                          setCredentialProfilesError(null);
                          setCredentialProfilesNotice(null);
                        }}
                        type="button"
                      >
                        {t("Rename")}
                      </button>
                      <button
                        className="rounded-md border border-surface-border/30 px-2.5 py-1.5 text-xs text-content-secondary transition-colors hover:bg-surface-panel hover:text-content-primary disabled:cursor-not-allowed disabled:opacity-50"
                        disabled={busy || anotherProfileBusy}
                        onClick={() =>
                          void setCredentialProfileEnabled(
                            profile,
                            profile.status !== "active",
                          )
                        }
                        type="button"
                      >
                        {profile.status === "active"
                          ? t("Disable")
                          : t("Enable")}
                      </button>
                      <button
                        className="rounded-md border border-danger/30 px-2.5 py-1.5 text-xs text-danger transition-colors hover:bg-danger/10 disabled:cursor-not-allowed disabled:opacity-50"
                        disabled={busy || anotherProfileBusy}
                        onClick={() => void removeCredentialProfile(profile)}
                        type="button"
                      >
                        {t("Remove")}
                      </button>
                    </div>
                  </article>
                );
              })}
            </div>
          ) : null}

          {credentialProfilesLoadState === "error" ? (
            <button
              className="rounded-md border border-surface-border/30 px-3 py-2 text-xs text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary"
              onClick={() =>
                setCredentialProfilesRefreshAttempt((attempt) => attempt + 1)
              }
              type="button"
            >
              {t("Retry loading profiles")}
            </button>
          ) : null}
        </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "preferences"}
          >
            <div>
              <label
                className="text-sm font-medium text-content-primary"
                htmlFor="settings-language"
              >
                {t("Language")}
              </label>
              <p className="mt-1 text-sm text-content-secondary">
                {t("Choose the app language or follow the system setting.")}
              </p>
            </div>
            <select
              className="w-full max-w-xs rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none focus:border-accent focus:ring-2 focus:ring-accent/30"
              id="settings-language"
              onChange={(event) =>
                onLanguageChange(event.target.value as LanguagePreference)
              }
              value={languagePreference}
            >
              <option value="system">{t("System")}</option>
              {localeDefinitions.map((locale) => (
                <option key={locale.code} value={locale.code}>
                  {locale.label}
                </option>
              ))}
            </select>
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "preferences"}
          >
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Theme")}
              </h4>
              <p className="mt-1 text-sm text-content-secondary">
                {t("Choose a fixed palette or follow the system appearance.")}
              </p>
            </div>
            <div className="inline-flex rounded-md border border-surface-border/30 bg-surface-raised/65 p-1">
              {themePreferences.map((theme) => (
                <button
                  aria-pressed={themePreference === theme}
                  className={`rounded px-3 py-1.5 text-sm capitalize transition-colors duration-150 focus:outline-none focus:ring-2 focus:ring-accent/30 ${
                    themePreference === theme
                      ? "bg-accent text-accent-contrast"
                      : "text-content-secondary hover:bg-surface-panel hover:text-content-primary"
                  }`}
                  key={theme}
                  onClick={() => onThemeChange(theme)}
                  type="button"
                >
                  {themePreferenceLabel(theme, t)}
                </button>
              ))}
            </div>
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "preferences"}
          >
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Personality")}
              </h4>
              <p className="mt-1 text-sm text-content-secondary">
                {t("Choose the default tone for agent responses.")}
              </p>
            </div>
            <div className="grid gap-2 sm:grid-cols-3">
              {(["none", "friendly", "pragmatic"] as const).map((option) => (
                <button
                  aria-pressed={personality === option}
                  className={`rounded-md border px-3 py-2 text-sm capitalize transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 ${
                    personality === option
                      ? "border-accent/60 bg-accent/10 text-content-primary"
                      : "border-surface-border/30 text-content-secondary hover:bg-surface-raised"
                  }`}
                  disabled={
                    preferencesLoadState !== "loaded" || personalitySavePending
                  }
                  key={option}
                  onClick={() => void savePersonality(option)}
                  type="button"
                >
                  {personalityLabel(option, t)}
                </button>
              ))}
            </div>
            {personalityError ? (
              <p className="text-xs text-danger" role="alert">
                {personalityError}
              </p>
            ) : null}
            {personalityNotice ? (
              <p className="text-xs text-accent" role="status">
                {personalityNotice}
              </p>
            ) : null}
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "preferences"}
          >
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Ponytail")}
              </h4>
              <p className="mt-1 text-sm text-content-secondary">
                {t(
                  "Lazy senior dev ruleset: the agent reuses what exists and writes the minimum code that works. Applies to new sessions and new chat turns. Off by default.",
                )}
              </p>
            </div>
            <div className="grid gap-2 sm:grid-cols-4">
              {(["off", "lite", "full", "ultra"] as const).map((option) => (
                <button
                  aria-pressed={ponytail === option}
                  className={`rounded-md border px-3 py-2 text-sm capitalize transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 ${
                    ponytail === option
                      ? "border-accent/60 bg-accent/10 text-content-primary"
                      : "border-surface-border/30 text-content-secondary hover:bg-surface-raised"
                  }`}
                  disabled={
                    preferencesLoadState !== "loaded" || ponytailSavePending
                  }
                  key={option}
                  onClick={() => void savePonytail(option)}
                  type="button"
                >
                  {ponytailLabel(option, t)}
                </button>
              ))}
            </div>
            {ponytailError ? (
              <p className="text-xs text-danger" role="alert">
                {ponytailError}
              </p>
            ) : null}
            {ponytailNotice ? (
              <p className="text-xs text-accent" role="status">
                {ponytailNotice}
              </p>
            ) : null}
          </section>

          <section
            className="space-y-5"
            hidden={activeCategory !== "jev"}
          >
            <div className="rounded-xl border border-accent/40 bg-accent/10 p-5">
              <p className="text-xs font-bold uppercase tracking-[0.24em] text-accent">
                JEV
              </p>
              <h4 className="mt-2 text-xl font-semibold text-content-primary">
                {t("Task routing")}
              </h4>
              <p className="mt-2 max-w-2xl text-sm leading-6 text-content-secondary">
                {t(
                  "Halv sends each task brief to a small decision model and picks the cheapest agent that can do it. The coordinator model is the ceiling. Briefs leave this computer.",
                )}
              </p>
            </div>
            <button
              aria-pressed={taskRouting}
              className={`flex w-full items-center justify-between rounded-md border px-3 py-2 text-left text-sm transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 ${
                taskRouting
                  ? "border-accent/60 bg-accent/10 text-content-primary"
                  : "border-surface-border/30 text-content-secondary hover:bg-surface-raised"
              }`}
              disabled={
                preferencesLoadState !== "loaded" || taskRoutingSavePending
              }
              onClick={() => void saveTaskRouting(!taskRouting)}
              type="button"
            >
              <span>{t("Route tasks to cheaper agents")}</span>
              <span className="text-xs font-semibold uppercase tracking-wide">
                {taskRouting ? t("On") : t("Off")}
              </span>
            </button>
            {taskRoutingError ? (
              <p className="text-xs text-danger" role="alert">
                {taskRoutingError}
              </p>
            ) : null}
            {taskRoutingNotice ? (
              <p className="text-xs text-accent" role="status">
                {taskRoutingNotice}
              </p>
            ) : null}
            <div className="space-y-2 border-t border-surface-border/25 pt-5">
              <h5 className="text-sm font-medium text-content-primary">
                {t("Ultracode")}
              </h5>
              <p className="text-sm text-content-secondary">
                {t(
                  "In a Claude Code Workflow, Halv routes each agent stage. Read agents can run on Sonnet, and a failed check can retry on a stronger model. Changes apply to new Claude panes.",
                )}
              </p>
            </div>
            {(
              [
                {
                  label: t("Route workflow agents"),
                  value: workflowRouting,
                  save: saveWorkflowRouting,
                },
                {
                  label: t("Start Claude panes in ultracode"),
                  value: claudeUltracode,
                  save: saveClaudeUltracode,
                },
              ] as const
            ).map((toggle) => (
              <button
                aria-pressed={toggle.value}
                className={`flex w-full items-center justify-between rounded-md border px-3 py-2 text-left text-sm transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 ${
                  toggle.value
                    ? "border-accent/60 bg-accent/10 text-content-primary"
                    : "border-surface-border/30 text-content-secondary hover:bg-surface-raised"
                }`}
                disabled={
                  preferencesLoadState !== "loaded" ||
                  workflowSavePending ||
                  !taskRouting
                }
                key={toggle.label}
                onClick={() => void toggle.save(!toggle.value)}
                type="button"
              >
                <span>{toggle.label}</span>
                <span className="text-xs font-semibold uppercase tracking-wide">
                  {toggle.value ? t("On") : t("Off")}
                </span>
              </button>
            ))}
            <p className="text-xs text-content-muted">
              {t(
                "Ultracode makes Claude Code use a workflow for most tasks. It uses more tokens. You can also turn it on in a pane with /effort ultracode on.",
              )}
            </p>
            {workflowError ? (
              <p className="text-xs text-danger" role="alert">
                {workflowError}
              </p>
            ) : null}
            {workflowNotice ? (
              <p className="text-xs text-accent" role="status">
                {workflowNotice}
              </p>
            ) : null}
            <div className="border-t border-surface-border/25 pt-5">
              <h5 className="text-sm font-medium text-content-primary">
                {t("Agent")}
              </h5>
              <p className="mt-1 text-sm text-content-secondary">
                {t("Choose the model and effort for each routing tier.")}
              </p>
              <p className="mt-2 text-xs text-content-muted">
                {t("Changes apply to new agent sessions. Max stays with your coordinator.")}
              </p>
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              {(["cheapest", "cheap", "medium", "strong"] as const).map(
                (tier) => {
                  const assignment = routingTierAgents[tier];
                  const options = routingTierModelOptions();
                  return (
                    <label
                      className="rounded-lg border border-surface-border/30 bg-surface-raised/45 p-3"
                      key={tier}
                    >
                      <span className="text-xs font-semibold uppercase tracking-wide text-content-muted">
                        {routingTierLabel(tier, t)}
                      </span>
                      <select
                        aria-label={`${routingTierLabel(tier, t)} ${t("Agent")}`}
                        className="mt-2 w-full rounded-md border border-surface-border/40 bg-surface-panel px-3 py-2 text-sm font-medium text-content-primary outline-none focus:border-accent focus:ring-2 focus:ring-accent/30"
                        disabled={
                          preferencesLoadState !== "loaded" ||
                          routingTierAgentsSavePending ||
                          !taskRouting
                        }
                        onChange={(event) =>
                          void saveRoutingTierAgentFromValue(
                            tier,
                            event.target.value,
                            options,
                            saveRoutingTierAgent,
                          )
                        }
                        value={routingTierModelValue(assignment)}
                      >
                        {options.map((option) => (
                          <option key={option.value} value={option.value}>
                            {option.assignment.lane === "auto"
                              ? t("Auto")
                              : `${workerLaneLabel(option.assignment.lane, t)} · ${option.assignment.model}${option.assignment.effort ? ` · ${option.assignment.effort}` : ""}`}
                          </option>
                        ))}
                      </select>
                      <span className="mt-2 block text-xs text-content-muted">
                        {assignment.lane === "auto"
                          ? t("Auto")
                          : workerLaneLabel(assignment.lane, t)}
                      </span>
                    </label>
                  );
                },
              )}
            </div>
            {routingTierAgentsError ? (
              <p className="text-xs text-danger" role="alert">
                {routingTierAgentsError}
              </p>
            ) : null}
            {routingTierAgentsNotice ? (
              <p className="text-xs text-accent" role="status">
                {routingTierAgentsNotice}
              </p>
            ) : null}
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "preferences"}
          >
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Claude subscription")}
              </h4>
            </div>
            <label className="flex items-center gap-2 rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary">
              <input
                aria-describedby="claude-subscription-proxy-warning"
                checked={claudeSubscriptionProxy}
                className="accent-accent"
                disabled={
                  preferencesLoadState !== "loaded" ||
                  claudeSubscriptionProxySavePending
                }
                onChange={(event) =>
                  void saveClaudeSubscriptionProxy(event.target.checked)
                }
                type="checkbox"
              />
              {t("Route Claude subscription through Halv Engine")}
            </label>
            <p
              className="text-xs leading-5 text-content-muted"
              id="claude-subscription-proxy-warning"
            >
              {t(
                "Halv Engine will compress your Claude Code traffic before it reaches Anthropic. Anthropic's consumer terms may not allow modifying subscription traffic; enable at your own risk. Only affects new terminal sessions.",
              )}
            </p>
            {claudeSubscriptionProxyError ? (
              <p className="text-xs text-danger" role="alert">
                {claudeSubscriptionProxyError}
              </p>
            ) : null}
            {claudeSubscriptionProxyNotice ? (
              <p className="text-xs text-accent" role="status">
                {claudeSubscriptionProxyNotice}
              </p>
            ) : null}
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "agent"}
          >
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Subagent concurrency")}
              </h4>
              <p
                className="mt-1 text-sm text-content-secondary"
                id="subagent-concurrency-description"
              >
                {t(
                  "Optionally cap how many child agents can run at once to help control usage. Leave this empty for Unlimited.",
                )}
              </p>
            </div>

            {preferencesLoadState === "loading" ? (
              <p className="text-xs text-content-muted" role="status">
                {t("Loading concurrency preference...")}
              </p>
            ) : null}

            <form
              className="flex flex-col gap-2 sm:flex-row sm:items-start"
              noValidate
              onSubmit={saveSubagentConcurrency}
            >
              <div className="min-w-0 flex-1">
                <label
                  className="sr-only"
                  htmlFor="subagent-concurrency-limit"
                >
                  {t("Maximum concurrent child agents")}
                </label>
                <input
                  aria-describedby="subagent-concurrency-description"
                  aria-invalid={preferencesError ? true : undefined}
                  autoComplete="off"
                  className="w-full rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none transition-colors duration-150 placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={
                    preferencesLoadState !== "loaded" || preferencesSavePending
                  }
                  id="subagent-concurrency-limit"
                  inputMode="numeric"
                  min={1}
                  onChange={(event) => {
                    setSubagentConcurrencyInput(event.target.value);
                    setPreferencesError(null);
                    setPreferencesNotice(null);
                  }}
                  placeholder={t("Unlimited")}
                  step={1}
                  type="number"
                  value={subagentConcurrencyInput}
                />
              </div>
              <button
                className="rounded-md bg-content-primary px-3 py-2 text-sm font-semibold text-surface-base transition-colors duration-150 hover:bg-content-secondary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60"
                disabled={
                  preferencesLoadState !== "loaded" ||
                  preferencesSavePending ||
                  subagentConcurrencyUnchanged
                }
                type="submit"
              >
                {preferencesSavePending ? t("Saving...") : t("Save limit")}
              </button>
            </form>

            {preferencesLoadState === "error" ? (
              <button
                className="w-fit rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
                onClick={() =>
                  setPreferencesLoadAttempt((attempt) => attempt + 1)
                }
                type="button"
              >
                {t("Retry loading")}
              </button>
            ) : null}

            {preferencesError ? (
              <p className="text-xs text-danger" role="alert">
                {preferencesError}
              </p>
            ) : null}
            {preferencesNotice ? (
              <p className="text-xs text-accent" role="status">
                {preferencesNotice}
              </p>
            ) : null}
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "agent"}
            ref={privacySectionRef}
          >
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Diagnostics & privacy")}
              </h4>
              <p className="mt-1 text-sm text-content-secondary">
                {t(
                  "Setup and usage milestones, app version, OS, and a random install ID. No prompts, code, emails, or file paths.",
                )}
              </p>
              <div className="mt-3 rounded-md border border-surface-border/30 px-3 py-2">
                <AnonymousUsagePreference description={false} />
              </div>
              <details className="mt-2 text-xs text-content-muted">
                <summary className="w-fit cursor-pointer font-medium text-content-secondary hover:text-content-primary">
                  {t("What is sent")}
                </summary>
                <ul className="mt-2 list-disc space-y-1 pl-5">
                  <li>{t("Installed")}</li>
                  <li>{t("CLI connected")}</li>
                  <li>{t("First saving")}</li>
                  <li>{t("Trial started")}</li>
                  <li>{t("Paid")}</li>
                </ul>
                <p className="mt-2">
                  {t("Install id")}: {" "}
                  <code className="break-all font-mono text-content-secondary">
                    {installId ?? t("Unavailable")}
                  </code>
                </p>
              </details>
              <details className="mt-2 text-xs text-content-muted">
                <summary className="w-fit cursor-pointer font-medium text-content-secondary hover:text-content-primary">
                  {t("What a support report includes")}
                </summary>
                <p className="mt-2 leading-relaxed">
                  {t(
                    "A support report includes the app version, operating system and architecture, local activation timestamps and launch count, aggregate record counts, and component health. It excludes prompts and message contents, secrets and credentials, account identifiers, project names, raw project paths, hostnames, device identifiers, and file contents.",
                  )}
                </p>
              </details>
            </div>

            <div className="flex flex-wrap gap-2">
              <button
                className="rounded-md bg-content-primary px-3 py-2 text-sm font-semibold text-surface-base transition-colors duration-150 hover:bg-content-secondary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60"
                disabled={diagnosticsPending || diagnosticsExportPending}
                onClick={() => void runLocalDiagnostics()}
                type="button"
              >
                {diagnosticsPending
                  ? t("Running...")
                  : t("Run local diagnostics")}
              </button>
              <button
                className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                disabled={diagnosticsPending || diagnosticsExportPending}
                onClick={() => void exportSupportReport()}
                type="button"
              >
                {diagnosticsExportPending
                  ? t("Exporting...")
                  : t("Export support report...")}
              </button>
            </div>
            <p className="text-xs text-content-muted">
              {t(
                "Export saves a JSON file locally so you can review it before choosing to share it.",
              )}
            </p>

            {diagnostics ? (
              <div className="space-y-3 rounded-md border border-surface-border/30 bg-surface-raised/45 p-3">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <p className="text-sm font-medium text-content-primary">
                    {t("Local health: {status}", {
                      status: diagnosticStatusLabel(
                        diagnostics.overallStatus,
                        t,
                      ),
                    })}
                  </p>
                  <p className="text-xs text-content-muted">
                    {formatDateTime(new Date(diagnostics.generatedAt))}
                  </p>
                </div>

                <div className="grid gap-2 sm:grid-cols-3">
                  {(
                    [
                      [t("Storage"), diagnostics.health.storage],
                      [t("Settings"), diagnostics.health.preferences],
                      [t("Proxy"), diagnostics.health.headroom],
                    ] as const
                  ).map(([label, component]) => (
                    <div
                      className="rounded-md border border-surface-border/25 bg-surface-panel/60 p-2.5"
                      key={label}
                    >
                      <div className="flex items-center justify-between gap-2">
                        <p className="text-xs font-medium text-content-primary">
                          {label}
                        </p>
                        <span
                          className={`text-[10px] font-semibold uppercase tracking-wide ${diagnosticStatusClass(component.status)}`}
                        >
                          {diagnosticStatusLabel(component.status, t)}
                        </span>
                      </div>
                      <p className="mt-1 text-xs leading-relaxed text-content-muted">
                        {component.summary}
                      </p>
                    </div>
                  ))}
                </div>

                {diagnostics.activation ? (
                  <div className="grid gap-2 text-xs text-content-secondary sm:grid-cols-3">
                    <p>
                      <span className="text-content-muted">
                        {t("First recorded")}
                      </span>
                      <br />
                      {formatDateTime(
                        new Date(diagnostics.activation.firstActivatedAt),
                      )}
                    </p>
                    <p>
                      <span className="text-content-muted">
                        {t("Last activation")}
                      </span>
                      <br />
                      {formatDateTime(
                        new Date(diagnostics.activation.lastActivatedAt),
                      )}
                    </p>
                    <p>
                      <span className="text-content-muted">
                        {t("Local launches")}
                      </span>
                      <br />
                      {diagnostics.activation.launchCount}
                    </p>
                  </div>
                ) : null}

                {diagnostics.localDataCounts ? (
                  <p className="text-xs text-content-muted">
                    {t(
                      "Aggregate local records: {conversations} conversations · {messages} messages · {projects} projects · {workspaces} workspaces · {memories} memories",
                      {
                        conversations:
                          diagnostics.localDataCounts.conversations,
                        messages: diagnostics.localDataCounts.messages,
                        projects: diagnostics.localDataCounts.projects,
                        workspaces: diagnostics.localDataCounts.workspaces,
                        memories: diagnostics.localDataCounts.memories,
                      },
                    )}
                  </p>
                ) : null}
              </div>
            ) : null}

            {diagnosticsError ? (
              <p className="text-xs text-danger" role="alert">
                {diagnosticsError}
              </p>
            ) : null}
            {diagnosticsNotice ? (
              <p className="text-xs text-accent" role="status">
                {diagnosticsNotice}
              </p>
            ) : null}
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "system"}
          >
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <h4 className="text-sm font-medium text-content-primary">
                  {t("Updates")}
                </h4>
                <p className="mt-1 text-sm text-content-secondary">
                  {t("Current version")}{" "}
                  <span className="font-mono text-content-primary">
                    {appVersion ?? t("unknown")}
                  </span>
                </p>
                {updateStatusLine ? (
                  <p
                    className={`mt-1 text-xs ${
                      updateStatusLine.tone === "danger"
                        ? "text-danger"
                        : updateStatusLine.tone === "accent"
                          ? "text-accent"
                          : "text-content-muted"
                    }`}
                    role="status"
                  >
                    {updateStatusLine.text}
                  </p>
                ) : null}
                {updateStatus?.state === "available" && updateSnapshot && !updateCanInstall ? (
                  <p className="mt-1 text-xs text-content-muted">
                    {appUpdateCannotInstallCopy(updateSnapshot.installMode, t)}
                  </p>
                ) : null}
              </div>
              <div className="flex flex-wrap gap-2">
                {updateStatus?.state === "ready" && !updateBlockers ? (
                  <button
                    className="rounded-md bg-accent px-3 py-2 text-sm font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
                    disabled={appUpdate.pending === "restart"}
                    onClick={() => void appUpdateStore.restart(false)}
                    type="button"
                  >
                    {t("Restart now")}
                  </button>
                ) : null}
                {updateStatus?.state === "available" && updateCanInstall ? (
                  <button
                    className="rounded-md bg-accent px-3 py-2 text-sm font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
                    disabled={appUpdate.pending === "download"}
                    onClick={() => void appUpdateStore.download()}
                    type="button"
                  >
                    {t("Download")}
                  </button>
                ) : null}
                <button
                  className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={
                    updateStatus?.state === "checking" ||
                    updateBusy ||
                    appUpdate.pending === "check"
                  }
                  onClick={() => void appUpdateStore.check()}
                  type="button"
                >
                  {updateStatus?.state === "checking"
                    ? t("Checking...")
                    : t("Check for updates")}
                </button>
              </div>
            </div>

            {updateStatus?.state === "downloading" ? (
              <div className="space-y-1.5">
                <div
                  aria-label={t("Update download progress")}
                  aria-valuemax={100}
                  aria-valuemin={0}
                  aria-valuenow={appUpdateProgressPercent(updateStatus) ?? undefined}
                  className="h-1.5 overflow-hidden rounded-full bg-surface-border/30"
                  role="progressbar"
                >
                  <div
                    className="h-full rounded-full bg-accent transition-[width] duration-150"
                    style={{
                      width: `${appUpdateProgressPercent(updateStatus) ?? 0}%`,
                    }}
                  />
                </div>
                <p className="font-mono text-xs tabular-nums text-content-muted">
                  {appUpdateProgressLabel(updateStatus, t, formatNumber)}
                </p>
              </div>
            ) : null}

            {updateBlockers && updateSnapshot ? (
              <div className="space-y-2 rounded-md border border-warning/30 bg-warning-surface/30 p-3 text-sm">
                <p className="text-xs text-content-primary">
                  {updateSnapshot.restartWhenIdle
                    ? t("Halv restarts when the work is done")
                    : t("Restarting now stops this work:")}
                </p>
                <ul className="list-disc space-y-0.5 pl-5 text-xs text-content-secondary">
                  {/* "Restart when done" waits only for replies and turns. */}
                  {appUpdateBlockerLines(updateBlockers, t, {
                    includeTerminals: !updateSnapshot.restartWhenIdle,
                  }).map((line) => (
                    <li key={line}>{line}</li>
                  ))}
                </ul>
                <div className="flex flex-wrap gap-2">
                  {!updateSnapshot.restartWhenIdle &&
                  updateBlockers.chatReplies + updateBlockers.guiAgentSessions > 0 ? (
                    <button
                      className="rounded-md bg-accent px-3 py-1.5 text-xs font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
                      disabled={appUpdate.pending === "restartWhenIdle"}
                      onClick={() => void appUpdateStore.restartWhenIdle()}
                      type="button"
                    >
                      {t("Restart when done")}
                    </button>
                  ) : null}
                  <button
                    className="rounded-md border border-surface-border/40 px-3 py-1.5 text-xs text-content-primary transition-colors duration-150 hover:bg-surface-raised focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60"
                    disabled={appUpdate.pending === "restart"}
                    onClick={() => void appUpdateStore.restart(true)}
                    type="button"
                  >
                    {t("Restart now")}
                  </button>
                  <button
                    className="rounded-md px-3 py-1.5 text-xs text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
                    onClick={() => void appUpdateStore.cancelRestart()}
                    type="button"
                  >
                    {t("Cancel")}
                  </button>
                </div>
              </div>
            ) : null}

            <label className="flex items-center gap-2 rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary">
              <input
                aria-describedby="app-update-auto-download-note"
                checked={updateSnapshot?.autoDownload ?? true}
                className="accent-accent"
                disabled={!updateSnapshot || appUpdate.pending === "autoDownload"}
                onChange={(event) =>
                  void appUpdateStore.setAutoDownload(event.target.checked)
                }
                type="checkbox"
              />
              {t("Download updates automatically")}
            </label>
            <p
              className="text-xs leading-5 text-content-muted"
              id="app-update-auto-download-note"
            >
              {t(
                "Halv downloads new versions in the background and asks before it restarts.",
              )}
            </p>

            {appUpdate.actionError ? (
              <p className="text-xs text-danger" role="alert">
                {appUpdateCommandErrorLabel(appUpdate.actionError, t)}
              </p>
            ) : null}
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "system"}
          >
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Savings queue")}
              </h4>
              <p className="mt-1 text-sm text-content-secondary">
                {savingsQueue
                  ? t("{unreported} unreported, {rejected} rejected", {
                      unreported: savingsQueue.unreported,
                      rejected: savingsQueue.rejected,
                    })
                  : t("Checking...")}
              </p>
              {savingsQueue?.lastError ? (
                <p className="mt-1 text-xs text-danger">
                  {t("Last error: {error}", { error: savingsQueue.lastError })}
                </p>
              ) : null}
            </div>
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "system"}
          >
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <h4 className="text-sm font-medium text-content-primary">
                  {t("Terminal supervisor")}
                </h4>
                <p className="mt-1 text-sm text-content-secondary">
                  {supervisorStatusLabel(supervisorStatus, t)}
                </p>
                {supervisorStatus?.available && supervisorStatus.supervisorStale ? (
                  <p className="mt-1 text-xs text-content-muted">
                    {t("It updates when no agent session runs.")}
                  </p>
                ) : null}
              </div>
              {supervisorStatus?.available &&
              supervisorStatus.supervisorStale &&
              !supervisorRestartConfirmation ? (
                <button
                  className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={supervisorRestartPending}
                  onClick={() => void restartSupervisor(false)}
                  type="button"
                >
                  {supervisorRestartPending
                    ? t("Restarting…")
                    : t("Restart supervisor")}
                </button>
              ) : null}
            </div>
            {supervisorRestartConfirmation ? (
              <div className="space-y-2 rounded-md border border-danger/30 bg-danger-surface/30 p-3">
                <p className="text-xs text-content-primary">
                  {supervisorRestartConfirmation.runningSessions === 1
                    ? t(
                        "Restarting the supervisor will end 1 running terminal session. This action cannot be undone.",
                      )
                    : t(
                        "Restarting the supervisor will end {count} running terminal sessions. This action cannot be undone.",
                        { count: supervisorRestartConfirmation.runningSessions },
                      )}
                </p>
                <div className="flex flex-wrap gap-2">
                  <button
                    className="rounded-md border border-danger/40 bg-danger-surface/55 px-3 py-1.5 text-xs font-semibold text-danger transition-colors duration-150 hover:bg-danger-surface focus:outline-none focus:ring-2 focus:ring-danger/40 disabled:cursor-not-allowed disabled:opacity-60"
                    disabled={supervisorRestartPending}
                    onClick={() => void restartSupervisor(true)}
                    type="button"
                  >
                    {supervisorRestartPending
                      ? t("Restarting…")
                      : t("End sessions and restart")}
                  </button>
                  <button
                    className="rounded-md px-3 py-1.5 text-xs text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60"
                    disabled={supervisorRestartPending}
                    onClick={() => {
                      setSupervisorRestartConfirmation(null);
                      setSupervisorRestartError(null);
                    }}
                    type="button"
                  >
                    {t("Cancel")}
                  </button>
                </div>
              </div>
            ) : null}
            {supervisorRestartError ? (
              <p className="text-xs text-danger" role="alert">
                {supervisorRestartError}
              </p>
            ) : null}
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "system"}
          >
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <h4 className="text-sm font-medium text-content-primary">
                  {t("Proxy")}
                </h4>
                <p className="mt-1 text-sm text-content-secondary">
                  {formatEngineStatus(sidecar)}
                </p>
              </div>
              <div className="flex flex-wrap gap-2">
                {headroomUpdateAvailable ? (
                  <button
                    className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                    disabled={headroomUpdatePending || restartPending}
                    onClick={() => void updateHeadroom()}
                    type="button"
                  >
                    {headroomUpdatePending
                      ? t("Updating...")
                      : t("Update proxy")}
                  </button>
                ) : null}
                <button
                  className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={
                    restartPending || sidecarInstalling || headroomUpdatePending
                  }
                  onClick={() => void restartSidecar()}
                  type="button"
                >
                  {restartPending ? t("Restarting...") : t("Restart")}
                </button>
              </div>
            </div>
            {showInstallPanel ? (
              <div
                className={`space-y-3 rounded-md border p-3 text-sm ${
                  installError
                    ? "border-danger/35 bg-danger-surface/45 text-danger"
                    : sidecar?.state === "notInstalled"
                      ? "border-warning/30 bg-warning-surface/45 text-warning"
                      : "border-accent/30 bg-accent/5 text-content-secondary"
                }`}
              >
                <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                  <div>
                    <p className="font-medium">
                      {sidecar?.state === "notInstalled"
                        ? t("The proxy is not installed.")
                        : sidecarInstalling
                          ? t("Halv is setting up its proxy.")
                          : t("The proxy is ready.")}
                    </p>
                    {sidecar?.state === "notInstalled" ? (
                      <p className="mt-1 opacity-85">
                        {t(
                          "Install automatically from Halv, or use the manual path if the installer cannot finish.",
                        )}
                      </p>
                    ) : sidecarInstalling ? (
                      <p className="mt-1 opacity-85">
                        {t("No action needed. Progress streams below.")}
                      </p>
                    ) : null}
                  </div>
                  {sidecar?.state === "notInstalled" ? (
                    <button
                      className="shrink-0 rounded-md bg-content-primary px-3 py-2 text-sm font-semibold text-surface-base transition-colors duration-150 hover:bg-content-secondary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60"
                      disabled={installPending}
                      onClick={() => void installHeadroom()}
                      type="button"
                    >
                      {installPending
                        ? t("Installing...")
                        : t("Retry automatic setup")}
                    </button>
                  ) : null}
                </div>

                {installLog.length > 0 ? (
                  <div className="max-h-44 overflow-y-auto rounded-md border border-surface-border/25 bg-surface-sunken/55 p-3 font-mono text-[11px] leading-relaxed text-content-secondary">
                    {installLog.map((line, index) => (
                      <div key={`${index}-${line}`}>{line}</div>
                    ))}
                  </div>
                ) : null}

                {installError ? (
                  <p className="text-xs opacity-90">{installError}</p>
                ) : null}

                {sidecar?.state === "notInstalled" ? (
                  <button
                    className="text-xs font-medium underline underline-offset-4 opacity-85 transition-opacity duration-150 hover:opacity-100"
                    onClick={() => setManualInstallOpen((current) => !current)}
                    type="button"
                  >
                    {showManualInstall
                      ? t("Hide manual install")
                      : t("Manual install")}
                  </button>
                ) : null}

                {sidecar?.state === "notInstalled" && showManualInstall ? (
                  <div className="space-y-3 rounded-md border border-current/20 bg-surface-sunken/35 p-3">
                    {sidecar.installHint ? (
                      <p className="text-xs opacity-75">{sidecar.installHint}</p>
                    ) : null}
                    <div className="flex flex-col gap-2 rounded-md border border-current/20 bg-surface-sunken/35 p-3 sm:flex-row sm:items-center sm:justify-between">
                      <code className="break-all font-mono text-sm">
                        {HEADROOM_INSTALL_COMMAND}
                      </code>
                      <button
                        className="shrink-0 rounded-md border border-current/30 px-3 py-2 text-sm font-medium transition-colors duration-150 hover:bg-surface-raised/40"
                        onClick={() => void copyInstallCommand()}
                        type="button"
                      >
                        {installCommandCopied ? t("Copied") : t("Copy")}
                      </button>
                    </div>
                    <p className="text-xs opacity-75">
                      {t("Fallback without pipx:")}{" "}
                      <code className="rounded bg-surface-sunken/50 px-1 py-0.5">
                        {HEADROOM_PIP_FALLBACK_COMMAND}
                      </code>
                    </p>
                    {copyError ? (
                      <p className="text-xs opacity-85">{copyError}</p>
                    ) : null}
                  </div>
                ) : null}
              </div>
            ) : null}
          </section>

          <section
            className="space-y-3 border-t border-surface-border/25 pt-5"
            hidden={activeCategory !== "system"}
          >
            <div>
              <h4 className="text-sm font-medium text-content-primary">
                {t("Halv Engine")}
              </h4>
              <p className="mt-1 text-sm text-content-muted">
                {t(
                  "Everything Halv Engine saves you, across all of its systems.",
                )}
              </p>
            </div>
            <SavingsDashboard
              sidecar={sidecar}
              stats={status.sidecarStats}
              today={todaySavings}
              totals={savingsTotals}
            />
          </section>
          </div>
        </div>
      </section>
    </div>
  );
}

// The supervisor keeps agent sessions alive across a restart (ADR 0008).
function supervisorStatusLabel(
  supervisor: TerminalSupervisorStatus | null,
  t: Translate,
) {
  if (!supervisor) {
    return t("Checking...");
  }
  if (supervisor.available) {
    return supervisor.supervisorVersion
      ? t("From Halv {version}. Agent sessions keep running when Halv restarts.", {
          version: supervisor.supervisorVersion,
        })
      : t("Running. Agent sessions keep running when Halv restarts.");
  }
  if (supervisor.protocolMismatch) {
    return t(
      "Agents from an older Halv are still running. Halv cannot connect to them.",
    );
  }
  return supervisor.message
    ? t("Not running. Terminals run inside Halv and close when Halv restarts.")
    : t("Not available on this system. Terminals run inside Halv.");
}

function diagnosticStatusLabel(
  status: SupportDiagnostics["overallStatus"],
  t: Translate,
) {
  switch (status) {
    case "healthy":
      return t("Healthy");
    case "attention":
      return t("Needs attention");
    case "unavailable":
      return t("Unavailable");
  }
}

function notificationModeLabel(
  mode: AppPreferences["completionNotifications"],
  t: Translate,
) {
  switch (mode) {
    case "never":
      return t("never");
    case "background":
      return t("background");
    case "always":
      return t("always");
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

function personalityLabel(
  personality: AppPreferences["personality"],
  t: Translate,
) {
  switch (personality) {
    case "none":
      return t("No preset");
    case "friendly":
      return t("friendly");
    case "pragmatic":
      return t("pragmatic");
  }
}

function workerLaneLabel(lane: AppPreferences["workerLane"], t: Translate) {
  switch (lane) {
    case "claude":
      return t("Claude");
    case "codex":
      return t("Codex");
    default:
      return t("Auto");
  }
}

function routingTierAgentsForLane(
  lane: Exclude<AppPreferences["workerLane"], "auto">,
): AppPreferences["routingTierAgents"] {
  return {
    cheapest: routingTierAssignment(lane, "cheapest"),
    cheap: routingTierAssignment(lane, "cheap"),
    medium: routingTierAssignment(lane, "medium"),
    strong: routingTierAssignment(lane, "strong"),
  };
}

function defaultRoutingTierAgents(): AppPreferences["routingTierAgents"] {
  return {
    cheapest: { ...DEFAULT_TIER_AGENTS.cheapest },
    cheap: { ...DEFAULT_TIER_AGENTS.cheap },
    medium: { ...DEFAULT_TIER_AGENTS.medium },
    strong: { ...DEFAULT_TIER_AGENTS.strong },
  };
}

function routingTierAssignment(
  lane: Exclude<AppPreferences["workerLane"], "auto">,
  tier: keyof AppPreferences["routingTierAgents"],
): TierAgentAssignment {
  const entry = DEFAULT_CATALOG[lane][tier];
  return { lane, model: entry.model, effort: entry.effort };
}

type RoutingTierModelOption = {
  value: string;
  assignment: TierAgentAssignment;
};

function routingTierModelValue(assignment: TierAgentAssignment): string {
  return assignment.lane === "auto"
    ? "auto"
    : `${assignment.lane}:${assignment.model ?? ""}:${assignment.effort ?? ""}`;
}

function routingTierModelOptions(): RoutingTierModelOption[] {
  const assignments: TierAgentAssignment[] = [
    { lane: "auto", model: null, effort: null },
  ];
  for (const lane of ["claude", "codex"] as const) {
    for (const tier of ["cheapest", "cheap", "medium", "strong"] as const) {
      const assignment = routingTierAssignment(lane, tier);
      if (
        !assignments.some(
          (candidate) =>
            routingTierModelValue(candidate) === routingTierModelValue(assignment),
        )
      ) {
        assignments.push(assignment);
      }
    }
  }
  return assignments.map((assignment) => ({
    value: routingTierModelValue(assignment),
    assignment,
  }));
}

function saveRoutingTierAgentFromValue(
  tier: keyof AppPreferences["routingTierAgents"],
  value: string,
  options: RoutingTierModelOption[],
  save: (
    tier: keyof AppPreferences["routingTierAgents"],
    assignment: TierAgentAssignment,
  ) => Promise<void>,
) {
  const option = options.find((candidate) => candidate.value === value);
  return option ? save(tier, option.assignment) : Promise.resolve();
}

function routingTierLabel(
  tier: keyof AppPreferences["routingTierAgents"],
  t: Translate,
) {
  switch (tier) {
    case "cheapest":
      return t("Cheapest");
    case "cheap":
      return t("Cheap");
    case "medium":
      return t("Medium");
    case "strong":
      return t("Strong");
  }
}

function ponytailLabel(ponytail: AppPreferences["ponytail"], t: Translate) {
  switch (ponytail) {
    case "off":
      return t("Off");
    case "lite":
      return t("Lite");
    case "full":
      return t("Full");
    case "ultra":
      return t("Ultra");
  }
}

function ponytailNoticeLabel(
  ponytail: AppPreferences["ponytail"],
  t: Translate,
) {
  switch (ponytail) {
    case "off":
      return t("Ponytail set to off.");
    case "lite":
      return t("Ponytail set to lite.");
    case "full":
      return t("Ponytail set to full.");
    case "ultra":
      return t("Ponytail set to ultra.");
  }
}

function credentialProfileStatusLabel(
  status: CredentialProfile["status"],
  t: Translate,
) {
  switch (status) {
    case "active":
      return t("active");
    case "disabled":
      return t("disabled");
    case "invalid":
      return t("invalid");
  }
}

function diagnosticStatusClass(
  status: SupportDiagnostics["overallStatus"],
) {
  return status === "healthy"
    ? "text-accent"
    : status === "attention"
      ? "text-warning"
      : "text-content-muted";
}

function formatCredentialProfileLastValidated(
  value: string | null,
  t: Translate,
) {
  if (!value) {
    return t("Never");
  }
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : formatDateTime(date);
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function parseInstallError(error: unknown): { message: string; details: string[] } {
  if (isSidecarInstallError(error)) {
    return {
      message: error.message,
      details: error.details.length > 0 ? error.details : [error.manualHint],
    };
  }
  return { message: errorMessage(error), details: [] };
}

function isSidecarInstallError(error: unknown): error is SidecarInstallError {
  return (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    "details" in error &&
    Array.isArray((error as SidecarInstallError).details)
  );
}
