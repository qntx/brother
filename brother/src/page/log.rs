//! Console and error log capture methods.

use super::{ConsoleEntry, JsError, Page};

impl Page {
    /// Return all captured console messages and clear the buffer.
    pub async fn take_console_logs(&self) -> Vec<ConsoleEntry> {
        std::mem::take(&mut *self.console_logs.lock().await)
    }

    /// Return all captured JS errors and clear the buffer.
    pub async fn take_js_errors(&self) -> Vec<JsError> {
        std::mem::take(&mut *self.js_errors.lock().await)
    }
}
