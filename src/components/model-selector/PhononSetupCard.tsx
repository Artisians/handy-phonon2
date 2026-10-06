import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../ui/Button";

export const PHONON_MODEL_ID = "phonon-2-local";

interface RuntimeStatus {
  state: "stopped" | "starting" | "ready" | "error";
  error: string | null;
}

interface PhononSetupCardProps {
  installed: boolean;
  busy: boolean;
  disabled: boolean;
  onSelect: (modelId: string) => void;
}

/** Runtime details sit inside the normal downloadable/selectable model card. */
export const PhononSetupCard: React.FC<PhononSetupCardProps> = ({
  installed,
  busy,
  disabled,
  onSelect,
}) => {
  const { t } = useTranslation();
  const [status, setStatus] = useState<RuntimeStatus | null>(null);
  const [cancelling, setCancelling] = useState(false);

  useEffect(() => {
    if (!installed && !busy) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      try {
        const result = await invoke<RuntimeStatus>("phonon_runtime_status");
        if (active) setStatus(result);
      } catch {
        if (active) setStatus(null);
      }
      if (active) timer = setTimeout(refresh, 1500);
    };
    void refresh();
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [installed, busy]);

  useEffect(() => {
    if (!busy) setCancelling(false);
  }, [busy]);

  const cancel = async () => {
    setCancelling(true);
    try {
      await invoke("cancel_phonon_start");
    } catch (error) {
      setStatus({ state: "error", error: String(error) });
      setCancelling(false);
    }
  };

  return (
    <section
      className="space-y-2 text-left"
      onClick={(event) => event.stopPropagation()}
      onKeyDown={(event) => event.stopPropagation()}
    >
      <p className="text-xs text-text/60">{t("phonon.statusNote")}</p>
      {installed && (
        <p role="status" className="text-xs text-text/70">
          {t(
            `phonon.runtime.${busy ? "starting" : (status?.state ?? "stopped")}`,
          )}
        </p>
      )}
      {busy && (
        <Button
          variant="danger-ghost"
          size="sm"
          disabled={cancelling}
          onClick={() => void cancel()}
        >
          {t(cancelling ? "phonon.cancelling" : "modelSelector.cancel")}
        </Button>
      )}
      {!busy && status?.error && installed && (
        <>
          <p role="alert" className="text-sm text-red-400 break-words">
            {status.error}
          </p>
          <Button
            variant="secondary"
            size="sm"
            disabled={disabled}
            onClick={() => onSelect(PHONON_MODEL_ID)}
          >
            {t("phonon.retry")}
          </Button>
        </>
      )}
      <details className="text-xs text-text/60">
        <summary className="cursor-pointer">{t("phonon.details")}</summary>
        <div className="mt-2 space-y-2">
          <p>{t("phonon.privacy")}</p>
          <p>{t("phonon.limits")}</p>
          <p>{t("phonon.attribution")}</p>
        </div>
      </details>
    </section>
  );
};
