//! Sanitized frontend boundary for experimental ChatGPT cleanup.
use crate::chatgpt_cleanup::{ChatGptManager, ChatGptStatus};
use crate::chatgpt_responses::ChatGptError;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

fn manager(app: &AppHandle) -> Arc<ChatGptManager> {
    app.state::<Arc<ChatGptManager>>().inner().clone()
}
pub async fn emit_status(app: &AppHandle) {
    let _ = app.emit("chatgpt-cleanup-status", manager(app).status().await);
}
async fn result_status(
    app: &AppHandle,
    result: Result<ChatGptStatus, ChatGptError>,
) -> Result<ChatGptStatus, ChatGptError> {
    if let Err(ref error) = result {
        manager(app).record_error(error.clone()).await;
    }
    emit_status(app).await;
    result
}
fn experimental(app: &AppHandle) -> Result<(), ChatGptError> {
    if !crate::settings::get_settings(app).experimental_enabled {
        return Err(ChatGptError::new(
            "disabled",
            "Enable experimental features before using ChatGPT cleanup.",
        ));
    }
    Ok(())
}
#[tauri::command]
#[specta::specta]
pub async fn chatgpt_cleanup_status(app: AppHandle) -> ChatGptStatus {
    manager(&app).status().await
}
#[tauri::command]
#[specta::specta]
pub async fn chatgpt_cleanup_login(app: AppHandle) -> Result<(), ChatGptError> {
    experimental(&app)?;
    let manager = manager(&app);
    let (attempt, generation) = manager.begin_login().await?;
    manager.check_generation(generation)?;
    if app
        .opener()
        .open_url(attempt.authorization_url(), None::<&str>)
        .is_err()
    {
        manager.cancel();
        let error = ChatGptError::new(
            "offline",
            "Could not open the default browser for ChatGPT sign-in.",
        );
        manager.record_error(error.clone()).await;
        emit_status(&app).await;
        return Err(error);
    }
    emit_status(&app).await;
    tauri::async_runtime::spawn(async move {
        if manager.finish_login(attempt, generation).await.is_ok() {
            if let Err(error) = manager.refresh_models().await {
                manager.record_error(error).await;
            }
        }
        emit_status(&app).await;
    });
    Ok(())
}
#[tauri::command]
#[specta::specta]
pub async fn chatgpt_cleanup_cancel_login(app: AppHandle) -> Result<(), ChatGptError> {
    manager(&app).cancel();
    emit_status(&app).await;
    Ok(())
}
#[derive(Serialize, specta::Type)]
pub struct ChatGptLogout {
    pub remote_revoked: bool,
}
#[tauri::command]
#[specta::specta]
pub async fn chatgpt_cleanup_logout(app: AppHandle) -> Result<ChatGptLogout, ChatGptError> {
    let result = manager(&app).logout().await;
    if let Err(ref error) = result {
        manager(&app).record_error(error.clone()).await;
    }
    emit_status(&app).await;
    result.map(|remote_revoked| ChatGptLogout { remote_revoked })
}
#[tauri::command]
#[specta::specta]
pub async fn chatgpt_cleanup_refresh_models(app: AppHandle) -> Result<ChatGptStatus, ChatGptError> {
    experimental(&app)?;
    result_status(&app, manager(&app).refresh_models().await).await
}
#[tauri::command]
#[specta::specta]
pub async fn chatgpt_cleanup_configure(
    app: AppHandle,
    model: String,
    service_tier: String,
) -> Result<ChatGptStatus, ChatGptError> {
    experimental(&app)?;
    result_status(&app, manager(&app).configure(model, service_tier).await).await
}
#[tauri::command]
#[specta::specta]
pub async fn chatgpt_cleanup_acknowledge_transcript_sharing(
    app: AppHandle,
) -> Result<ChatGptStatus, ChatGptError> {
    experimental(&app)?;
    result_status(&app, manager(&app).acknowledge().await).await
}

#[tauri::command]
#[specta::specta]
pub async fn chatgpt_cleanup_dismiss_welcome(
    app: AppHandle,
) -> Result<ChatGptStatus, ChatGptError> {
    result_status(&app, manager(&app).dismiss_welcome().await).await
}
