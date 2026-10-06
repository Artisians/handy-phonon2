// Compile and exercise the exact production filesystem and child-process
// installer without needing Tauri's desktop libraries on headless CI.
#[allow(dead_code)]
#[path = "../../../src-tauri/src/managers/model/phonon_install.rs"]
mod phonon_install;
