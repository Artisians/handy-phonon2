// Headless smoke entrypoint for the exact shipped subprocess and atomic
// publication code. It is not bundled in the user-facing app.
#[allow(dead_code)]
#[path = "../../../src-tauri/src/managers/model/phonon_install.rs"]
mod phonon_install;
use anyhow::{bail, Result};
use std::path::PathBuf;
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        bail!("usage: install <bundled-runtime> <archive> <model-target>");
    }
    let runtime = PathBuf::from(&args[0]);
    let archive = PathBuf::from(&args[1]);
    let target = PathBuf::from(&args[2]);
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("target needs a parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let staging = phonon_install::new_staging(parent);
    let result = (|| -> Result<()> {
        if !phonon_install::prepare(&runtime, &archive, &staging, || false)? {
            bail!("preparation cancelled");
        }
        if !phonon_install::finish_install(&staging, &target, || false)? {
            bail!("installation cancelled");
        }
        if !phonon_install::is_installed(&target) {
            bail!("installed model is not ready");
        }
        println!("PHONON_MODEL_INSTALL_OK {}", target.display());
        Ok(())
    })();
    phonon_install::clean_staging(parent)?;
    result
}
