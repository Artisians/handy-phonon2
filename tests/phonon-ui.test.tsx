import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createInstance } from "i18next";
import { I18nextProvider } from "react-i18next";
import translations from "../src/i18n/locales/en/translation.json";
import ModelCard, {
  isLegacySource,
} from "../src/components/onboarding/ModelCard";
import { PHONON_MODEL_ID } from "../src/components/model-selector/PhononSetupCard";
import { ModelsSettings } from "../src/components/settings/models/ModelsSettings";
import Onboarding from "../src/components/onboarding/Onboarding";
import { useModelStore } from "../src/stores/modelStore";
import type { ModelInfo } from "../src/bindings";

const i18n = createInstance();
await i18n.init({
  lng: "en",
  resources: { en: { translation: translations } },
});
const model: ModelInfo = {
  id: PHONON_MODEL_ID,
  name: "Phonon-2",
  description: "Local English speech recognition",
  filename: "",
  source: "Local",
  size_mb: 635,
  is_downloaded: true,
  is_downloading: false,
  partial_size: 0,
  is_directory: false,
  engine_type: "Phonon2",
  accuracy_score: 0,
  speed_score: 0,
  supports_translation: false,
  is_recommended: false,
  supported_languages: ["en"],
  supports_language_selection: false,
  is_custom: false,
  supports_streaming: false,
  supports_language_detection: false,
};

const render = (
  status:
    | "available"
    | "active"
    | "switching"
    | "downloadable"
    | "downloading"
    | "extracting"
    | "verifying",
) =>
  renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <ModelCard
        model={{
          ...model,
          is_downloaded: ["active", "available", "switching"].includes(status),
        }}
        status={status}
        onSelect={() => {}}
        onDownload={() => {}}
        onDelete={() => {}}
        onCancel={() => {}}
        downloadProgress={42}
      />
    </I18nextProvider>,
  );

for (const status of [
  "available",
  "active",
  "switching",
  "downloadable",
  "downloading",
  "verifying",
  "extracting",
] as const) {
  const html = render(status);
  assert.match(html, /No separate setup/);
  assert.doesNotMatch(
    html,
    /External service|PowerShell|python.exe|pip install|fermion serve/,
  );
  assert.match(html, /recognition runs locally/);
}
assert.equal(
  isLegacySource({
    ...model,
    source: { Url: { url: "https://example.com/model", sha256: null } },
  }),
  false,
);
assert.match(render("active"), />Active</);
assert.match(render("available"), />Delete</);
assert.match(render("available"), /635 MB/);
assert.match(render("downloadable"), /role="button"/);
assert.doesNotMatch(render("downloadable"), />Delete</);
assert.match(render("downloading"), /42%/);
assert.match(render("downloading"), />Cancel</);
assert.match(render("verifying"), />Cancel</);
assert.match(render("extracting"), />Cancel</);
assert.match(render("switching"), /Starting Phonon-2/);
assert.match(render("switching"), />Cancel</);
console.log(
  "phonon-ui: normal download/select/delete and progress/cancel/setup states passed",
);

// Exercise the actual filtering/section rendering, not only the individual card.
for (const currentModel of ["", PHONON_MODEL_ID]) {
  // React SSR reads Zustand's initial snapshot; set it explicitly for each case.
  Object.assign(useModelStore.getInitialState(), {
    models: [
      {
        ...model,
        is_downloaded: false,
        is_recommended: true,
        source: { Url: { url: "https://example.com/model", sha256: null } },
      },
    ],
    currentModel,
    loading: false,
  });
  const settings = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <ModelsSettings />
    </I18nextProvider>,
  );
  assert.match(settings, /Phonon-2/);
  assert.match(settings, /role="button"/);
  assert.doesNotMatch(settings, />Active</);
  const onboarding = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <Onboarding onModelSelected={() => {}} />
    </I18nextProvider>,
  );
  assert.match(onboarding, /Phonon-2/);
  assert.match(onboarding, /role="button"/);
}
console.log(
  "phonon-ui: fresh and deleted/stale-selected Phonon remains downloadable in Models and onboarding",
);
