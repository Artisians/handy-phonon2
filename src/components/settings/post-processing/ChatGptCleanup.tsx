import React, { useEffect, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Dialog } from "../../ui/Dialog";
import { Button } from "../../ui/Button";
import {
  createCleanupController,
  shouldShowChatGptWelcome,
  cleanupErrorCodes,
  type CleanupController,
  type CleanupState,
} from "../../../lib/chatGptCleanup";

interface CleanupViewProps {
  state: CleanupState;
  actions: Pick<
    CleanupController,
    "login" | "cancel" | "logout" | "refresh" | "configure" | "acknowledge"
  >;
  onManageUsage: () => void;
}

export function ChatGptCleanupView({
  state,
  actions,
  onManageUsage,
}: CleanupViewProps) {
  const { t } = useTranslation();
  const key = "settings.postProcessing.chatGpt";
  const { status, busy } = state;
  const connected = status?.connected ?? false;
  const pending = status?.login_pending || busy === "login";
  const disabled = !!busy || !!pending;
  const errorCode =
    state.error ||
    status?.last_error?.code ||
    (connected && !status?.plan_usage_enabled
      ? "plan_permission_required"
      : null);
  const error = errorCode
    ? cleanupErrorCodes.has(errorCode)
      ? errorCode
      : "request_failed"
    : null;
  const needsReauthentication =
    error === "plan_permission_required" ||
    error === "auth_expired" ||
    error === "invalid_token";
  const hasSelectedModel = !!status?.selected_model;
  const canConfigure = connected && !disabled;
  const canUseStandard =
    error === "unsupported_tier" &&
    hasSelectedModel &&
    status?.service_tier === "fast";

  return (
    <section
      className="p-4 space-y-4"
      aria-label={t(`${key}.title`)}
      aria-busy={!!busy}
    >
      <p className="text-sm text-mid-gray">{t(`${key}.experimental`)}</p>
      <p className="text-sm">{t(`${key}.sharing`)}</p>
      <div className="space-y-2" role="status" aria-live="polite">
        {busy === "load" ? (
          <p>{t(`${key}.loading`)}</p>
        ) : connected ? (
          <>
            <p className="font-medium">
              {t(
                `${key}.${!status?.plan_usage_enabled || needsReauthentication ? "signedIn" : "usingPlan"}`,
              )}
            </p>
            {(status?.account?.name || status?.account?.email) && (
              <p className="text-sm break-words">
                {status.account.name}
                {status.account.name && status.account.email ? " · " : ""}
                {status.account.email}
              </p>
            )}
          </>
        ) : (
          <p>{t(`${key}.loggedOut`)}</p>
        )}
        {pending && <p>{t(`${key}.loginPending`)}</p>}
        {busy && busy !== "load" && !pending && <p>{t(`${key}.working`)}</p>}
      </div>
      {error && (
        <div
          role="alert"
          className="rounded-lg border border-warning/40 p-3 text-sm space-y-2"
        >
          <p>{t(`${key}.errors.${error}`)}</p>
          <p>{t(`${key}.fallback`)}</p>
          {error === "quota" && (
            <Button onClick={onManageUsage}>{t(`${key}.manageUsage`)}</Button>
          )}
          {canUseStandard && (
            <Button
              variant="secondary"
              disabled={disabled}
              onClick={() =>
                void actions.configure(status!.selected_model!, "default")
              }
            >
              {t(`${key}.useStandard`)}
            </Button>
          )}
        </div>
      )}
      {state.remoteRevocationPending && (
        <p role="status" className="text-sm text-warning">
          {t(`${key}.localLogout`)}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        {(!connected || needsReauthentication) && !pending && (
          <Button
            onClick={() => void actions.login()}
            disabled={disabled}
            className="inline-flex items-center justify-center gap-3 min-h-10 px-4 rounded-full bg-black text-white border-black hover:bg-black/80 focus-visible:ring-2 focus-visible:ring-offset-2"
          >
            {/* Official asset: developers.openai.com/assets/siwc/sign-in-buttons/chatgpt-logo-white.svg */}
            <img
              src="/assets/chatgpt-logo-white.svg"
              alt=""
              aria-hidden="true"
              width={21}
              height={21}
            />
            {t(`${key}.login`)}
          </Button>
        )}
        {pending && (
          <Button
            variant="secondary"
            onClick={() => void actions.cancel()}
            disabled={!!busy && busy !== "login"}
          >
            {t(`${key}.cancelLogin`)}
          </Button>
        )}
        {connected && (
          <>
            <Button
              variant="secondary"
              onClick={() => void actions.logout()}
              disabled={disabled}
            >
              {t(`${key}.logout`)}
            </Button>
            <Button
              variant="secondary"
              onClick={() => void actions.refresh()}
              disabled={disabled}
            >
              {t(`${key}.refreshModels`)}
            </Button>
            <Button variant="ghost" onClick={onManageUsage}>
              {t(`${key}.manageUsage`)}
            </Button>
          </>
        )}
      </div>
      {connected && (
        <>
          {!status?.transcript_sharing_acknowledged ? (
            <label className="flex items-start gap-3 text-sm rounded-lg bg-mid-gray/5 p-3">
              <input
                type="checkbox"
                className="mt-1"
                checked={false}
                disabled={disabled}
                onChange={(event) => {
                  if (event.target.checked) void actions.acknowledge();
                }}
              />
              <span>{t(`${key}.acknowledge`)}</span>
            </label>
          ) : (
            <p className="text-xs text-mid-gray">{t(`${key}.acknowledged`)}</p>
          )}
          <div className="space-y-2">
            <label
              htmlFor="chatgpt-cleanup-model"
              className="text-sm font-medium"
            >
              {t(`${key}.model`)}
            </label>
            <select
              id="chatgpt-cleanup-model"
              value={status?.selected_model || ""}
              disabled={!canConfigure || !status?.models.length}
              className="block w-full rounded-lg border border-mid-gray/30 bg-background p-2 text-sm"
              onChange={(event) =>
                void actions.configure(
                  event.target.value,
                  status?.service_tier ?? "default",
                )
              }
            >
              <option value="" disabled>
                {t(`${key}.selectModel`)}
              </option>
              {status?.models.map((model) => (
                <option key={model.slug} value={model.slug}>
                  {model.display_name}
                </option>
              ))}
            </select>
            {!status?.models.length && (
              <p className="text-sm" role="status">
                {t(`${key}.noModels`)}
              </p>
            )}
            <p className="text-xs text-mid-gray">{t(`${key}.modelHelp`)}</p>
          </div>
          <div className="space-y-2">
            <label className="flex items-center gap-3 text-sm font-medium">
              <input
                type="checkbox"
                checked={status?.service_tier === "fast"}
                disabled={!canConfigure || !hasSelectedModel}
                aria-describedby="chatgpt-fast-warning"
                onChange={(event) =>
                  void actions.configure(
                    status!.selected_model!,
                    event.target.checked ? "fast" : "default",
                  )
                }
              />
              <span>{t(`${key}.fast`)}</span>
            </label>
            <p id="chatgpt-fast-warning" className="text-sm text-warning">
              {t(`${key}.fastWarning`)}
            </p>
            <p className="text-xs text-mid-gray">{t(`${key}.tierHelp`)}</p>
            {status?.last_service_tier && (
              <p className="text-xs" role="status">
                {t(`${key}.actualTier`, { tier: status.last_service_tier })}
              </p>
            )}
          </div>
        </>
      )}
    </section>
  );
}

export function ChatGptCleanup() {
  const [controller] = useState(createCleanupController);
  const state = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
  );
  const [linkError, setLinkError] = useState(false);
  const [welcomeDismissed, setWelcomeDismissed] = useState(false);
  const manageUsage = () => {
    setLinkError(false);
    void openUrl("https://chatgpt.com/settings/usage").catch(() =>
      setLinkError(true),
    );
  };
  const dismissWelcome = () => {
    if (state.busy) return;
    setWelcomeDismissed(true);
    void controller.dismissWelcome();
  };
  const { t } = useTranslation();
  const showWelcome =
    !welcomeDismissed && shouldShowChatGptWelcome(state.status);
  useEffect(() => {
    let cleanup: (() => void) | undefined;
    let disposed = false;
    void controller.start().then((stop) => {
      if (disposed) stop();
      else cleanup = stop;
    });
    return () => {
      disposed = true;
      controller.stop();
      cleanup?.();
    };
  }, [controller]);
  return (
    <>
      <ChatGptCleanupView
        state={state}
        actions={controller}
        onManageUsage={manageUsage}
      />
      <Dialog
        open={showWelcome}
        title={t("settings.postProcessing.chatGpt.welcomeTitle")}
        description={t("settings.postProcessing.chatGpt.welcomeDescription")}
        onOpenChange={(open) => {
          if (!open) dismissWelcome();
        }}
        closeLabel={t("settings.postProcessing.chatGpt.gotIt")}
        dismissible={!state.busy}
        footer={
          <Button onClick={dismissWelcome} disabled={!!state.busy}>
            {t("settings.postProcessing.chatGpt.gotIt")}
          </Button>
        }
      >
        <p className="text-sm">
          {t("settings.postProcessing.chatGpt.sharing")}
        </p>
        <Button variant="ghost" onClick={manageUsage}>
          {t("settings.postProcessing.chatGpt.manageUsage")}
        </Button>
        {linkError && (
          <p role="alert" className="text-sm">
            {t("settings.postProcessing.chatGpt.linkError")}
          </p>
        )}
      </Dialog>
      {linkError && !showWelcome && (
        <p role="alert" className="p-4 text-sm">
          {t("settings.postProcessing.chatGpt.linkError")}
        </p>
      )}
    </>
  );
}
