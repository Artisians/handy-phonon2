import assert from "node:assert/strict";
import { build } from "esbuild";
import { chromium } from "@playwright/test";

// Actual model card/store with a local-only, controllable Tauri transport.
const bundle = await build({
  stdin: {
    contents: `
      import React, { useState } from 'react';
      import { createRoot } from 'react-dom/client';
      import { createInstance } from 'i18next';
      import { I18nextProvider } from 'react-i18next';
      import translations from './src/i18n/locales/en/translation.json';
      import ModelCard from './src/components/onboarding/ModelCard';
      import { useModelStore } from './src/stores/modelStore';
      const i18n = createInstance();
      window.calls = [];
      window.runtime = {state:'stopped',error:null};
      window.__TAURI_INTERNALS__ = {invoke: async (command) => {
        window.calls.push(command);
        if (command === 'phonon_runtime_status') return {...window.runtime};
        if (command === 'cancel_phonon_start') return new Promise(resolve => {window.finishCancel = () => { window.runtime = {state:'stopped',error:null}; window.show('available'); resolve(); }; });
        if (command === 'set_active_model') return new Promise(resolve => {window.finishSelection = resolve;});
        if (command === 'get_current_model') return 'actual-backend-selection';
        throw Error('Unexpected command '+command);
      }};
      window.store = useModelStore;
      const model = {id:'phonon-2-local',name:'Phonon-2',description:'English CPU dictation',filename:'phonon-2',source:'Local',size_mb:635,is_downloaded:false,is_downloading:false,partial_size:0,is_directory:true,engine_type:'Phonon2',accuracy_score:0,speed_score:0,supports_translation:false,is_recommended:false,supported_languages:['en'],supports_language_selection:false,is_custom:false,supports_streaming:false,supports_language_detection:false};
      function Harness() {
        const [status,setStatus] = useState('downloadable');
        window.show = setStatus;
        if (status === 'hidden') return null;
        return <I18nextProvider i18n={i18n}><ModelCard model={{...model,is_downloaded:['available','active','switching'].includes(status)}} status={status} downloadProgress={42}
          onDownload={() => {window.calls.push('download');setStatus('downloading');}}
          onCancel={() => {window.calls.push('cancel-download');setStatus('downloadable');}}
          onSelect={() => {window.calls.push('select');window.runtime={state:'starting',error:null};setStatus('switching');}}
          onDelete={() => {window.calls.push('delete');setStatus('downloadable');}} /></I18nextProvider>;
      }
      i18n.init({lng:'en',resources:{en:{translation:translations}}}).then(() => createRoot(document.getElementById('root')).render(<Harness/>));
    `,
    resolveDir: process.cwd(),
    loader: "tsx",
  },
  bundle: true,
  platform: "browser",
  format: "iife",
  write: false,
});
const browser = await chromium.launch({
  ...(process.env.CHROMIUM_PATH
    ? { executablePath: process.env.CHROMIUM_PATH }
    : process.platform === "linux"
      ? { executablePath: "/usr/bin/chromium" }
      : {}),
  headless: true,
  args: process.platform === "linux" ? ["--no-sandbox"] : [],
});
try {
  const page = await browser.newPage();
  await page.route("**/*", (route) => route.abort());
  await page.setContent('<html><body><main id="root"></main></body></html>');
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  await page.getByRole("heading", { name: "Phonon-2", exact: true }).waitFor();
  // Details are independent, while the descriptive center of the card selects.
  await page.getByText("Privacy and model details", { exact: true }).click();
  assert.equal(
    (await page.evaluate(() => window.calls)).includes("download"),
    false,
  );
  await page.getByText("Privacy and model details", { exact: true }).click();
  await page
    .getByText(
      "English · CPU · Windows x64. Download once, then Handy starts and stops Phonon-2 automatically. No separate setup.",
      { exact: true },
    )
    .click();
  assert.equal(
    (await page.evaluate(() => window.calls)).filter((x) => x === "download")
      .length,
    1,
  );
  // The download control has an aria-label distinct from its visible caption.
  await page
    .getByRole("button", { name: "Cancel download", exact: true })
    .click();
  await page.getByRole("heading", { name: "Phonon-2", exact: true }).click();
  assert.deepEqual(
    (await page.evaluate(() => window.calls)).filter(
      (x) => x === "download" || x === "cancel-download",
    ),
    ["download", "cancel-download", "download"],
  );
  for (const status of ["verifying", "extracting"]) {
    await page.evaluate((status) => window.show(status), status);
    await page.getByRole("button", { name: "Cancel", exact: true }).click();
  }
  await page.evaluate(() => window.show("available"));
  await page.getByRole("heading", { name: "Phonon-2", exact: true }).click();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  assert.equal(
    await page.getByRole("button", { name: "Cancelling…" }).isDisabled(),
    true,
  );
  assert.equal(
    (await page.evaluate(() => window.calls)).filter(
      (x) => x === "cancel_phonon_start",
    ).length,
    1,
  );
  await page.evaluate(() => window.finishCancel());
  await page.getByRole("heading", { name: "Phonon-2", exact: true }).click();
  await page.evaluate(() => {
    window.runtime = { state: "error", error: "Startup failed. Retry." };
    window.show("active");
  });
  await page.getByRole("button", { name: "Retry and select" }).click();
  await page.evaluate(() => {
    window.runtime = { state: "ready", error: null };
    window.show("active");
  });
  await page
    .getByText("Phonon-2 is running locally.", { exact: true })
    .waitFor();
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  assert.equal(
    await page.getByRole("button", { name: "Delete", exact: true }).count(),
    0,
  );
  // A newer selection from the backend wins; a delayed frontend completion must
  // never blindly restore the requested id. Duplicate selection is gated.
  await page.evaluate(() => {
    window.firstSelection = window.store
      .getState()
      .selectModel("phonon-2-local");
    window.secondSelection = window.store.getState().selectModel("other-model");
  });
  assert.equal(await page.evaluate(() => window.secondSelection), false);
  assert.equal(
    (await page.evaluate(() => window.calls)).filter(
      (x) => x === "set_active_model",
    ).length,
    1,
  );
  await page.evaluate(() => window.finishSelection(null));
  assert.equal(await page.evaluate(() => window.firstSelection), false);
  assert.equal(
    await page.evaluate(() => window.store.getState().currentModel),
    "actual-backend-selection",
  );
  assert.equal(
    await page.evaluate(() => window.store.getState().selectionPending),
    false,
  );
  await page.evaluate(() => window.show("active"));
  await page
    .getByText("Phonon-2 is running locally.", { exact: true })
    .waitFor();
  await page.evaluate(() => window.show("hidden"));
  const before = (await page.evaluate(() => window.calls)).length;
  await page.waitForTimeout(1700);
  assert.equal((await page.evaluate(() => window.calls)).length, before);
  console.log(
    "phonon-browser: download/cancel/retry, cancel during verify/setup, startup cancellation, error recovery, delete, duplicate/stale selection and unmount cleanup passed",
  );
} finally {
  await browser.close();
}
