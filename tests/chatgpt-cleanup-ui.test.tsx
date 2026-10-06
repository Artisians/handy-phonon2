import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createInstance } from "i18next";
import { I18nextProvider } from "react-i18next";
import translations from "../src/i18n/locales/en/translation.json";
import { ChatGptCleanupView } from "../src/components/settings/post-processing/ChatGptCleanup";
import {
  createCleanupController,
  normalizeChatGptStatus,
  shouldShowChatGptWelcome,
  cleanupErrorCodes,
  type ChatGptStatus,
  type CleanupState,
} from "../src/lib/chatGptCleanup";

export const connectedStatus: ChatGptStatus = {
  connected: true,
  login_pending: false,
  account: { name: "Test account", email: "test@example.invalid" },
  models: [
    {
      slug: "model-z",
      display_name: "First returned model",
      visibility: "list",
    },
    {
      slug: "hidden-model",
      display_name: "Hidden model",
      visibility: "hidden",
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
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { resolve, promise };
};
async function main() {
  const i18n = createInstance();
  await i18n.init({
    lng: "en",
    resources: { en: { translation: translations } },
  });
  const actions = {
    login: async () => {},
    cancel: async () => {},
    logout: async () => {},
    refresh: async () => {},
    configure: async () => {},
    acknowledge: async () => {},
  };
  const render = (
    status: ChatGptStatus | null,
    patch: Partial<CleanupState> = {},
  ) =>
    renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <ChatGptCleanupView
          actions={actions}
          onManageUsage={() => {}}
          state={{
            status: status && normalizeChatGptStatus(status),
            busy: null,
            error: null,
            remoteRevocationPending: false,
            ...patch,
          }}
        />
      </I18nextProvider>,
    );
  const loggedOut = { ...connectedStatus, connected: false };
  assert.equal(
    shouldShowChatGptWelcome({
      ...connectedStatus,
      plan_usage_welcome_seen: false,
    }),
    true,
  );
  assert.equal(shouldShowChatGptWelcome(connectedStatus), false);
  assert.equal(
    shouldShowChatGptWelcome({
      ...connectedStatus,
      plan_usage_enabled: false,
      plan_usage_welcome_seen: false,
    }),
    false,
  );
  assert.equal(shouldShowChatGptWelcome(loggedOut), false);
  const loggedOutHtml = render(loggedOut);
  assert.match(loggedOutHtml, /Continue with ChatGPT/);
  assert.doesNotMatch(
    loggedOutHtml,
    /test@example|First returned model|API Key|type="password"/,
  );
  const html = render(connectedStatus);
  assert.match(html, /Using ChatGPT plan/);
  assert.match(html, /does not send raw audio/);
  assert.match(html, /I understand/);
  assert.match(html, /Fast consumes.*credits faster/);
  assert.doesNotMatch(html, /hidden-model|Hidden model/);
  assert.ok(html.indexOf('value="model-z"') < html.indexOf('value="model-a"'));
  assert.doesNotMatch(html, /type="checkbox"[^>]*checked/);
  assert.match(
    render({ ...connectedStatus, models: [], selected_model: null }),
    /No eligible cleanup models/,
  );
  assert.match(
    render({ ...connectedStatus, last_service_tier: "priority" }),
    /returned by OpenAI: priority/,
  );
  assert.doesNotMatch(render(connectedStatus), /returned by OpenAI:/);
  assert.match(
    render({ ...connectedStatus, transcript_sharing_acknowledged: true }),
    /Transcript sharing acknowledged/,
  );
  assert.doesNotMatch(
    render({ ...connectedStatus, transcript_sharing_acknowledged: true }),
    /I understand/,
  );
  const pending = render({ ...loggedOut, login_pending: true });
  assert.match(pending, /Cancel sign-in/);
  assert.doesNotMatch(pending, /Continue with ChatGPT/);
  assert.match(render(null, { busy: "load" }), /aria-busy="true"/);
  const busy = render(connectedStatus, { busy: "configure" });
  assert.match(busy, /<select[^>]*disabled/);
  assert.match(busy, /type="checkbox"[^>]*disabled/);
  for (const code of cleanupErrorCodes) {
    const result = render({
      ...connectedStatus,
      last_error: { code, message: "SECRET-TOKEN" },
    });
    assert.match(result, /role="alert"/);
    assert.doesNotMatch(result, /SECRET-TOKEN|settings\.postProcessing/);
  }
  const permissionNeeded = render({
    ...connectedStatus,
    last_error: { code: "plan_permission_required", message: "" },
  });
  assert.match(permissionNeeded, /Continue with ChatGPT/);
  assert.match(permissionNeeded, /Signed in with ChatGPT/);
  assert.doesNotMatch(permissionNeeded, /Using ChatGPT plan/);
  const identityOnly = render({
    ...connectedStatus,
    plan_usage_enabled: false,
  });
  assert.match(identityOnly, /Continue with ChatGPT/);
  assert.doesNotMatch(identityOnly, /Using ChatGPT plan/);
  const unsupported = render({
    ...connectedStatus,
    service_tier: "fast",
    last_error: { code: "unsupported_tier", message: "" },
  });
  assert.match(unsupported, /Use Standard/);
  assert.match(unsupported, /will not switch automatically/);
  assert.doesNotMatch(
    render({
      ...connectedStatus,
      last_error: { code: "unknown-SECRET", message: "SECRET-TOKEN" },
    }),
    /SECRET/,
  );
  assert.match(
    render(loggedOut, { remoteRevocationPending: true }),
    /Remote revocation could not be confirmed/,
  );

  let event!: (status: ChatGptStatus) => void;
  let server = normalizeChatGptStatus(connectedStatus);
  let refreshResult: ReturnType<typeof deferred<ChatGptStatus>> | null = null;
  const calls: string[] = [];
  let failCode: string | null = null;
  const controller = createCleanupController({
    listen: async (handler) => {
      event = handler;
      return () => {};
    },
    invoke: async <T,>(command: string): Promise<T> => {
      calls.push(command);
      if (failCode && command !== "chatgpt_cleanup_status")
        throw { code: failCode, message: "SECRET-TOKEN" };
      if (command === "chatgpt_cleanup_refresh_models" && refreshResult)
        return (await refreshResult.promise) as T;
      if (command === "chatgpt_cleanup_logout") {
        server = normalizeChatGptStatus(loggedOut);
        return { remote_revoked: false } as T;
      }
      return server as T;
    },
  });
  const stop = await controller.start();
  assert.equal(controller.getSnapshot().busy, null);
  assert.deepEqual(
    controller.getSnapshot().status?.models.map((model) => model.slug),
    ["model-z", "model-a"],
  );
  refreshResult = deferred<ChatGptStatus>();
  const refresh = controller.refresh();
  await controller.refresh();
  await controller.configure("model-a", "fast");
  assert.equal(
    calls.filter((call) => call === "chatgpt_cleanup_refresh_models").length,
    1,
  );
  assert.equal(calls.includes("chatgpt_cleanup_configure"), false);
  const newAccount = {
    ...server,
    account: { name: "New account", email: null },
    models: [],
    selected_model: "model-z",
  };
  event(newAccount);
  refreshResult.resolve(server);
  await refresh;
  assert.equal(controller.getSnapshot().status?.account?.name, "New account");
  assert.equal(controller.getSnapshot().status?.selected_model, null);
  assert.deepEqual(controller.getSnapshot().status?.models, []);
  refreshResult = null;
  failCode = "quota";
  await controller.refresh();
  assert.equal(controller.getSnapshot().error, "quota");
  failCode = "SECRET-TOKEN";
  await controller.refresh();
  assert.equal(controller.getSnapshot().error, "request_failed");
  failCode = null;
  await controller.logout();
  assert.equal(controller.getSnapshot().remoteRevocationPending, true);
  assert.equal(controller.getSnapshot().status?.account, null);
  assert.deepEqual(controller.getSnapshot().status?.models, []);
  controller.stop();
  stop();
  event(connectedStatus);
  assert.equal(controller.getSnapshot().status?.connected, false);

  // Cancelling a slow login then retrying must not let the old completion
  // clear the new request's busy state or launch a stale status read.
  const firstLogin = deferred<void>();
  const secondLogin = deferred<void>();
  let loginCount = 0;
  const cancellation = createCleanupController({
    listen: async () => () => {},
    invoke: async <T,>(command: string) => {
      if (command === "chatgpt_cleanup_login") {
        loginCount += 1;
        return (await (loginCount === 1
          ? firstLogin.promise
          : secondLogin.promise)) as T;
      }
      return loggedOut as T;
    },
  });
  await cancellation.start();
  const oldAttempt = cancellation.login();
  await cancellation.cancel();
  const newAttempt = cancellation.login();
  firstLogin.resolve();
  await oldAttempt;
  assert.equal(cancellation.getSnapshot().busy, "login");
  secondLogin.resolve();
  await newAttempt;
  assert.equal(cancellation.getSnapshot().busy, null);
  cancellation.stop();

  // Delayed listener registration after StrictMode cleanup must not read state
  // or leak a subscription into the new lifecycle.
  const delayedListener = deferred<() => void>();
  let unlistened = false;
  let staleReads = 0;
  const delayed = createCleanupController({
    listen: async () => delayedListener.promise,
    invoke: async <T,>() => {
      staleReads += 1;
      return loggedOut as T;
    },
  });
  const starting = delayed.start();
  delayed.stop();
  delayedListener.resolve(() => {
    unlistened = true;
  });
  await starting;
  assert.equal(unlistened, true);
  assert.equal(staleReads, 0);
  console.log(
    "chatgpt-cleanup-ui: render states, visibility/order, acknowledgement, Fast, busy locks, account changes, stale events, logout and safe errors passed",
  );
}
void main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
