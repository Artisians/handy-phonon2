//! Filesystem contract for Handy's managed Phonon installation.
//! No user Python, HF cache, or independently started service is touched.
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub(super) const MODEL_ID: &str = "phonon-2-local";
pub(super) const DIRECTORY: &str = "phonon-2";
pub(super) const ARCHIVE: &str = "phonon-2.bps.tar.zst.partial";
pub(super) const STAGING: &str = ".phonon-2-install";
pub(super) const URL: &str = "https://huggingface.co/FermionResearch/Phonon-2/resolve/160671c34ffeae4d80d6f86896c68e40aad971a7/phonon-2.bps.tar.zst";
pub(super) const ARCHIVE_BYTES: u64 = 163_515_201;
pub(super) const ARCHIVE_SHA256: &str =
    "98125795b6dda72f5c6eee9ba33d19815df65dcb18b50a357bf9f73c9935309e";
pub(super) const MODEL_BYTES: u64 = 177_438_361;
pub(super) const MODEL_SHA256: &str =
    "4b6bfa3a12cc3c4e0a54f2ab3ec4ca7a842b09e5c7ecfc8e7ca0ac6cc8c11468";
const MARKER: &str = ".handy-installed-v1";

/// Shared single-flight lease: covers setup, cancel cleanup, and delete.
pub(super) struct Operation<'a>(&'a std::sync::atomic::AtomicBool);
impl<'a> Operation<'a> {
    pub(super) fn claim(flag: &'a std::sync::atomic::AtomicBool) -> Option<Self> {
        flag.compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .ok()
        .map(|_| Self(flag))
    }
}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The stamp is written only after the complete reconstructed model was hashed.
/// Scan cheaply on startup; never infer readiness from an existing directory.
pub(super) fn is_installed(dir: &Path) -> bool {
    fs::read_to_string(dir.join(MARKER)).ok().as_deref() == Some(MODEL_SHA256)
        && dir.join("config.json").is_file()
        && dir.join("packed_manifest.json").is_file()
        && dir
            .join("model.fermion")
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() == MODEL_BYTES)
}

pub(super) fn stamp(dir: &Path) -> Result<()> {
    if !dir.join("config.json").is_file() || !dir.join("packed_manifest.json").is_file() {
        bail!("Phonon-2 setup produced incomplete model metadata. Retry Download.");
    }
    let mut file = fs::File::create(dir.join(MARKER))?;
    use std::io::Write;
    file.write_all(MODEL_SHA256.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn staging_paths(models: &Path) -> [PathBuf; 2] {
    [
        models.join(STAGING),
        models.join(format!("{STAGING}.partial")),
    ]
}

pub(super) fn new_staging(models: &Path) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    models.join(format!("{STAGING}-{}-{nonce}", std::process::id()))
}

pub(super) fn clean_staging(models: &Path) -> Result<()> {
    // Only app-owned hidden staging names; never user/custom model directories.
    let mut paths = staging_paths(models).to_vec();
    for entry in fs::read_dir(models)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(&format!("{STAGING}-"))
        {
            paths.push(entry.path());
        }
    }
    for path in paths {
        if path.exists() {
            if path.is_file() {
                fs::remove_file(&path)?;
                continue;
            }
            fs::remove_dir_all(&path).with_context(|| {
                format!(
                    "Cannot remove interrupted Phonon-2 setup at {}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Never remove a working install before its replacement is ready. Normal
/// retries only see absent/incomplete targets; a valid target is idempotent.
pub(super) fn publish(staging: &Path, target: &Path) -> Result<()> {
    if !is_installed(staging) {
        bail!("Phonon-2 setup is incomplete; refusing to install it.");
    }
    if is_installed(target) {
        fs::remove_dir_all(staging)?;
        return Ok(());
    }
    if target.exists() {
        fs::remove_dir_all(target)?;
    }
    fs::rename(staging, target).context("Cannot finish Phonon-2 installation")
}

/// Complete exactly the same post-process checks used by the model manager and
/// the installed/portable CI smoke harness. A process exit alone is not ready.
pub(super) fn finish_install(
    staging: &Path,
    target: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<bool> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    if cancelled() {
        return Ok(false);
    }
    let mut file = fs::File::open(staging.join("model.fermion"))?;
    let mut digest = Sha256::new();
    let mut bytes = [0_u8; 65536];
    loop {
        if cancelled() {
            return Ok(false);
        }
        let read = file.read(&mut bytes)?;
        if read == 0 {
            break;
        }
        digest.update(&bytes[..read]);
    }
    if format!("{:x}", digest.finalize()) != MODEL_SHA256 {
        bail!("Phonon-2 reconstructed model verification failed. Retry Download.");
    }
    drop(file);
    if cancelled() {
        return Ok(false);
    }
    stamp(staging)?;
    publish(staging, target)?;
    Ok(true)
}

struct OwnedPreparation(Child);
impl Drop for OwnedPreparation {
    fn drop(&mut self) {
        // kill+wait also runs on errors/panics, and never targets another PID.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Run the bundled isolated interpreter. Caller executes this off the UI and
/// async runtime threads, checks the final hash, and atomically publishes.
/// Returns false on cancellation; output is kept in a bounded-lifetime file
/// rather than a pipe that could deadlock a chatty subprocess.
pub(super) fn prepare(
    runtime: &Path,
    archive: &Path,
    staging: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<bool> {
    if cancelled() {
        return Ok(false);
    }
    let python = runtime.join("python/python.exe");
    let script = runtime.join("prepare_model.py");
    if !python.is_file() || !script.is_file() {
        bail!(
            "Bundled Phonon-2 setup files are missing. Reinstall Handy Phonon and retry Download."
        );
    }
    let log_path = staging.with_extension("setup.log");
    let output = fs::File::create(&log_path)?;
    let mut command = Command::new(&python);
    command
        .args(["-I", "-B"])
        .arg(script)
        .arg(archive)
        .arg(staging)
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .current_dir(runtime)
        .stdin(Stdio::null())
        .stdout(Stdio::from(output.try_clone()?))
        .stderr(Stdio::from(output));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let result = (|| -> Result<bool> {
        let mut child = OwnedPreparation(
            command
                .spawn()
                .context("Cannot start bundled Phonon-2 setup")?,
        );
        let started = Instant::now();
        loop {
            if cancelled() {
                return Ok(false);
            }
            if let Some(status) = child.0.try_wait()? {
                if !status.success() {
                    // Keep errors useful without returning unlimited upstream output.
                    use std::io::Read;
                    let mut detail = String::new();
                    if let Ok(log) = fs::File::open(&log_path) {
                        let _ = log.take(4096).read_to_string(&mut detail);
                    }
                    bail!(
                        "Phonon-2 setup failed ({status}). Retry Download. {}",
                        detail.trim()
                    );
                }
                return Ok(true);
            }
            if started.elapsed() > Duration::from_secs(15 * 60) {
                bail!("Phonon-2 setup timed out. Retry Download.");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    })();
    let _ = fs::remove_file(log_path);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn complete(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("config.json"), "{}").unwrap();
        fs::write(dir.join("packed_manifest.json"), "{}").unwrap();
        fs::File::create(dir.join("model.fermion"))
            .unwrap()
            .set_len(MODEL_BYTES)
            .unwrap();
        stamp(dir).unwrap();
    }
    #[test]
    fn unmarked_or_truncated_models_are_not_ready() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!is_installed(temp.path()));
        complete(temp.path());
        assert!(is_installed(temp.path()));
        fs::remove_file(temp.path().join(MARKER)).unwrap();
        assert!(!is_installed(temp.path()));
        stamp(temp.path()).unwrap();
        fs::write(temp.path().join("model.fermion"), "partial").unwrap();
        assert!(!is_installed(temp.path()));
    }
    #[test]
    fn publish_rejects_incomplete_and_preserves_active_model() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join(DIRECTORY);
        let staging = temp.path().join(STAGING);
        complete(&target);
        fs::create_dir_all(&staging).unwrap();
        assert!(publish(&staging, &target).is_err());
        assert!(is_installed(&target));
        complete(&staging);
        publish(&staging, &target).unwrap();
        assert!(is_installed(&target));
        assert!(!staging.exists());
    }
    #[test]
    fn interrupted_setup_cleanup_preserves_archive_and_installed_model() {
        let temp = tempfile::tempdir().unwrap();
        complete(&temp.path().join(DIRECTORY));
        fs::write(temp.path().join(ARCHIVE), "resumable").unwrap();
        for p in staging_paths(temp.path()) {
            fs::create_dir_all(p).unwrap();
        }
        clean_staging(temp.path()).unwrap();
        clean_staging(temp.path()).unwrap();
        assert!(is_installed(&temp.path().join(DIRECTORY)));
        assert!(temp.path().join(ARCHIVE).is_file());
        assert!(staging_paths(temp.path()).iter().all(|p| !p.exists()));
    }
    #[test]
    fn cancelled_setup_never_starts_missing_runtime() {
        assert!(!prepare(
            Path::new("missing"),
            Path::new("missing"),
            Path::new("missing"),
            || true
        )
        .unwrap());
    }
    #[test]
    fn repeated_clicks_and_delete_are_excluded_until_cleanup_releases_lease() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let active = AtomicBool::new(false);
        let lease = Operation::claim(&active).unwrap();
        assert!(Operation::claim(&active).is_none());
        assert!(active.load(Ordering::SeqCst));
        drop(lease);
        assert!(Operation::claim(&active).is_some());
        assert!(!active.load(Ordering::SeqCst));
    }
    #[test]
    fn failed_reconstruction_hash_never_publishes_or_stamps() {
        let temp = tempfile::tempdir().unwrap();
        let staging = new_staging(temp.path());
        let target = temp.path().join(DIRECTORY);
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("model.fermion"), "corrupt").unwrap();
        assert!(finish_install(&staging, &target, || false).is_err());
        assert!(!target.exists());
        assert!(!staging.join(MARKER).exists());
        assert!(!finish_install(&staging, &target, || true).unwrap());
    }
    #[test]
    fn unique_attempts_cannot_publish_over_another_attempts_staging() {
        let temp = tempfile::tempdir().unwrap();
        let first = new_staging(temp.path());
        let second = new_staging(temp.path());
        assert_ne!(first, second);
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        clean_staging(temp.path()).unwrap();
        assert!(!first.exists());
        assert!(!second.exists());
    }
    #[cfg(target_os = "linux")]
    fn mock_runtime(root: &Path, script: &str) {
        fs::create_dir_all(root.join("python")).unwrap();
        std::os::unix::fs::symlink("/usr/bin/python3", root.join("python/python.exe")).unwrap();
        fs::write(root.join("prepare_model.py"), script).unwrap();
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn cancellation_reaps_the_owned_setup_process_before_retry() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime with spaces");
        mock_runtime(&runtime, "import os, pathlib, sys, time\npathlib.Path(sys.argv[2] + '.pid').write_text(str(os.getpid()))\ntime.sleep(60)\n");
        let staging = new_staging(temp.path());
        let started = Instant::now();
        assert!(!prepare(
            &runtime,
            &temp.path().join("archive with spaces"),
            &staging,
            || started.elapsed() > Duration::from_millis(300)
        )
        .unwrap());
        let pid_file = PathBuf::from(format!("{}.pid", staging.display()));
        let pid = fs::read_to_string(&pid_file).unwrap();
        assert!(
            !PathBuf::from(format!("/proc/{pid}")).exists(),
            "child must be reaped before retry"
        );
        assert!(!staging.exists());
        assert!(!staging.with_extension("setup.log").exists());
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn failed_setup_has_actionable_error_and_retry_can_run() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        mock_runtime(&runtime, "raise RuntimeError('fixture failure')\n");
        let staging = new_staging(temp.path());
        let error = prepare(&runtime, &temp.path().join("archive"), &staging, || false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("fixture failure"));
        assert!(!staging.with_extension("setup.log").exists());
        fs::write(runtime.join("prepare_model.py"), "pass\n").unwrap();
        assert!(prepare(&runtime, &temp.path().join("archive"), &staging, || false).unwrap());
        assert!(!is_installed(&staging)); // exit 0 by itself never means ready
    }
}
