//! Snapshot capture methods.

use chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams;

use crate::error::{Error, Result};
use crate::snapshot::{self, CursorItem, Snapshot, SnapshotOptions};

use super::Page;

impl Page {
    /// Capture an accessibility snapshot with default options.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP accessibility call fails.
    pub async fn snapshot(&self) -> Result<Snapshot> {
        self.snapshot_with(SnapshotOptions::default()).await
    }

    /// Capture an accessibility snapshot with custom options.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP accessibility call fails.
    pub async fn snapshot_with(&self, options: SnapshotOptions) -> Result<Snapshot> {
        let result = self
            .inner
            .execute(GetFullAxTreeParams::default())
            .await
            .map_err(Error::Cdp)?;

        let nodes: Vec<serde_json::Value> = serde_json::to_value(&result.result.nodes)
            .and_then(serde_json::from_value)
            .map_err(|e| Error::Snapshot(format!("failed to parse AX tree: {e}")))?;

        let mut snap = snapshot::build_snapshot(&nodes, &options);

        // Append cursor-interactive elements (cursor:pointer / onclick / tabindex)
        // that have no proper ARIA roles and were missed by the AX tree.
        if options.cursor_interactive {
            self.append_cursor_interactive_elements(&mut snap).await;
        }

        // Cache refs for subsequent ref-based interactions
        *self.refs.lock().await = snap.refs().clone();

        Ok(snap)
    }

    /// Detect elements with cursor:pointer / onclick / tabindex that lack ARIA
    /// roles and append them as extra refs to the snapshot.
    async fn append_cursor_interactive_elements(&self, snap: &mut Snapshot) {
        // JS finds elements that are cursor-interactive but not natively interactive
        let js = r"(() => {
            const interactive = new Set([
                'a','button','input','select','textarea','details','summary'
            ]);
            const results = [];
            for (const el of document.querySelectorAll('*')) {
                if (interactive.has(el.tagName.toLowerCase())) continue;
                const role = el.getAttribute('role');
                if (role && ['button','link','textbox','checkbox','radio',
                    'combobox','menuitem','option','tab','switch'].includes(role)) continue;
                const cs = getComputedStyle(el);
                const ptr = cs.cursor === 'pointer';
                const click = el.hasAttribute('onclick') || el.onclick !== null;
                const ti = el.getAttribute('tabindex');
                const tab = ti !== null && ti !== '-1';
                if (!ptr && !click && !tab) continue;
                if (ptr && !click && !tab) {
                    const p = el.parentElement;
                    if (p && getComputedStyle(p).cursor === 'pointer') continue;
                }
                const text = (el.textContent || '').trim().slice(0, 100);
                if (!text) continue;
                const r = el.getBoundingClientRect();
                if (r.width === 0 || r.height === 0) continue;
                const hints = [];
                if (ptr) hints.push('cursor:pointer');
                if (click) hints.push('onclick');
                if (tab) hints.push('tabindex');
                results.push({ text, hints: hints.join(', ') });
            }
            return JSON.stringify(results);
        })()";

        let Ok(val) = self.eval(js).await else {
            return;
        };
        let json_str = val.as_str().unwrap_or("[]");
        let Ok(items) = serde_json::from_str::<Vec<CursorItem>>(json_str) else {
            return;
        };

        snap.append_cursor_elements(&items);
    }
}
