import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createInstance } from "i18next";
import { I18nextProvider } from "react-i18next";
import translations from "../src/i18n/locales/en/translation.json";
import ModelCard from "../src/components/onboarding/ModelCard";
import { PHONON_MODEL_ID } from "../src/components/model-selector/PhononSetupCard";
import type { ModelInfo } from "../src/bindings";

const i18n = createInstance();
await i18n.init({
  lng: "en",
  resources: { en: { translation: translations } },
});
const model: ModelInfo = {
  id: PHONON_MODEL_ID,
  name: "Phonon-2 (local server)",
  description: "External service",
  filename: "",
  source: "Local",
  size_mb: 0,
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
  status: "available" | "active" | "switching" | "downloadable",
) =>
  renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <ModelCard
        model={model}
        status={status}
        onSelect={() => {}}
        onDownload={() => {
          throw new Error("Must never download the adapter");
        }}
        onDelete={() => {
          throw new Error("Must never delete the adapter");
        }}
      />
    </I18nextProvider>,
  );

for (const status of [
  "available",
  "active",
  "switching",
  "downloadable",
] as const) {
  const html = render(status);
  assert.match(html, /External service/);
  assert.match(html, /not installed by Handy/);
  assert.match(html, /does not continuously monitor availability/);
  assert.match(html, /127\.0\.0\.1/);
  assert.match(html, /fermion-research==0\.2\.7/);
  assert.doesNotMatch(html, />Delete</);
  assert.doesNotMatch(html, />Download</);
  assert.doesNotMatch(html, />0 MB</);
  assert.doesNotMatch(html, />Active</);
  assert.equal((html.match(/<button/g) || []).length, 1);
}
assert.match(render("active"), />Selected</);
assert.match(render("switching"), /disabled=""/);
assert.match(render("switching"), /Checking server/);
assert.match(render("available"), /Check and select/);
console.log("phonon-ui: 4 card states and external-service semantics passed");
