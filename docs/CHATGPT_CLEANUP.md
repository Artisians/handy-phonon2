# Experimental ChatGPT cleanup

This fork replaces the experimental cleanup provider controls with the official
**Sign in with ChatGPT** public-client flow. Speech recognition, including the
Phonon-2 adapter, is unchanged. Experimental features and cleanup remain off by
default. Existing legacy provider settings stay on disk but are neither migrated
into ChatGPT credentials nor used by the new cleanup route.

## Use and limits

1. Enable experimental features and open Post Processing.
2. Choose **Sign in with ChatGPT**. Your default browser handles OpenAI consent;
   the app accepts the callback on a temporary IPv4 loopback listener.
3. Acknowledge the welcome and the separate transcript-sharing disclosure.
4. Refresh the account's model catalog, choose a listed model, and choose
   Standard or Fast. There is no bundled or hard-coded model fallback.
5. Enable cleanup and choose the existing cleanup prompt. Only transcription
   text and cleanup instructions go to OpenAI, never raw microphone audio.

This is a **single-profile experimental implementation**. An issued registration
and its subject binding are retained after sign-out, and reauthorization must
match that identity. Switching to a different ChatGPT account is not implemented;
identity mismatch is an error, not an automatic remapping. Signing out clears the
local token bundle, selected model, requested Fast tier, and sharing approval.
The one-time welcome acknowledgement is retained.

Fast requests use `service_tier: "fast"`; Standard uses `"default"`. The service
may return another tier, which is shown separately. Availability and consumption
are controlled by the account and OpenAI, not guaranteed by this app. There is no
fallback to a paid API key or automatic retry with a different tier. A quota,
eligibility, or unsupported-tier failure pauses cleanup until explicit recovery
such as Refresh or changing the selection. Check ChatGPT Settings > Usage for
current limits; the app does not infer a reset time.

No real login, dynamic registration, account inference, or billing test was
performed during implementation. Official SIWC is a preview and account/region
eligibility must be tested interactively after a successful desktop build.

## Credential and request boundary

- Browser OAuth uses fresh state, nonce, and S256 PKCE; callback duplicates,
  mismatched state/client/identity, and replay are rejected.
- The issued client ID is saved before exchanging the authorization code.
- ID tokens require RS256/JWKS signature, issuer, audience, expiry, nonce,
  authorized-party and identity checks. Discovery endpoints stay on the official
  authentication origin and HTTP redirects are disabled.
- Windows saves a user-bound DPAPI-encrypted token bundle with atomic encrypted
  file replacement, avoiding Credential Manager's small per-item limit. macOS
  uses Keychain; Linux uses Secret Service. Missing/locked OS protection is an
  error. Tokens never use the ordinary settings file or the frontend boundary.
- A single mutex covers refresh and durable replacement. Cancellation rejects
  stale use while still persisting an already-successful refresh rotation.
- Sign-out attempts discovery-based remote revocation, then clears local tokens
  even if the remote attempt fails. The UI reports unconfirmed remote revocation.
- Cleanup refreshes `/v1/models` with the same account token immediately before
  the request, accepts only `visibility: "list"` models in server order, and
  sends text-only `POST /v1/responses` with `store: false`, `stream: true`.
- Streaming text is not pasted incrementally. Only a complete assistant text
  message in `response.completed` is eligible; failed/incomplete/refused,
  interrupted, malformed, or stale results retain the original transcription.
- Requests have bounded timeouts and response sizes. Ambiguous inference and
  rotating-token requests are not automatically retried. Error messages and
  logs exclude server bodies, transcripts, OAuth codes, URLs and token contents.

## Verification

The model-free harness imports the production Rust modules by path:

```sh
cargo test --locked --manifest-path tests/chatgpt-cleanup/Cargo.toml
cargo clippy --locked --manifest-path tests/chatgpt-cleanup/Cargo.toml --all-targets -- -D warnings
npm run test:chatgpt-ui
```

It uses synthetic credentials, signed test JWTs, memory-backed credential stores,
and localhost HTTP servers. It never reads the OS credential store or calls a
live OAuth/inference endpoint. Coverage includes callback binding/replay,
OIDC validation, terminal/transient refresh errors, serialized rotation,
cancellation races, remote revocation failure, model visibility/order, request
shape, Fast-tier errors, stream termination/refusal, and secret-safe state.

These tests do not establish that Windows DPAPI, the browser consent flow, or
full Tauri startup works on a real desktop. The hosted Windows compile/test gate
and an explicit interactive desktop acceptance test are separate required gates.

## Official references

- https://developers.openai.com/siwc/token-sharing-open-source/sign-in
- https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions
- https://developers.openai.com/siwc/token-sharing-open-source/token-reference
- https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference
- https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery
- https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations
- https://developers.openai.com/siwc/ui-ux-guidelines
