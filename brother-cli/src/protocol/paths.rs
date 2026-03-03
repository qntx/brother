//! Runtime directory and file path utilities for daemon port/pid files.

/// Runtime directory for daemon files (`~/.brother/`).
#[must_use]
pub fn runtime_dir() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|d| d.join("brother"))
}

/// Path to the daemon port file for a given session.
#[must_use]
pub fn port_file_path_for(session: &str) -> Option<std::path::PathBuf> {
    runtime_dir().map(|d| d.join(format!("{session}.port")))
}

/// Path to the daemon PID file for a given session.
#[must_use]
pub fn pid_file_path_for(session: &str) -> Option<std::path::PathBuf> {
    runtime_dir().map(|d| d.join(format!("{session}.pid")))
}
