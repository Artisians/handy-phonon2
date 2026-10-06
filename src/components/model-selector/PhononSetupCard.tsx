import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { useModelStore } from "@/stores/modelStore";
import { Button } from "../ui/Button";

export const PHONON_MODEL_ID = "phonon-2-local";

interface PhononSetupCardProps {
  selected: boolean;
  busy: boolean;
  disabled: boolean;
  onSelect: (modelId: string) => void;
}

/** The selectable adapter is bundled; its separately managed service is not. */
export const PhononSetupCard: React.FC<PhononSetupCardProps> = ({
  selected,
  busy,
  disabled,
  onSelect,
}) => {
  const { t } = useTranslation();
  const [attempted, setAttempted] = useState(false);
  const error = useModelStore((state) => state.error);

  return (
    <section className="rounded-xl border-2 border-mid-gray/20 px-4 py-3 space-y-3 text-left">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-base font-semibold">{t("phonon.title")}</h3>
        <span className="text-xs text-text/60">{t("phonon.external")}</span>
        {selected && (
          <span className="text-xs text-logo-primary">
            {t("phonon.selected")}
          </span>
        )}
      </div>
      <p className="text-sm text-text/70">{t("phonon.description")}</p>
      <p className="text-xs text-text/60">{t("phonon.statusNote")}</p>
      <details className="text-sm">
        <summary className="cursor-pointer text-logo-primary">
          {t("phonon.setup")}
        </summary>
        <div className="mt-2 space-y-2 text-text/70">
          <p>{t("phonon.install")}</p>
          <pre className="whitespace-pre-wrap break-all rounded bg-mid-gray/10 p-2 text-xs select-text">
            {t("phonon.installCommands")}
          </pre>
          <p>{t("phonon.start")}</p>
          <pre className="whitespace-pre-wrap break-all rounded bg-mid-gray/10 p-2 text-xs select-text">
            {t("phonon.startCommand")}
          </pre>
          <p>{t("phonon.privacy")}</p>
          <p>{t("phonon.limits")}</p>
          <p>{t("phonon.attribution")}</p>
        </div>
      </details>
      <Button
        variant="secondary"
        size="sm"
        disabled={disabled || busy}
        onClick={() => {
          setAttempted(true);
          onSelect(PHONON_MODEL_ID);
        }}
      >
        {t(busy ? "phonon.checking" : "phonon.checkAndSelect")}
      </Button>
      {attempted && error && !busy && (
        <p role="alert" className="text-sm text-red-400 break-words">
          {error}
        </p>
      )}
    </section>
  );
};
