import assert from "node:assert/strict";
import { build } from "esbuild";
import { chromium } from "@playwright/test";
import { readFile, readdir } from "node:fs/promises";

// A local, mock-only component harness. Never loads auth URLs or OpenAI APIs.
const bundle = await build({
  stdin: {
    contents: `
      import React, { useState } from 'react';
      import { createRoot } from 'react-dom/client';
      import { createInstance } from 'i18next';
      import { I18nextProvider } from 'react-i18next';
      import translations from './src/i18n/locales/en/translation.json';
      import { ChatGptCleanupView } from './src/components/settings/post-processing/ChatGptCleanup';
      const i18n = createInstance();
      const initial = { connected: false, login_pending: false, account: null,
        models: [], selected_model: null, service_tier: 'default', last_service_tier: null,
        last_error: null, transcript_sharing_acknowledged: false, plan_usage_enabled: true, plan_usage_welcome_seen: true };
      window.calls = [];
      function Harness() {
        const [state, setState] = useState({ status: initial, busy: null, error: null, remoteRevocationPending: false });
        window.show = (status, busy = null) => setState({status, busy, error: null, remoteRevocationPending: false});
        const actions = {
          login: async () => { window.calls.push('login'); setState(s => ({...s, status: {...s.status, login_pending: true}})); },
          cancel: async () => { window.calls.push('cancel'); setState(s => ({...s, status: {...s.status, login_pending: false}})); },
          logout: async () => { window.calls.push('logout'); setState(s => ({...s, status: initial})); },
          refresh: async () => { window.calls.push('refresh'); setState(s => ({...s, busy: 'refresh'})); },
          acknowledge: async () => { window.calls.push('acknowledge'); setState(s => ({...s, status: {...s.status, transcript_sharing_acknowledged: true}})); },
          configure: async (model, serviceTier) => { window.calls.push([model, serviceTier]); setState(s => ({...s, status: {...s.status, selected_model: model, service_tier: serviceTier, last_error: null}})); },
        };
        return <I18nextProvider i18n={i18n}><ChatGptCleanupView state={state} actions={actions} onManageUsage={() => window.calls.push('usage')} /></I18nextProvider>;
      }
      i18n.init({lng: 'en', resources: {en: {translation: translations}}}).then(() => createRoot(document.getElementById('root')).render(<Harness />));
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
  executablePath: process.env.CHROMIUM_PATH || "/usr/bin/chromium",
  headless: true,
  args: ["--no-sandbox"],
});
try {
  const page = await browser.newPage({
    viewport: { width: 900, height: 1000 },
  });
  await page.route("**/*", (route) => route.abort());
  await page.setContent(
    '<html><body><main id="root" style="max-width:720px;margin:auto"></main></body></html>',
  );
  const cssFile = (await readdir("dist/assets")).find((name) =>
    name.endsWith(".css"),
  );
  if (cssFile)
    await page.addStyleTag({
      content: await readFile(`dist/assets/${cssFile}`, "utf8"),
    });
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  await page.getByRole("button", { name: "Continue with ChatGPT" }).click();
  await page.getByRole("button", { name: "Cancel sign-in" }).click();
  await page.getByRole("button", { name: "Continue with ChatGPT" }).click();
  assert.deepEqual(await page.evaluate(() => window.calls), [
    "login",
    "cancel",
    "login",
  ]);
  const connected = {
    connected: true,
    login_pending: false,
    account: { name: "Test account", email: null },
    models: [
      {
        slug: "model-z",
        display_name: "First returned model",
        visibility: "list",
      },
      {
        slug: "model-a",
        display_name: "Second returned model",
        visibility: "list",
      },
    ],
    selected_model: "model-z",
    service_tier: "default",
    last_service_tier: null,
    last_error: null,
    transcript_sharing_acknowledged: false,
    plan_usage_enabled: true,
    plan_usage_welcome_seen: true,
  };
  await page.evaluate((status) => window.show(status), connected);
  await page.getByRole("checkbox", { name: /I understand/ }).check();
  await page
    .getByRole("combobox", { name: "Cleanup model" })
    .selectOption("model-a");
  assert.equal(
    await page.getByRole("checkbox", { name: "Fast", exact: true }).isChecked(),
    false,
  );
  await page.getByRole("checkbox", { name: "Fast", exact: true }).check();
  assert.deepEqual((await page.evaluate(() => window.calls)).slice(-3), [
    "acknowledge",
    ["model-a", "default"],
    ["model-a", "fast"],
  ]);
  await page.evaluate(
    (status) =>
      window.show({
        ...status,
        service_tier: "fast",
        last_error: { code: "unsupported_tier", message: "SECRET" },
      }),
    connected,
  );
  assert.equal(
    await page.getByRole("checkbox", { name: "Fast", exact: true }).isChecked(),
    true,
  );
  await page.getByRole("button", { name: "Use Standard" }).click();
  assert.equal(
    await page.getByRole("checkbox", { name: "Fast", exact: true }).isChecked(),
    false,
  );
  await page.getByRole("button", { name: "Refresh models" }).click();
  assert.equal(await page.getByRole("combobox").isDisabled(), true);
  assert.equal(
    await page
      .getByRole("checkbox", { name: "Fast", exact: true })
      .isDisabled(),
    true,
  );
  assert.equal(
    await page.getByRole("button", { name: "Log out" }).isDisabled(),
    true,
  );
  await page.evaluate(
    (status) =>
      window.show({
        ...status,
        account: { name: "Another account", email: null },
        models: [],
        selected_model: null,
      }),
    connected,
  );
  assert.equal(await page.getByRole("combobox").inputValue(), "");
  assert.equal(
    await page
      .getByText("No eligible cleanup models", { exact: false })
      .count(),
    1,
  );
  await page.evaluate((status) => window.show(status), connected);
  await page.screenshot({
    path: "/tmp/handy-chatgpt-cleanup-ui.png",
    fullPage: true,
  });
  await page.getByRole("button", { name: "Log out" }).click();
  assert.equal(
    await page.getByRole("button", { name: "Continue with ChatGPT" }).count(),
    1,
  );
  assert.equal(await page.getByRole("combobox").count(), 0);
  console.log(
    "chatgpt-cleanup-browser: mock login/cancel/retry, disclosure, model change, Fast opt-in/Standard fallback, busy controls, account change and logout passed",
  );
} finally {
  await browser.close();
}
