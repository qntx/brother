/// Semantic locator methods for finding elements by ARIA role, text, label, etc.
use crate::error::{Error, Result};
use crate::page::Page;

impl Page {
    /// Find elements by ARIA role (e.g. `button`, `link`, `heading`).
    ///
    /// Returns a JSON array of `{ ref, role, name }` objects.
    ///
    /// # Errors
    ///
    /// Returns an error if the JS evaluation fails.
    pub async fn find_by_role(&self, role: &str, name: Option<&str>) -> Result<serde_json::Value> {
        // Use CDP accessibility tree for role-based search.
        let result = self
            .inner
            .execute(
                chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams::default(),
            )
            .await
            .map_err(Error::Cdp)?;

        let nodes = &result.result.nodes;
        let role_lower = role.to_lowercase();
        let name_lower = name.map(str::to_lowercase);

        let mut matches = Vec::new();
        for node in nodes {
            let node_role = node
                .role
                .as_ref()
                .and_then(|r| r.value.as_ref())
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            if node_role != role_lower {
                continue;
            }
            let node_name = node
                .name
                .as_ref()
                .and_then(|n| n.value.as_ref())
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if let Some(ref nl) = name_lower
                && !node_name.to_lowercase().contains(nl.as_str())
            {
                continue;
            }
            let backend_id = node.backend_dom_node_id.unwrap_or_default();
            matches.push(serde_json::json!({
                "role": node_role,
                "name": node_name,
                "backendNodeId": backend_id,
            }));
        }

        Ok(serde_json::Value::Array(matches))
    }

    /// Find elements whose text content matches the given pattern.
    ///
    /// # Errors
    ///
    /// Returns an error if the JS evaluation fails.
    pub async fn find_by_text(&self, text: &str, exact: bool) -> Result<serde_json::Value> {
        let escaped = text.replace('\\', "\\\\").replace('\'', "\\'");
        let condition = if exact {
            format!("el.textContent.trim() === '{escaped}'")
        } else {
            format!(
                "el.textContent.toLowerCase().includes('{}')",
                escaped.to_lowercase()
            )
        };
        let js = format!(
            r"(() => {{
                const results = [];
                const all = document.querySelectorAll('*');
                for (const el of all) {{
                    if (el.children.length === 0 && {condition}) {{
                        results.push({{
                            tag: el.tagName.toLowerCase(),
                            text: el.textContent.trim().substring(0, 100),
                        }});
                        if (results.length >= 20) break;
                    }}
                }}
                return results;
            }})()",
        );
        self.eval(&js).await
    }

    /// Find elements by associated label text.
    ///
    /// # Errors
    ///
    /// Returns an error if the JS evaluation fails.
    pub async fn find_by_label(&self, label: &str) -> Result<serde_json::Value> {
        let escaped = label.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            r"(() => {{
                const results = [];
                const labels = document.querySelectorAll('label');
                for (const lbl of labels) {{
                    if (lbl.textContent.toLowerCase().includes('{}')) {{
                        const forId = lbl.getAttribute('for');
                        if (forId) {{
                            const input = document.getElementById(forId);
                            if (input) results.push({{ label: lbl.textContent.trim(), tag: input.tagName.toLowerCase(), id: forId }});
                        }} else {{
                            const input = lbl.querySelector('input,select,textarea');
                            if (input) results.push({{ label: lbl.textContent.trim(), tag: input.tagName.toLowerCase() }});
                        }}
                    }}
                }}
                return results;
            }})()",
            escaped.to_lowercase()
        );
        self.eval(&js).await
    }

    /// Find elements by placeholder attribute.
    ///
    /// # Errors
    ///
    /// Returns an error if the JS evaluation fails.
    pub async fn find_by_placeholder(&self, placeholder: &str) -> Result<serde_json::Value> {
        let escaped = placeholder.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            r"(() => {{
                const results = [];
                const els = document.querySelectorAll('[placeholder]');
                for (const el of els) {{
                    if (el.placeholder.toLowerCase().includes('{}')) {{
                        results.push({{ tag: el.tagName.toLowerCase(), placeholder: el.placeholder, type: el.type || '' }});
                    }}
                }}
                return results;
            }})()",
            escaped.to_lowercase()
        );
        self.eval(&js).await
    }

    /// Find elements by `data-testid` attribute.
    ///
    /// # Errors
    ///
    /// Returns an error if the JS evaluation fails.
    pub async fn find_by_testid(&self, testid: &str) -> Result<serde_json::Value> {
        let escaped = testid.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            r#"(() => {{
                const results = [];
                const els = document.querySelectorAll('[data-testid="{escaped}"]');
                for (const el of els) {{
                    results.push({{ tag: el.tagName.toLowerCase(), testid: el.dataset.testid, text: el.textContent.trim().substring(0, 100) }});
                }}
                return results;
            }})()"#,
        );
        self.eval(&js).await
    }
}
