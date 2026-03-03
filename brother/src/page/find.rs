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

    /// Find elements by `alt` attribute (images, areas, inputs).
    ///
    /// # Errors
    ///
    /// Returns an error if the JS evaluation fails.
    pub async fn find_by_alt_text(&self, alt: &str, exact: bool) -> Result<serde_json::Value> {
        let escaped = alt.replace('\\', "\\\\").replace('\'', "\\'");
        let condition = if exact {
            format!("el.alt === '{escaped}'")
        } else {
            format!(
                "el.alt.toLowerCase().includes('{}')",
                escaped.to_lowercase()
            )
        };
        let js = format!(
            r"(() => {{
                const results = [];
                const els = document.querySelectorAll('[alt]');
                for (const el of els) {{
                    if ({condition}) {{
                        results.push({{ tag: el.tagName.toLowerCase(), alt: el.alt, src: el.src || '' }});
                    }}
                }}
                return results;
            }})()",
        );
        self.eval(&js).await
    }

    /// Find elements by `title` attribute.
    ///
    /// # Errors
    ///
    /// Returns an error if the JS evaluation fails.
    pub async fn find_by_title(&self, title: &str, exact: bool) -> Result<serde_json::Value> {
        let escaped = title.replace('\\', "\\\\").replace('\'', "\\'");
        let condition = if exact {
            format!("el.title === '{escaped}'")
        } else {
            format!(
                "el.title.toLowerCase().includes('{}')",
                escaped.to_lowercase()
            )
        };
        let js = format!(
            r"(() => {{
                const results = [];
                const els = document.querySelectorAll('[title]');
                for (const el of els) {{
                    if ({condition}) {{
                        results.push({{ tag: el.tagName.toLowerCase(), title: el.title, text: el.textContent.trim().substring(0, 100) }});
                    }}
                }}
                return results;
            }})()",
        );
        self.eval(&js).await
    }

    /// Find an element by semantic locator and execute a sub-action on it.
    ///
    /// Supported locator types: `role`, `text`, `label`, `placeholder`,
    /// `testid`, `alttext`, `title`.
    ///
    /// Supported sub-actions: `click`, `fill`, `check`, `hover`.
    ///
    /// Returns a JSON object describing the action taken.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found or the action fails.
    pub async fn locator_action(
        &self,
        by: &str,
        value: &str,
        name: Option<&str>,
        exact: bool,
        subaction: &str,
        fill_value: Option<&str>,
    ) -> Result<serde_json::Value> {
        let escaped_val = value.replace('\\', "\\\\").replace('\'', "\\'");

        // Validate locator type early
        if !matches!(
            by,
            "role" | "text" | "label" | "placeholder" | "testid" | "alttext" | "alt" | "title"
        ) {
            return Err(Error::InvalidArgument(format!(
                "unknown locator type '{by}'. Use: role, text, label, placeholder, testid, alttext, title"
            )));
        }

        // Build JS to find the element via DOM queries (chromiumoxide, not Playwright)
        let find_js = Self::build_locator_find_js(by, &escaped_val, name, exact);

        let action_js = match subaction {
            "click" => format!(
                r"(async () => {{
                    {find_js}
                    if (!el) throw new Error('no element found for {by}={escaped_val}');
                    el.scrollIntoView({{ block: 'center' }});
                    el.click();
                    return {{ action: 'click', tag: el.tagName.toLowerCase(), text: (el.textContent || '').trim().substring(0, 80) }};
                }})()"
            ),
            "fill" => {
                let fv = fill_value.unwrap_or("");
                let escaped_fv = fv.replace('\\', "\\\\").replace('\'', "\\'");
                format!(
                    r"(async () => {{
                        {find_js}
                        if (!el) throw new Error('no element found for {by}={escaped_val}');
                        el.scrollIntoView({{ block: 'center' }});
                        el.focus();
                        el.value = '';
                        el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                        el.value = '{escaped_fv}';
                        el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                        el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                        return {{ action: 'fill', tag: el.tagName.toLowerCase(), value: '{escaped_fv}' }};
                    }})()"
                )
            }
            "check" => format!(
                r"(async () => {{
                    {find_js}
                    if (!el) throw new Error('no element found for {by}={escaped_val}');
                    if (!el.checked) el.click();
                    return {{ action: 'check', tag: el.tagName.toLowerCase(), checked: el.checked }};
                }})()"
            ),
            "hover" => format!(
                r"(async () => {{
                    {find_js}
                    if (!el) throw new Error('no element found for {by}={escaped_val}');
                    el.scrollIntoView({{ block: 'center' }});
                    el.dispatchEvent(new MouseEvent('mouseover', {{ bubbles: true }}));
                    el.dispatchEvent(new MouseEvent('mouseenter', {{ bubbles: true }}));
                    return {{ action: 'hover', tag: el.tagName.toLowerCase(), text: (el.textContent || '').trim().substring(0, 80) }};
                }})()"
            ),
            other => {
                return Err(Error::InvalidArgument(format!(
                    "unknown subaction '{other}'. Use: click, fill, check, hover"
                )));
            }
        };

        self.eval(&action_js).await
    }

    /// Build JS code to find an element by locator type.
    /// Sets variable `el` to the first matching element.
    fn build_locator_find_js(
        by: &str,
        escaped_val: &str,
        name: Option<&str>,
        exact: bool,
    ) -> String {
        match by {
            "role" => {
                // Use the accessibility tree query via TreeWalker
                let role_lower = escaped_val.to_lowercase();
                let name_filter = name.map_or_else(String::new, |n| {
                    let en = n.replace('\\', "\\\\").replace('\'', "\\'").to_lowercase();
                    format!(
                        " && (el.getAttribute('aria-label') || el.textContent || '').toLowerCase().includes('{en}')"
                    )
                });
                format!(
                    r"const el = (() => {{
                        const all = document.querySelectorAll('[role]');
                        for (const e of all) {{
                            if (e.getAttribute('role').toLowerCase() === '{role_lower}'{name_filter}) return e;
                        }}
                        // Fallback: implicit roles via tag name
                        const roleMap = {{ button: 'button', a: 'link', input: 'textbox', select: 'combobox', textarea: 'textbox', h1: 'heading', h2: 'heading', h3: 'heading', h4: 'heading', h5: 'heading', h6: 'heading' }};
                        for (const e of document.querySelectorAll('*')) {{
                            const implicit = roleMap[e.tagName.toLowerCase()];
                            if (implicit === '{role_lower}'{name_filter}) return e;
                        }}
                        return null;
                    }})();"
                )
            }
            "text" => {
                let cond = if exact {
                    format!("el.textContent.trim() === '{escaped_val}'")
                } else {
                    format!(
                        "el.textContent.toLowerCase().includes('{}')",
                        escaped_val.to_lowercase()
                    )
                };
                format!(
                    r"const el = (() => {{
                        for (const el of document.querySelectorAll('*')) {{
                            if (el.children.length === 0 && {cond}) return el;
                        }}
                        return null;
                    }})();"
                )
            }
            "label" => {
                let lower = escaped_val.to_lowercase();
                format!(
                    r"const el = (() => {{
                        for (const lbl of document.querySelectorAll('label')) {{
                            if (lbl.textContent.toLowerCase().includes('{lower}')) {{
                                const forId = lbl.getAttribute('for');
                                if (forId) return document.getElementById(forId);
                                return lbl.querySelector('input,select,textarea');
                            }}
                        }}
                        return null;
                    }})();"
                )
            }
            "placeholder" => {
                let lower = escaped_val.to_lowercase();
                let cond = if exact {
                    format!("e.placeholder === '{escaped_val}'")
                } else {
                    format!("e.placeholder.toLowerCase().includes('{lower}')")
                };
                format!(
                    r"const el = (() => {{
                        for (const e of document.querySelectorAll('[placeholder]')) {{
                            if ({cond}) return e;
                        }}
                        return null;
                    }})();"
                )
            }
            "testid" => format!(
                r#"const el = document.querySelector('[data-testid="{escaped_val}"]');"#
            ),
            "alttext" | "alt" => {
                let lower = escaped_val.to_lowercase();
                let cond = if exact {
                    format!("e.alt === '{escaped_val}'")
                } else {
                    format!("e.alt.toLowerCase().includes('{lower}')")
                };
                format!(
                    r"const el = (() => {{
                        for (const e of document.querySelectorAll('[alt]')) {{
                            if ({cond}) return e;
                        }}
                        return null;
                    }})();"
                )
            }
            "title" => {
                let lower = escaped_val.to_lowercase();
                let cond = if exact {
                    format!("e.title === '{escaped_val}'")
                } else {
                    format!("e.title.toLowerCase().includes('{lower}')")
                };
                format!(
                    r"const el = (() => {{
                        for (const e of document.querySelectorAll('[title]')) {{
                            if ({cond}) return e;
                        }}
                        return null;
                    }})();"
                )
            }
            _ => "const el = null;".to_owned(),
        }
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
