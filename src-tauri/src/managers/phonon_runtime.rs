//! Owns only the private Python process shipped with the Windows installer.
//! Never connects to or terminates an independently running Fermion service.
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Connection {
    pub endpoint: String,
    pub token: String,
}

#[derive(Clone, Serialize)]
pub struct RuntimeStatus {
    pub state: String,
    pub error: Option<String>,
}

struct ProcessState {
    child: Option<Child>,
    connection: Option<Connection>,
    status: RuntimeStatus,
}

pub struct PhononRuntime {
    root: PathBuf,
    startup: Mutex<()>,
    process: Mutex<ProcessState>,
    generation: AtomicU64,
    shutdown: AtomicBool,
    cancelled: AtomicBool,
    #[cfg(test)]
    fake_mode: Option<&'static str>,
    timeout: Duration,
}

impl PhononRuntime {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            startup: Mutex::new(()),
            process: Mutex::new(ProcessState {
                child: None,
                connection: None,
                status: RuntimeStatus {
                    state: "stopped".into(),
                    error: None,
                },
            }),
            generation: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            #[cfg(test)]
            fake_mode: None,
            timeout: Duration::from_secs(180),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ProcessState> {
        self.process.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn check_generation(&self, generation: u64) -> Result<()> {
        if self.shutdown.load(Ordering::SeqCst)
            || self.cancelled.load(Ordering::SeqCst)
            || self.generation.load(Ordering::SeqCst) != generation
        {
            bail!("Phonon-2 startup cancelled");
        }
        Ok(())
    }

    pub fn allow_start(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }

    pub fn status(&self) -> RuntimeStatus {
        let mut state = self.lock();
        if let Some(child) = state.child.as_mut() {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    state.child = None;
                    state.connection = None;
                    state.status = RuntimeStatus {
                        state: "error".into(),
                        error: Some(format!("Phonon-2 stopped ({exit}). Select Start and select to retry. If it keeps failing, reinstall Handy Phonon.")),
                    };
                }
                Err(_) => {
                    state.status.state = "error".into();
                    state.status.error = Some(
                        "Cannot check the bundled Phonon-2 process. Restart Handy Phonon.".into(),
                    );
                }
                Ok(None) => {}
            }
        }
        state.status.clone()
    }

    /// Serialized initialization. Cancellation/shutdown uses a separate mutex,
    /// so neither waits for a model load or the readiness deadline.
    pub fn ensure_running(&self, model_dir: &Path) -> Result<Connection> {
        let generation = self.generation.load(Ordering::SeqCst);
        let _startup = self.startup.lock().unwrap_or_else(|e| e.into_inner());
        self.check_generation(generation)?;
        if self.status().state == "ready" {
            if let Some(connection) = self.lock().connection.clone() {
                return Ok(connection);
            }
        }
        let result = self.start(generation, model_dir);
        if let Err(error) = &result {
            let mut state = self.lock();
            stop_owned(&mut state);
            if self.generation.load(Ordering::SeqCst) == generation {
                state.status = RuntimeStatus {
                    state: "error".into(),
                    error: Some(format!("{error:#}")),
                };
            }
        }
        result
    }

    fn start(&self, generation: u64, model_dir: &Path) -> Result<Connection> {
        let python = self.root.join("python/python.exe");
        let launcher = self.root.join("start_server.py");
        if !python.is_file() || !launcher.is_file() {
            bail!("The bundled Phonon-2 runtime is missing. Install the complete Windows x64 Handy Phonon installer, then try again. No separate Python setup is needed.");
        }
        if !model_dir.is_dir() {
            bail!(
                "Phonon-2 model files are missing. Download the model from Models, then try again."
            );
        }
        // A free ephemeral port avoids the user's services, including port 8010.
        // If another process wins the bind race, the wrapper exits 98 and we retry.
        // A random per-process secret also prevents audio being sent to that process.
        for attempt in 0..3 {
            self.check_generation(generation)?;
            let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
            let connection = Connection {
                endpoint: format!("http://127.0.0.1:{port}"),
                token: uuid::Uuid::new_v4().to_string(),
            };
            let mut command = Command::new(&python);
            command
                .arg("-I")
                .arg("-B")
                .arg(&launcher)
                .arg("--port")
                .arg(port.to_string())
                .arg("--parent-pid")
                .arg(std::process::id().to_string())
                .arg("--model-dir")
                .arg(model_dir)
                .current_dir(&self.root)
                .env("HANDY_PHONON_TOKEN", &connection.token)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(test)]
            if let Some(mode) = self.fake_mode {
                command = Command::new(std::env::current_exe()?);
                let module = module_path!()
                    .split_once("::")
                    .map(|(_, path)| path)
                    .unwrap_or("phonon_runtime");
                let test_name = format!("{module}::tests::fake_server");
                command
                    .args(["--ignored", "--exact", &test_name, "--nocapture"])
                    .env("HANDY_TEST_PORT", port.to_string())
                    .env("HANDY_TEST_MODE", mode)
                    .env("HANDY_PHONON_TOKEN", &connection.token)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000); // CREATE_NO_WINDOW; no shell/terminal.
            }
            {
                let mut state = self.lock();
                self.check_generation(generation)?;
                stop_owned(&mut state);
                state.status = RuntimeStatus {
                    state: "starting".into(),
                    error: None,
                };
                state.child = Some(command.spawn().context("Could not start bundled Phonon-2. Reinstall Handy Phonon if its files were removed or quarantined")?);
                state.connection = Some(connection.clone());
            }
            let started = Instant::now();
            loop {
                self.check_generation(generation)?;
                {
                    let mut state = self.lock();
                    let child = state.child.as_mut().context("Phonon-2 startup cancelled")?;
                    if let Some(exit) = child.try_wait()? {
                        state.child = None;
                        state.connection = None;
                        if exit.code() == Some(98) && attempt < 2 {
                            break;
                        }
                        bail!("Bundled Phonon-2 could not start ({exit}). Close memory-heavy apps and retry. If it keeps failing, reinstall the complete Handy Phonon installer.");
                    }
                }
                if super::phonon::check_owned_health(&connection).is_ok() {
                    let mut state = self.lock();
                    self.check_generation(generation)?;
                    if state
                        .child
                        .as_mut()
                        .context("Phonon-2 startup cancelled")?
                        .try_wait()?
                        .is_none()
                    {
                        state.status = RuntimeStatus {
                            state: "ready".into(),
                            error: None,
                        };
                        return Ok(connection);
                    }
                }
                if started.elapsed() >= self.timeout {
                    bail!("Phonon-2 did not become ready within 3 minutes. Close memory-heavy apps and select Start and select to retry.");
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
        bail!("Phonon-2 could not reserve a private local port. Retry startup.")
    }

    /// Cancel only a pending start, leaving an already-ready service alone.
    pub fn cancel_start(&self) {
        let mut state = self.lock();
        if state.status.state != "ready" {
            self.cancelled.store(true, Ordering::SeqCst);
            self.generation.fetch_add(1, Ordering::SeqCst);
            stop_owned(&mut state);
            state.status = RuntimeStatus {
                state: "stopped".into(),
                error: None,
            };
        }
    }

    /// Release model memory when switching/unloading, without preventing a later restart.
    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut state = self.lock();
        stop_owned(&mut state);
        state.status = RuntimeStatus {
            state: "stopped".into(),
            error: None,
        };
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut state = self.lock();
        stop_owned(&mut state);
        state.status = RuntimeStatus {
            state: "stopped".into(),
            error: None,
        };
    }
}

fn stop_owned(state: &mut ProcessState) {
    if let Some(mut child) = state.child.take() {
        // OS child handles, never taskkill/process names/PIDs found by port scan.
        let _ = child.kill();
        let _ = child.wait();
    }
    state.connection = None;
}

impl Drop for PhononRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::Arc;

    // Child fixture uses the test executable itself, so the lifecycle tests need
    // no Python installation on either Linux or a standard Windows runner.
    #[test]
    #[ignore]
    fn fake_server() {
        let Ok(port) = std::env::var("HANDY_TEST_PORT") else {
            return;
        };
        let mode = std::env::var("HANDY_TEST_MODE").unwrap();
        if mode == "exit" {
            std::process::exit(12);
        }
        if mode == "collision" {
            std::process::exit(98);
        }
        if mode == "slow" {
            std::thread::sleep(Duration::from_secs(30));
            return;
        }
        let listener = TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
        let token = std::env::var("HANDY_PHONON_TOKEN").unwrap();
        for socket in listener.incoming() {
            let mut socket = socket.unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = [0; 4096];
            let count = socket.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]);
            assert!(!request.to_lowercase().contains("authorization:"));
            let lower = request.to_lowercase();
            let challenge = lower
                .lines()
                .find_map(|line| line.strip_prefix("x-handy-phonon-challenge: "))
                .unwrap();
            use hmac::{Hmac, Mac};
            let mut mac = Hmac::<sha2::Sha256>::new_from_slice(token.as_bytes()).unwrap();
            mac.update(challenge.as_bytes());
            let proof: String = mac
                .finalize()
                .into_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let proof = if mode == "impostor" {
                challenge
            } else {
                &proof
            };
            let body = r#"{"status":"ok","kind":"speech","model":"FermionResearch/Phonon-2"}"#;
            write!(socket, "HTTP/1.1 200 OK\r\nX-Handy-Phonon-Proof: {proof}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    }

    fn fixture(mode: &'static str) -> (Arc<PhononRuntime>, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("Handy Phonon tests {}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("python")).unwrap();
        std::fs::create_dir(root.join("model with spaces")).unwrap();
        std::fs::write(root.join("python/python.exe"), b"test fixture").unwrap();
        std::fs::write(root.join("start_server.py"), b"test fixture").unwrap();
        let mut runtime = PhononRuntime::new(root.clone());
        runtime.fake_mode = Some(mode);
        runtime.timeout = if mode == "ready" {
            Duration::from_secs(10)
        } else {
            Duration::from_millis(700)
        };
        (Arc::new(runtime), root)
    }

    #[test]
    fn concurrent_start_reuses_one_owned_process_and_shutdown_reaps_it() {
        let (runtime, root) = fixture("ready");
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let runtime = runtime.clone();
                let model = root.join("model with spaces");
                std::thread::spawn(move || runtime.ensure_running(&model).unwrap().endpoint)
            })
            .collect();
        let endpoints: Vec<_> = handles
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(endpoints.iter().all(|endpoint| endpoint == &endpoints[0]));
        assert_eq!(runtime.status().state, "ready");
        runtime.cancel_start(); // Ready sessions are unaffected by late Cancel.
        assert_eq!(runtime.status().state, "ready");
        runtime.shutdown();
        assert!(runtime.lock().child.is_none());
        assert!(runtime
            .ensure_running(&root.join("model with spaces"))
            .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelling_pending_start_prevents_stale_success_and_allows_retry() {
        let (runtime, root) = fixture("slow");
        let worker_runtime = runtime.clone();
        let model = root.join("model with spaces");
        let worker = std::thread::spawn(move || worker_runtime.ensure_running(&model));
        while runtime.status().state != "starting" {
            std::thread::sleep(Duration::from_millis(5));
        }
        runtime.cancel_start();
        assert!(worker
            .join()
            .unwrap()
            .err()
            .unwrap()
            .to_string()
            .contains("cancelled"));
        assert_eq!(runtime.status().state, "stopped");
        assert!(runtime.lock().child.is_none());
        // A Cancel arriving before the launch worker starts is also respected.
        assert!(runtime
            .ensure_running(&root.join("model with spaces"))
            .is_err());
        runtime.allow_start();
        assert!(!runtime.cancelled.load(Ordering::SeqCst));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn timeout_wrong_identity_early_exit_and_collision_are_clean_retryable_errors() {
        for mode in ["slow", "impostor", "exit", "collision"] {
            let (runtime, root) = fixture(mode);
            assert!(runtime
                .ensure_running(&root.join("model with spaces"))
                .is_err());
            assert_eq!(runtime.status().state, "error");
            assert!(runtime.lock().child.is_none());
            runtime.allow_start();
            assert!(runtime
                .ensure_running(&root.join("model with spaces"))
                .is_err());
            assert!(runtime.lock().child.is_none());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn stop_leaves_unrelated_listener_alive_and_permits_restart() {
        let external = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let (runtime, root) = fixture("ready");
        let first = runtime
            .ensure_running(&root.join("model with spaces"))
            .unwrap();
        runtime.stop();
        assert_eq!(runtime.status().state, "stopped");
        assert!(external.local_addr().is_ok());
        let next = runtime
            .ensure_running(&root.join("model with spaces"))
            .unwrap();
        assert_ne!(first.token, next.token);
        runtime.stop();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_bundle_has_actionable_error_without_launching_external_python() {
        let runtime = PhononRuntime::new(PathBuf::from("/nonexistent Handy Phonon runtime"));
        let error = runtime
            .ensure_running(Path::new("/missing model"))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("installer"));
        assert!(runtime.lock().child.is_none());
    }
}
