import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ChatGptServiceTier = "default" | "fast";
export interface ChatGptModel {
  slug: string;
  display_name: string;
  visibility: string;
}
export interface ChatGptError {
  code: string;
  message: string;
}
export interface ChatGptStatus {
  connected: boolean;
  login_pending: boolean;
  account: { email: string | null; name: string | null } | null;
  models: ChatGptModel[];
  selected_model: string | null;
  service_tier: ChatGptServiceTier;
  last_service_tier: string | null;
  last_error: ChatGptError | null;
  transcript_sharing_acknowledged: boolean;
  plan_usage_enabled: boolean;
  plan_usage_welcome_seen: boolean;
}
export type CleanupOperation =
  | "load"
  | "login"
  | "cancel"
  | "logout"
  | "refresh"
  | "configure"
  | "acknowledge"
  | "welcome";
export interface CleanupState {
  status: ChatGptStatus | null;
  busy: CleanupOperation | null;
  error: string | null;
  remoteRevocationPending: boolean;
}

// Preserve the account's discovery order. Never invent an eligible model or
// retain a model from the previous account when a fresh status arrives.
export function shouldShowChatGptWelcome(
  status: ChatGptStatus | null,
): boolean {
  return (
    !!status?.connected &&
    status.plan_usage_enabled &&
    !status.plan_usage_welcome_seen
  );
}

export function normalizeChatGptStatus(status: ChatGptStatus): ChatGptStatus {
  const models = status.connected
    ? status.models.filter((model) => model.visibility === "list")
    : [];
  return {
    ...status,
    account: status.connected ? status.account : null,
    models,
    selected_model: models.some((model) => model.slug === status.selected_model)
      ? status.selected_model
      : null,
  };
}

interface CleanupTransport {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  listen: (handler: (status: ChatGptStatus) => void) => Promise<UnlistenFn>;
}
const transport: CleanupTransport = {
  invoke,
  listen: (handler) =>
    listen<ChatGptStatus>("chatgpt-cleanup-status", (event) =>
      handler(event.payload),
    ),
};

// Only known error codes reach the view. Never render or log raw auth errors,
// which can contain callback URLs or other credential-bearing data.
export const cleanupErrorCodes = new Set([
  "offline",
  "auth_expired",
  "quota",
  "unsupported_tier",
  "no_models",
  "login_cancelled",
  "login_timeout",
  "not_connected",
  "sharing_not_acknowledged",
  "credential_store",
  "plan_permission_required",
  "disabled",
  "login_pending",
  "invalid_callback",
  "identity_mismatch",
  "invalid_token",
  "invalid_client",
  "timeout",
  "not_eligible",
  "unavailable",
  "unsupported_capability",
  "forbidden",
  "invalid_response",
  "incomplete_response",
  "invalid_model",
  "stale_result",
  "request_failed",
]);
function safeErrorCode(error: unknown): string {
  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    typeof error.code === "string" &&
    cleanupErrorCodes.has(error.code)
  ) {
    return error.code;
  }
  return "request_failed";
}

export function createCleanupController(api: CleanupTransport = transport) {
  let state: CleanupState = {
    status: null,
    busy: "load",
    error: null,
    remoteRevocationPending: false,
  };
  let revision = 0;
  let active = true;
  let lifecycle = 0;
  let operationSequence = 0;
  const subscribers = new Set<() => void>();
  const update = (patch: Partial<CleanupState>) => {
    if (!active) return;
    state = { ...state, ...patch };
    subscribers.forEach((subscriber) => subscriber());
  };
  const accept = (status: ChatGptStatus) => {
    revision += 1;
    update({
      status: normalizeChatGptStatus(status),
      error: null,
      ...(status.connected ? { remoteRevocationPending: false } : {}),
    });
  };
  const run = async (
    operation: CleanupOperation,
    command: string,
    args?: Record<string, unknown>,
  ) => {
    if (state.busy && !(operation === "cancel" && state.busy === "login"))
      return;
    const current = ++revision;
    const operationLifecycle = lifecycle;
    const currentOperation = ++operationSequence;
    update({ busy: operation, error: null });
    try {
      if (operation === "logout") {
        const result = await api.invoke<{ remote_revoked: boolean }>(command);
        if (
          operationLifecycle !== lifecycle ||
          currentOperation !== operationSequence
        )
          return;
        update({ remoteRevocationPending: !result.remote_revoked });
      } else if (operation === "login" || operation === "cancel") {
        await api.invoke(command, args);
      } else {
        const result = await api.invoke<ChatGptStatus>(command, args);
        if (current === revision && currentOperation === operationSequence)
          accept(result);
      }
      if (
        operationLifecycle !== lifecycle ||
        currentOperation !== operationSequence
      )
        return;
      // Read backend state even when an event beat the command response.
      if (
        operation === "login" ||
        operation === "cancel" ||
        operation === "logout"
      ) {
        const readRevision = revision;
        const result = await api.invoke<ChatGptStatus>(
          "chatgpt_cleanup_status",
        );
        if (readRevision === revision) accept(result);
      }
    } catch (error) {
      if (current === revision && currentOperation === operationSequence)
        update({ error: safeErrorCode(error) });
    } finally {
      if (
        operationLifecycle === lifecycle &&
        currentOperation === operationSequence &&
        state.busy === operation
      )
        update({ busy: null });
    }
  };
  return {
    getSnapshot: () => state,
    subscribe: (subscriber: () => void) => {
      subscribers.add(subscriber);
      return () => {
        subscribers.delete(subscriber);
      };
    },
    async start() {
      active = true;
      const currentLifecycle = ++lifecycle;
      let unlisten: UnlistenFn | undefined;
      try {
        unlisten = await api.listen((status) => {
          if (active && currentLifecycle === lifecycle) accept(status);
        });
        if (!active || currentLifecycle !== lifecycle) {
          unlisten();
          return () => {};
        }
        const current = revision;
        const result = await api.invoke<ChatGptStatus>(
          "chatgpt_cleanup_status",
        );
        if (current === revision) accept(result);
      } catch (error) {
        if (currentLifecycle === lifecycle)
          update({ error: safeErrorCode(error) });
      } finally {
        if (currentLifecycle === lifecycle) update({ busy: null });
      }
      return () => {
        unlisten?.();
      };
    },
    stop() {
      active = false;
      lifecycle += 1;
      revision += 1;
    },
    login: () => run("login", "chatgpt_cleanup_login"),
    cancel: () => run("cancel", "chatgpt_cleanup_cancel_login"),
    logout: () => run("logout", "chatgpt_cleanup_logout"),
    refresh: () => run("refresh", "chatgpt_cleanup_refresh_models"),
    dismissWelcome: () => run("welcome", "chatgpt_cleanup_dismiss_welcome"),
    acknowledge: () =>
      run("acknowledge", "chatgpt_cleanup_acknowledge_transcript_sharing"),
    configure: (model: string, serviceTier: ChatGptServiceTier) =>
      run("configure", "chatgpt_cleanup_configure", { model, serviceTier }),
  };
}
export type CleanupController = ReturnType<typeof createCleanupController>;
