// Compile the production implementations, never copies; no audio model or GUI needed.
#[path = "../../../src-tauri/src/chatgpt_auth.rs"]
pub mod chatgpt_auth;
#[path = "../../../src-tauri/src/chatgpt_cleanup.rs"]
pub mod chatgpt_cleanup;
#[path = "../../../src-tauri/src/chatgpt_responses.rs"]
pub mod chatgpt_responses;
#[path = "../../../src-tauri/src/chatgpt_store.rs"]
pub mod chatgpt_store;
