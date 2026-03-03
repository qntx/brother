//! Snapshot capture methods.

use std::collections::HashSet;

use chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams;

use crate::error::{Error, Result};
use crate::snapshot::{self, CursorItem, Snapshot, SnapshotOptions};

use super::Page;

/// Filter an AX tree to only nodes belonging to the subtree rooted
/// at the node whose `backendDOMNodeId` matches `target_backend_id`.
///
/// Walks from the matching root and collects all descendants via `childIds`.
fn filter_subtree(all_nodes: &[serde_json::Value], target_backend_id: i64) -> Vec<serde_json::Value> {
    // Find the AX node whose backendDOMNodeId matches
    let root_node_id = all_nodes.iter().find_map(|n| {
        let bid = n.get("backendDOMNodeId").and_then(serde_json::Value::as_i64)?;
        if bid == target_backend_id {
            n.get("nodeId").and_then(serde_json::Value::as_str).map(ToOwned::to_owned)
        } else {
            None
        }
    });

    let Some(root_id) = root_node_id else {
        // Fallback: return full tree if target not found
        return all_nodes.to_vec();
    };

    // BFS to collect all descendant nodeIds
    let mut included: HashSet<String> = HashSet::new();
    let mut queue = vec![root_id.clone()];
    included.insert(root_id);

    // Build nodeId → node map for child lookup
    let node_map: std::collections::HashMap<String, &serde_json::Value> = all_nodes
        .iter()
        .filter_map(|n| {
            n.get("nodeId")
                .and_then(serde_json::Value::as_str)
                .map(|id| (id.to_owned(), n))
        })
        .collect();

    while let Some(nid) = queue.pop() {
        if let Some(node) = node_map.get(&nid)
            && let Some(children) = node.get("childIds").and_then(serde_json::Value::as_array)
        {
            for child in children {
                if let Some(cid) = child.as_str()
                    && included.insert(cid.to_owned())
                {
                    queue.push(cid.to_owned());
                }
            }
        }
    }

    // Filter and return only included nodes, preserving order
    all_nodes
        .iter()
        .filter(|n| {
            n.get("nodeId")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| included.contains(id))
        })
        .cloned()
        .collect()
}

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
    /// If `options.selector` is set, the snapshot is scoped to the subtree
    /// rooted at the first element matching that CSS selector.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP accessibility call fails.
    pub async fn snapshot_with(&self, options: SnapshotOptions) -> Result<Snapshot> {
        let nodes: Vec<serde_json::Value> = if let Some(ref sel) = options.selector {
            // Scoped snapshot: resolve the selector to a backend node, then
            // query only the accessibility subtree under that node.
            let backend_id = self.resolve_backend_node_id(sel).await?;
            let params = GetFullAxTreeParams::default();
            // GetFullAXTree does not support scoping directly, so we get the
            // full tree and filter to the subtree rooted at the target node.
            let result = self.inner.execute(params).await.map_err(Error::Cdp)?;
            let all: Vec<serde_json::Value> = serde_json::to_value(&result.result.nodes)
                .and_then(serde_json::from_value)
                .map_err(|e| Error::Snapshot(format!("failed to parse AX tree: {e}")))?;
            filter_subtree(&all, backend_id)
        } else {
            let result = self
                .inner
                .execute(GetFullAxTreeParams::default())
                .await
                .map_err(Error::Cdp)?;
            serde_json::to_value(&result.result.nodes)
                .and_then(serde_json::from_value)
                .map_err(|e| Error::Snapshot(format!("failed to parse AX tree: {e}")))?
        };

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

    /// Resolve a CSS selector to a CDP `backendNodeId` using
    /// `DOM.resolveNode` after finding the element via JS.
    ///
    /// Returns the `backendDOMNodeId` that can be matched against
    /// the accessibility tree's node metadata.
    async fn resolve_backend_node_id(&self, selector: &str) -> Result<i64> {
        use chromiumoxide::cdp::browser_protocol::dom::{
            DescribeNodeParams, GetDocumentParams, QuerySelectorParams,
        };

        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        // We use CDP DOM.getDocument + DOM.querySelector to resolve
        // the selector to a nodeId, then read its backendDOMNodeId.
        let js = format!(
            r"(() => {{
                const el = document.querySelector('{escaped}');
                return el ? true : false;
            }})()"
        );
        let exists = self.eval(&js).await?;
        if !exists.as_bool().unwrap_or(false) {
            return Err(Error::ElementNotFound(format!(
                "selector '{escaped}' matched no elements for scoped snapshot"
            )));
        }

        let doc = self
            .inner
            .execute(GetDocumentParams::builder().build())
            .await
            .map_err(Error::Cdp)?;

        let root_id = doc.result.root.node_id;

        let qs = self
            .inner
            .execute(QuerySelectorParams::new(root_id, selector))
            .await
            .map_err(|_| {
                Error::ElementNotFound(format!("selector '{escaped}' matched no elements"))
            })?;

        let desc = self
            .inner
            .execute(
                DescribeNodeParams::builder()
                    .node_id(qs.result.node_id)
                    .build(),
            )
            .await
            .map_err(Error::Cdp)?;

        let id = *desc.result.node.backend_node_id.inner();
        Ok(id)
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
