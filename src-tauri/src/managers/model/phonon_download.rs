//! Download + reconstruction orchestration for the native model picker.
use super::download::HttpDownloadOutcome;
use super::{phonon_install as install, DownloadCleanup, ModelManager};
use anyhow::{bail, Result};
use hf_hub::api::tokio::CancellationToken;
use std::fs;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager};

impl ModelManager {
    pub fn is_phonon_operation_active(&self) -> bool {
        self.phonon_operation.load(Ordering::SeqCst)
    }

    pub(super) async fn download_phonon_model(&self) -> Result<()> {
        // Coalesce repeated clicks, including the period after Cancel while
        // its subprocess is stopping. Only one attempt owns cache and staging.
        let cancel = CancellationToken::new();
        let operation = {
            // Claim and register under the same mutex observed by Cancel, so
            // even a cancellation racing the initial click cannot be lost.
            let mut flags = self.cancel_flags.lock().unwrap();
            let Some(operation) = install::Operation::claim(&self.phonon_operation) else {
                return Ok(());
            };
            flags.insert(install::MODEL_ID.into(), cancel.clone());
            operation
        };
        let cleanup = DownloadCleanup {
            available_models: &self.available_models,
            cancel_flags: &self.cancel_flags,
            model_id: install::MODEL_ID.into(),
            disarmed: false,
        };
        let target = self.models_dir.join(install::DIRECTORY);
        if install::is_installed(&target) {
            drop(cleanup);
            self.update_download_status()?;
            let _ = self
                .app_handle
                .emit("model-download-complete", install::MODEL_ID);
            return Ok(());
        }
        if let Some(model) = self
            .available_models
            .lock()
            .unwrap()
            .get_mut(install::MODEL_ID)
        {
            model.is_downloading = true;
            model.is_downloaded = false;
        }
        let result = self.install_phonon(&cancel).await;
        // The blocking child has exited before cleanup or retry becomes legal.
        let stage_cleanup = install::clean_staging(&self.models_dir);
        self.extracting_models
            .lock()
            .unwrap()
            .remove(install::MODEL_ID);
        drop(cleanup);
        let result = result.and_then(|completed| stage_cleanup.map(|_| completed));
        self.update_download_status()?;
        let outcome = match result {
            Ok(true) => {
                let _ = self
                    .app_handle
                    .emit("model-extraction-completed", install::MODEL_ID);
                let _ = self
                    .app_handle
                    .emit("model-download-complete", install::MODEL_ID);
                Ok(())
            }
            Ok(false) => {
                let _ = self
                    .app_handle
                    .emit("model-download-cancelled", install::MODEL_ID);
                Ok(())
            }
            Err(error) => {
                let _ = self.app_handle.emit(
                    "model-extraction-failed",
                    serde_json::json!({
                        "model_id": install::MODEL_ID, "error": error.to_string()
                    }),
                );
                Err(error)
            }
        };
        // Release only after terminal events, so an older attempt cannot clear
        // a new attempt's frontend progress after an immediate retry.
        drop(operation);
        outcome
    }

    async fn install_phonon(&self, cancel: &CancellationToken) -> Result<bool> {
        let runtime = self.app_handle.path().resolve(
            "resources/phonon-runtime",
            tauri::path::BaseDirectory::Resource,
        )?;
        // Detect broken/non-Windows installs before downloading 164 MB.
        if !runtime.join("python/python.exe").is_file()
            || !runtime.join("prepare_model.py").is_file()
        {
            bail!("Bundled Phonon-2 runtime is missing. Install the Windows Handy Phonon build and retry Download.");
        }
        install::clean_staging(&self.models_dir)?;
        let archive = self.models_dir.join(install::ARCHIVE);
        match self
            .download_http_resumable(
                install::MODEL_ID,
                install::URL,
                &archive,
                Some(install::ARCHIVE_BYTES),
                Some(install::ARCHIVE_SHA256),
                cancel,
            )
            .await?
        {
            HttpDownloadOutcome::Cancelled => return Ok(false),
            HttpDownloadOutcome::Completed => {}
        }
        if cancel.is_cancelled() {
            return Ok(false);
        }
        self.extracting_models
            .lock()
            .unwrap()
            .insert(install::MODEL_ID.into());
        let _ = self
            .app_handle
            .emit("model-extraction-started", install::MODEL_ID);
        let staging = install::new_staging(&self.models_dir);
        let target = self.models_dir.join(install::DIRECTORY);
        let token = cancel.clone();
        tokio::task::spawn_blocking(move || -> Result<bool> {
            if !install::prepare(&runtime, &archive, &staging, || token.is_cancelled())? {
                return Ok(false);
            }
            if token.is_cancelled() {
                return Ok(false);
            }
            // Keep the verified immutable archive as a setup retry cache.
            install::finish_install(&staging, &target, || token.is_cancelled())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Phonon-2 setup task stopped: {e}"))?
    }

    pub(super) fn delete_phonon_model(&self) -> Result<()> {
        let _operation = install::Operation::claim(&self.phonon_operation).ok_or_else(|| {
            anyhow::anyhow!("Phonon-2 is being downloaded or set up. Cancel it before deleting.")
        })?;
        // The command layer serializes switching and stops the owned runtime
        // before reaching here, so Windows never removes an in-use model.
        let target = self.models_dir.join(install::DIRECTORY);
        if target.exists() {
            fs::remove_dir_all(target)?;
        }
        let archive = self.models_dir.join(install::ARCHIVE);
        if archive.exists() {
            fs::remove_file(archive)?;
        }
        install::clean_staging(&self.models_dir)?;
        self.update_download_status()?;
        let _ = self.app_handle.emit("model-deleted", install::MODEL_ID);
        Ok(())
    }
}
