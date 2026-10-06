//! The documented SIWC Responses route. Text only, no billing fallback or retries.
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
pub struct ChatGptError {
    pub code: String,
    pub message: String,
}
impl ChatGptError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn network(error: reqwest::Error) -> Self {
        // Never format reqwest errors: URLs, headers and response bodies can be sensitive.
        if error.is_timeout() {
            Self::new("timeout", "ChatGPT timed out. Original text was kept.")
        } else {
            Self::new(
                "offline",
                "Could not connect to ChatGPT. Original text was kept.",
            )
        }
    }
}
impl std::fmt::Display for ChatGptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code)
    }
}
impl std::error::Error for ChatGptError {}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
pub struct ChatGptModel {
    pub slug: String,
    pub display_name: String,
    pub visibility: String,
}

pub fn parse_models(body: &Value) -> Result<Vec<ChatGptModel>, ChatGptError> {
    let entries = body
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(invalid_response)?;
    let mut models = Vec::new();
    for entry in entries {
        if entry.get("visibility").and_then(Value::as_str) != Some("list") {
            continue;
        }
        let model: ChatGptModel =
            serde_json::from_value(entry.clone()).map_err(|_| invalid_response())?;
        if model.slug.is_empty() || model.display_name.is_empty() {
            return Err(invalid_response());
        }
        if !models.iter().any(|m: &ChatGptModel| m.slug == model.slug) {
            models.push(model);
        }
    }
    Ok(models)
}

pub fn request_body(
    model: &str,
    tier: &str,
    instructions: &str,
    text: &str,
) -> Result<Value, ChatGptError> {
    if !matches!(tier, "default" | "fast") {
        return Err(ChatGptError::new(
            "unsupported_tier",
            "Choose Standard or Fast.",
        ));
    }
    Ok(json!({ "model": model, "instructions": instructions,
        "input": [{"role": "user", "content": text}],
        "store": false, "stream": true, "service_tier": tier }))
}

pub fn api_error(status: u16, value: &Value) -> ChatGptError {
    let code = value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("");
    let param = value
        .pointer("/error/param")
        .and_then(Value::as_str)
        .unwrap_or("");
    match code {
        "subscription_sharing_usage_limit_exceeded" => ChatGptError::new(
            "quota",
            "ChatGPT plan usage limit reached. Check ChatGPT Settings > Usage.",
        ),
        "subscription_sharing_user_not_eligible" => ChatGptError::new(
            "not_eligible",
            "ChatGPT plan usage is unavailable for this account or workspace.",
        ),
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => {
            ChatGptError::new(
                "unavailable",
                "ChatGPT plan usage is temporarily unavailable. Try again later.",
            )
        }
        "subscription_sharing_unsupported_capability" if param == "service_tier" => {
            ChatGptError::new(
                "unsupported_tier",
                "Fast is unavailable for this request. Select Standard to try again.",
            )
        }
        "subscription_sharing_unsupported_capability" => ChatGptError::new(
            "unsupported_capability",
            "This model or request is not supported by ChatGPT plan usage.",
        ),
        "subscription_sharing_invalid_user" => ChatGptError::new(
            "auth_expired",
            "ChatGPT could not validate this account. Sign in again.",
        ),
        "chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context" => {
            ChatGptError::new(
                "plan_permission_required",
                "ChatGPT plan usage permission was not accepted.",
            )
        }
        _ => match status {
            401 => ChatGptError::new(
                "auth_expired",
                "ChatGPT authorization was not accepted. Sign in again.",
            ),
            403 => ChatGptError::new(
                "forbidden",
                "ChatGPT blocked this request because of account, policy, or region restrictions.",
            ),
            429 => ChatGptError::new(
                "quota",
                "ChatGPT usage is currently limited. Check ChatGPT Settings > Usage.",
            ),
            500..=599 => ChatGptError::new(
                "unavailable",
                "ChatGPT is temporarily unavailable. Try again later.",
            ),
            _ => invalid_response(),
        },
    }
}
fn invalid_response() -> ChatGptError {
    ChatGptError::new(
        "invalid_response",
        "ChatGPT returned an invalid response. Original text was kept.",
    )
}
fn incomplete() -> ChatGptError {
    ChatGptError::new(
        "incomplete_response",
        "ChatGPT did not complete the response. Original text was kept.",
    )
}

pub fn http_client() -> Result<reqwest::Client, ChatGptError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(ChatGptError::network)
}
pub async fn bounded_json(response: reqwest::Response) -> Result<Value, ChatGptError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ChatGptError::network)?;
        if body.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err(invalid_response());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| invalid_response())
}
pub async fn fetch_models(
    client: &reqwest::Client,
    token: &str,
) -> Result<Vec<ChatGptModel>, ChatGptError> {
    fetch_models_at(client, token, "https://api.openai.com/v1/models").await
}
async fn fetch_models_at(
    client: &reqwest::Client,
    token: &str,
    url: &str,
) -> Result<Vec<ChatGptModel>, ChatGptError> {
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(ChatGptError::network)?;
    let status = response.status().as_u16();
    let value = bounded_json(response).await?;
    if !(200..300).contains(&status) {
        return Err(api_error(status, &value));
    }
    parse_models(&value)
}

#[derive(Debug, PartialEq)]
pub struct CompletedResponse {
    pub text: String,
    pub service_tier: Option<String>,
}

/// Does not expose deltas. Nothing is eligible for paste until the completed event.
#[derive(Default)]
struct SseDecoder {
    pending: Vec<u8>,
    data: Vec<String>,
    deltas: String,
    total: usize,
    result: Option<CompletedResponse>,
}
impl SseDecoder {
    fn push(&mut self, bytes: &[u8]) -> Result<(), ChatGptError> {
        self.total += bytes.len();
        if self.total > 4 * 1024 * 1024 {
            return Err(invalid_response());
        }
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let mut raw: Vec<u8> = self.pending.drain(..=end).collect();
            raw.pop();
            if raw.last() == Some(&b'\r') {
                raw.pop();
            }
            let line = std::str::from_utf8(&raw).map_err(|_| invalid_response())?;
            if line.is_empty() {
                self.dispatch()?;
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data
                    .push(data.strip_prefix(' ').unwrap_or(data).into());
            }
        }
        Ok(())
    }
    fn dispatch(&mut self) -> Result<(), ChatGptError> {
        if self.data.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.data).join("\n");
        if data == "[DONE]" {
            return Ok(());
        }
        let event: Value = serde_json::from_str(&data).map_err(|_| invalid_response())?;
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                self.deltas.push_str(
                    event
                        .get("delta")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid_response)?,
                );
            }
            Some("response.completed") => {
                if self.result.is_some() {
                    return Err(invalid_response());
                }
                let response = event.get("response").ok_or_else(invalid_response)?;
                if response.get("status").and_then(Value::as_str) != Some("completed") {
                    return Err(incomplete());
                }
                let mut text = String::new();
                // Prefer the authoritative terminal response, not partial/reasoning/tool events.
                for output in response
                    .get("output")
                    .and_then(Value::as_array)
                    .ok_or_else(invalid_response)?
                {
                    if output.get("type").and_then(Value::as_str) != Some("message") {
                        continue;
                    }
                    if output.get("role").and_then(Value::as_str) != Some("assistant") {
                        continue;
                    }
                    if output
                        .get("status")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s != "completed")
                    {
                        return Err(incomplete());
                    }
                    for content in output
                        .get("content")
                        .and_then(Value::as_array)
                        .ok_or_else(invalid_response)?
                    {
                        match content.get("type").and_then(Value::as_str) {
                            Some("output_text") => text.push_str(
                                content
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .ok_or_else(invalid_response)?,
                            ),
                            Some("refusal") => return Err(incomplete()),
                            _ => {}
                        }
                    }
                }
                if text.trim().is_empty() {
                    return Err(incomplete());
                }
                self.result = Some(CompletedResponse {
                    text,
                    service_tier: response
                        .get("service_tier")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
            Some("response.failed") => {
                let value = json!({"error": event.pointer("/response/error").cloned().unwrap_or(Value::Null)});
                return Err(api_error(0, &value));
            }
            Some("response.incomplete") => return Err(incomplete()),
            Some("error") => return Err(api_error(0, &json!({"error": event}))),
            _ => {}
        }
        Ok(())
    }
    fn finish(self) -> Result<CompletedResponse, ChatGptError> {
        if !self.pending.is_empty() || !self.data.is_empty() {
            return Err(incomplete());
        }
        self.result.ok_or_else(incomplete)
    }
}

pub async fn cleanup(
    client: &reqwest::Client,
    token: &str,
    model: &str,
    tier: &str,
    instructions: &str,
    text: &str,
) -> Result<CompletedResponse, ChatGptError> {
    cleanup_at(
        client,
        token,
        model,
        tier,
        instructions,
        text,
        "https://api.openai.com/v1/responses",
    )
    .await
}
async fn cleanup_at(
    client: &reqwest::Client,
    token: &str,
    model: &str,
    tier: &str,
    instructions: &str,
    text: &str,
    url: &str,
) -> Result<CompletedResponse, ChatGptError> {
    let response = client
        .post(url)
        .bearer_auth(token)
        .header("Accept", "text/event-stream")
        .json(&request_body(model, tier, instructions, text)?)
        .send()
        .await
        .map_err(ChatGptError::network)?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        // Admission can be a non-JSON response: classify by status, never expose its text.
        let value = bounded_json(response).await.unwrap_or(Value::Null);
        return Err(api_error(status, &value));
    }
    if !response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.starts_with("text/event-stream"))
    {
        return Err(invalid_response());
    }
    let mut decoder = SseDecoder::default();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        decoder.push(&chunk.map_err(ChatGptError::network)?)?;
    }
    decoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn completed(text: &str) -> String {
        format!(
            "data: {}\n\n",
            json!({"type":"response.completed", "response":{"status":"completed","service_tier":"default","output":[{"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":text}]}]}})
        )
    }
    #[test]
    fn dynamic_models_preserve_server_order_and_hide_nonlisted() {
        let models = parse_models(&json!({"models":[{"slug":"z-new","display_name":"Z","visibility":"list"},{"slug":"hidden","display_name":"Secret","visibility":"hidden"},{"slug":"a-new","display_name":"A","visibility":"list"}]})).unwrap();
        assert_eq!(
            models.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>(),
            vec!["z-new", "a-new"]
        );
        assert!(parse_models(&json!({"data":[]})).is_err());
    }
    #[test]
    fn documented_body_only() {
        let body = request_body("account-model", "fast", "Fix punctuation", "hello").unwrap();
        assert_eq!(body.as_object().unwrap().len(), 6);
        assert_eq!(body["service_tier"], "fast");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert!(body["input"].is_array());
        assert!(request_body("x", "priority", "", "").is_err());
    }
    #[test]
    fn completed_is_required_and_utf8_can_span_chunks() {
        let mut decoder = SseDecoder::default();
        for b in completed("Привет").as_bytes() {
            decoder.push(&[*b]).unwrap();
        }
        let result = decoder.finish().unwrap();
        assert_eq!(result.text, "Привет");
        assert_eq!(result.service_tier.as_deref(), Some("default"));
        let mut incomplete = SseDecoder::default();
        incomplete
            .push(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n")
            .unwrap();
        assert_eq!(incomplete.finish().unwrap_err().code, "incomplete_response");
    }
    #[test]
    fn failure_after_delta_does_not_leak_partial() {
        let mut decoder = SseDecoder::default();
        let bytes = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"private partial\"}\n\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\",\"message\":\"secret\"}}}\n\n";
        let err = decoder.push(bytes).unwrap_err();
        assert_eq!(err.code, "quota");
        assert!(!format!("{err:?}").contains("secret"));
    }
    #[test]
    fn unsupported_tier_and_unstructured_admission() {
        assert_eq!(api_error(400, &json!({"error":{"code":"subscription_sharing_unsupported_capability","param":"service_tier"}})).code, "unsupported_tier");
        assert_eq!(
            api_error(403, &json!({"detail":"private server diagnosis"})).code,
            "forbidden"
        );
    }
    #[tokio::test]
    async fn mock_stream_terminal_event_and_request() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 8192];
            let count = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(request.contains("Bearer test-token"));
            assert!(request.contains("\"service_tier\":\"fast\""));
            let body = completed("Clean text.");
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
        });
        let result = cleanup_at(
            &http_client().unwrap(),
            "test-token",
            "dynamic-model",
            "fast",
            "Cleanup",
            "text",
            &url,
        )
        .await
        .unwrap();
        assert_eq!(result.text, "Clean text.");
        assert_eq!(result.service_tier.as_deref(), Some("default"));
        server.await.unwrap();
    }
    #[test]
    fn incomplete_refusal_and_truncated_terminal_never_return_text() {
        let mut decoder = SseDecoder::default();
        assert_eq!(
            decoder
                .push(b"data: {\"type\":\"response.incomplete\"}\n\n")
                .unwrap_err()
                .code,
            "incomplete_response"
        );
        let mut decoder = SseDecoder::default();
        let event = json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"refusal","refusal":"refused"}]}]}});
        assert_eq!(
            decoder
                .push(format!("data: {event}\n\n").as_bytes())
                .unwrap_err()
                .code,
            "incomplete_response"
        );
        let mut decoder = SseDecoder::default();
        let terminal = completed("text");
        decoder
            .push(&terminal.as_bytes()[..terminal.len() - 1])
            .unwrap();
        assert_eq!(decoder.finish().unwrap_err().code, "incomplete_response");
    }
}
