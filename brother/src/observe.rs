//! Reading page state: snapshots, screenshots, text queries, semantic locators.

use std::collections::HashSet;

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams;
use chromiumoxide::page::ScreenshotParams;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::page::Page;
use crate::snapshot::{self, CursorItem, Snapshot, SnapshotOptions};

// ---------------------------------------------------------------------------
// ImageFormat type
// ---------------------------------------------------------------------------

/// Image format for screenshots and screencasts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    /// PNG (lossless, default).
    #[default]
    Png,
    /// JPEG (lossy, smaller file size).
    Jpeg,
}

impl ImageFormat {
    /// File extension for this format.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    /// Parse from a string (case-insensitive). Returns `Png` for unknown values.
    #[must_use]
    pub fn from_str_lossy(s: &str) -> Self {
        if s.eq_ignore_ascii_case("jpeg") || s.eq_ignore_ascii_case("jpg") {
            Self::Jpeg
        } else {
            Self::Png
        }
    }

    const fn to_cdp(self) -> CaptureScreenshotFormat {
        match self {
            Self::Png => CaptureScreenshotFormat::Png,
            Self::Jpeg => CaptureScreenshotFormat::Jpeg,
        }
    }
}

impl std::fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Png => f.write_str("png"),
            Self::Jpeg => f.write_str("jpeg"),
        }
    }
}

impl std::str::FromStr for ImageFormat {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "png" => Ok(Self::Png),
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            other => Err(format!("unknown image format '{other}', expected 'png' or 'jpeg'")),
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot capture
// ---------------------------------------------------------------------------

impl Page {
    /// Capture an accessibility snapshot with default options.
    pub async fn snapshot(&self) -> Result<Snapshot> {
        self.snapshot_with(SnapshotOptions::default()).await
    }

    /// Capture an accessibility snapshot with custom options.
    pub async fn snapshot_with(&self, options: SnapshotOptions) -> Result<Snapshot> {
        let nodes: Vec<serde_json::Value> = if let Some(ref sel) = options.selector {
            let backend_id = self.resolve_backend_node_id(sel).await?;
            let all: Vec<serde_json::Value> = serde_json::to_value(
                &self.inner.execute(GetFullAxTreeParams::default()).await.map_err(Error::Cdp)?.result.nodes,
            )
            .and_then(serde_json::from_value)
            .map_err(|e| Error::Snapshot(format!("failed to parse AX tree: {e}")))?;
            filter_subtree(&all, backend_id)
        } else {
            serde_json::to_value(
                &self.inner.execute(GetFullAxTreeParams::default()).await.map_err(Error::Cdp)?.result.nodes,
            )
            .and_then(serde_json::from_value)
            .map_err(|e| Error::Snapshot(format!("failed to parse AX tree: {e}")))?
        };

        let mut snap = snapshot::build_snapshot(&nodes, &options);
        if options.cursor_interactive {
            self.append_cursor_interactive_elements(&mut snap).await;
        }
        *self.refs.lock().await = snap.refs().clone();
        Ok(snap)
    }

    async fn resolve_backend_node_id(&self, selector: &str) -> Result<i64> {
        use chromiumoxide::cdp::browser_protocol::dom::{DescribeNodeParams, GetDocumentParams, QuerySelectorParams};
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let exists = self.eval(&format!("(() => {{ return !!document.querySelector('{escaped}'); }})()")).await?;
        if !exists.as_bool().unwrap_or(false) {
            return Err(Error::ElementNotFound(format!("selector '{escaped}' matched no elements for scoped snapshot")));
        }
        let doc = self.inner.execute(GetDocumentParams::builder().build()).await.map_err(Error::Cdp)?;
        let qs = self.inner.execute(QuerySelectorParams::new(doc.result.root.node_id, selector)).await
            .map_err(|_| Error::ElementNotFound(format!("selector '{escaped}' matched no elements")))?;
        let desc = self.inner.execute(DescribeNodeParams::builder().node_id(qs.result.node_id).build()).await.map_err(Error::Cdp)?;
        Ok(*desc.result.node.backend_node_id.inner())
    }

    async fn append_cursor_interactive_elements(&self, snap: &mut Snapshot) {
        let js = r"(() => {
            const interactive = new Set(['a','button','input','select','textarea','details','summary']);
            const results = [];
            for (const el of document.querySelectorAll('*')) {
                if (interactive.has(el.tagName.toLowerCase())) continue;
                const role = el.getAttribute('role');
                if (role && ['button','link','textbox','checkbox','radio','combobox','menuitem','option','tab','switch'].includes(role)) continue;
                const cs = getComputedStyle(el);
                const ptr = cs.cursor === 'pointer';
                const click = el.hasAttribute('onclick') || el.onclick !== null;
                const ti = el.getAttribute('tabindex');
                const tab = ti !== null && ti !== '-1';
                if (!ptr && !click && !tab) continue;
                if (ptr && !click && !tab) { const p = el.parentElement; if (p && getComputedStyle(p).cursor === 'pointer') continue; }
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
        let Ok(val) = self.eval(js).await else { return };
        let Ok(items) = serde_json::from_str::<Vec<CursorItem>>(val.as_str().unwrap_or("[]")) else { return };
        snap.append_cursor_elements(&items);
    }
}

fn filter_subtree(all_nodes: &[serde_json::Value], target_backend_id: i64) -> Vec<serde_json::Value> {
    let root_node_id = all_nodes.iter().find_map(|n| {
        let bid = n.get("backendDOMNodeId").and_then(serde_json::Value::as_i64)?;
        if bid == target_backend_id { n.get("nodeId").and_then(serde_json::Value::as_str).map(ToOwned::to_owned) } else { None }
    });
    let Some(root_id) = root_node_id else { return all_nodes.to_vec() };
    let mut included: HashSet<String> = HashSet::new();
    let mut queue = vec![root_id.clone()];
    included.insert(root_id);
    let node_map: std::collections::HashMap<String, &serde_json::Value> = all_nodes.iter()
        .filter_map(|n| n.get("nodeId").and_then(serde_json::Value::as_str).map(|id| (id.to_owned(), n)))
        .collect();
    while let Some(nid) = queue.pop() {
        if let Some(node) = node_map.get(&nid)
            && let Some(children) = node.get("childIds").and_then(serde_json::Value::as_array)
        {
            for child in children {
                if let Some(cid) = child.as_str() && included.insert(cid.to_owned()) {
                    queue.push(cid.to_owned());
                }
            }
        }
    }
    all_nodes.iter().filter(|n| n.get("nodeId").and_then(serde_json::Value::as_str).is_some_and(|id| included.contains(id))).cloned().collect()
}

// ---------------------------------------------------------------------------
// Screenshot
// ---------------------------------------------------------------------------

impl Page {
    /// Capture a PNG screenshot of the viewport.
    pub async fn screenshot_png(&self) -> Result<Vec<u8>> {
        self.inner.screenshot(ScreenshotParams::builder().format(CaptureScreenshotFormat::Png).build()).await.map_err(Error::Cdp)
    }

    /// Capture a JPEG screenshot.
    pub async fn screenshot_jpeg(&self, quality: u8) -> Result<Vec<u8>> {
        self.inner.screenshot(ScreenshotParams::builder().format(CaptureScreenshotFormat::Jpeg).quality(i64::from(quality)).build()).await.map_err(Error::Cdp)
    }

    /// Capture a screenshot with full options.
    pub async fn screenshot(&self, full_page: bool, selector: Option<&str>, format: ImageFormat, quality: Option<u8>) -> Result<Vec<u8>> {
        use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotParams, Viewport as CdpViewport};
        let fmt = format.to_cdp();
        if let Some(sel) = selector {
            let (x, y, w, h) = self.bounding_box(sel).await?;
            let clip = CdpViewport { x, y, width: w, height: h, scale: 1.0 };
            let mut params = CaptureScreenshotParams::builder().format(fmt).clip(clip);
            if let Some(q) = quality { params = params.quality(i64::from(q)); }
            let data = self.inner.execute(params.build()).await.map_err(Error::Cdp)?;
            return base64::engine::general_purpose::STANDARD.decode(&data.result.data).map_err(|e| Error::Browser(format!("base64 decode: {e}")));
        }
        let mut builder = ScreenshotParams::builder().format(fmt);
        if full_page { builder = builder.full_page(true); }
        if let Some(q) = quality { builder = builder.quality(i64::from(q)); }
        self.inner.screenshot(builder.build()).await.map_err(Error::Cdp)
    }
}

// ---------------------------------------------------------------------------
// Text / attribute / state queries
// ---------------------------------------------------------------------------

impl Page {
    /// Get text content of the page or a specific element.
    pub async fn get_text(&self, target: Option<&str>) -> Result<String> {
        if let Some(t) = target {
            self.call_text_on_target(t, "function() { return this.textContent || ''; }").await
        } else {
            let val = self.eval("document.body?.innerText || ''").await?;
            Ok(val.as_str().unwrap_or("").to_owned())
        }
    }

    /// Get inner text (rendered) of an element.
    pub async fn get_inner_text(&self, target: &str) -> Result<String> {
        self.call_text_on_target(target, "function() { return this.innerText || ''; }").await
    }

    /// Get the current page URL.
    pub async fn url(&self) -> Result<String> {
        Ok(self.inner.url().await.map_err(Error::Cdp)?.unwrap_or_default())
    }

    /// Get the current page title.
    pub async fn title(&self) -> Result<String> {
        Ok(self.inner.evaluate("document.title").await.map_err(Error::Cdp)?.into_value::<String>().unwrap_or_default())
    }

    /// Get the full page HTML content.
    pub async fn content(&self) -> Result<String> {
        self.inner.content().await.map_err(Error::Cdp)
    }

    /// Get the inner HTML of an element.
    pub async fn get_html(&self, target: &str) -> Result<String> {
        self.call_text_on_target(target, "function() { return this.innerHTML || ''; }").await
    }

    /// Get the value of an input element.
    pub async fn get_value(&self, target: &str) -> Result<String> {
        self.call_text_on_target(target, "function() { return this.value || ''; }").await
    }

    /// Get an attribute value from an element.
    pub async fn get_attribute(&self, target: &str, attribute: &str) -> Result<String> {
        let escaped = attribute.replace('\\', "\\\\").replace('\'', "\\'");
        self.call_text_on_target(target, &format!("function() {{ return this.getAttribute('{escaped}') || ''; }}")).await
    }

    /// Check if an element is visible.
    pub async fn is_visible(&self, target: &str) -> Result<bool> {
        self.call_bool_on_target(target, "function() { const r = this.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(this).visibility !== 'hidden'; }").await
    }

    /// Check if an element is enabled.
    pub async fn is_enabled(&self, target: &str) -> Result<bool> {
        self.call_bool_on_target(target, "function() { return !this.disabled; }").await
    }

    /// Check if a checkbox/radio is checked.
    pub async fn is_checked(&self, target: &str) -> Result<bool> {
        self.call_bool_on_target(target, "function() { return !!this.checked; }").await
    }

    /// Count elements matching a CSS selector.
    pub async fn count(&self, selector: &str) -> Result<usize> {
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let val = self.eval(&format!("document.querySelectorAll('{escaped}').length")).await?;
        Ok(usize::try_from(val.as_u64().unwrap_or(0)).unwrap_or(0))
    }

    /// Get computed styles of an element as a JSON value.
    pub async fn get_styles(&self, target: &str) -> Result<serde_json::Value> {
        let escaped = target.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!(
            "(() => {{\
                const el = document.querySelector('{escaped}');\
                if (!el) throw new Error('element not found: {escaped}');\
                const s = getComputedStyle(el); const r = el.getBoundingClientRect();\
                return {{ tag: el.tagName.toLowerCase(), text: (el.innerText || \"\").trim().slice(0, 80) || null,\
                    box: {{ x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height) }},\
                    styles: {{ fontSize: s.fontSize, fontWeight: s.fontWeight, fontFamily: s.fontFamily.split(\",\")[0].trim().replace(/\"/g, \"\"),\
                        color: s.color, backgroundColor: s.backgroundColor, borderRadius: s.borderRadius,\
                        border: s.border !== \"none\" && s.borderWidth !== \"0px\" ? s.border : null,\
                        boxShadow: s.boxShadow !== \"none\" ? s.boxShadow : null, padding: s.padding }} }};\
            }})()"
        )).await
    }

    /// Get the bounding box (x, y, width, height) of an element.
    pub async fn bounding_box(&self, target: &str) -> Result<(f64, f64, f64, f64)> {
        let object_id = self.resolve_target_object(target).await?;
        let js = "function(){const r=this.getBoundingClientRect();return JSON.stringify({x:r.x,y:r.y,width:r.width,height:r.height})}";
        let params = CallFunctionOnParams::builder().object_id(object_id).function_declaration(js).return_by_value(true).build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        let json_str: String = resp.result.result.value.as_ref()
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| Error::Browser("bounding_box returned no value".into()))?;
        let parsed: serde_json::Value = serde_json::from_str(&json_str).map_err(|e| Error::Browser(e.to_string()))?;
        Ok((parsed["x"].as_f64().unwrap_or(0.0), parsed["y"].as_f64().unwrap_or(0.0), parsed["width"].as_f64().unwrap_or(0.0), parsed["height"].as_f64().unwrap_or(0.0)))
    }
}

// ---------------------------------------------------------------------------
// Semantic locators (find by role, text, label, etc.)
// ---------------------------------------------------------------------------

impl Page {
    /// Find elements by ARIA role.
    pub async fn find_by_role(&self, role: &str, name: Option<&str>) -> Result<serde_json::Value> {
        let result = self.inner.execute(GetFullAxTreeParams::default()).await.map_err(Error::Cdp)?;
        let nodes = &result.result.nodes;
        let role_lower = role.to_lowercase();
        let name_lower = name.map(str::to_lowercase);
        let mut matches = Vec::new();
        for node in nodes {
            let node_role = node.role.as_ref().and_then(|r| r.value.as_ref()).and_then(serde_json::Value::as_str).unwrap_or("").to_lowercase();
            if node_role != role_lower { continue; }
            let node_name = node.name.as_ref().and_then(|n| n.value.as_ref()).and_then(serde_json::Value::as_str).unwrap_or("");
            if let Some(ref nl) = name_lower && !node_name.to_lowercase().contains(nl.as_str()) { continue; }
            let backend_id = node.backend_dom_node_id.unwrap_or_default();
            matches.push(serde_json::json!({ "role": node_role, "name": node_name, "backendNodeId": backend_id }));
        }
        Ok(serde_json::Value::Array(matches))
    }

    /// Find elements by text content.
    pub async fn find_by_text(&self, text: &str, exact: bool) -> Result<serde_json::Value> {
        let escaped = text.replace('\\', "\\\\").replace('\'', "\\'");
        let condition = if exact { format!("el.textContent.trim() === '{escaped}'") } else { format!("el.textContent.toLowerCase().includes('{}')", escaped.to_lowercase()) };
        self.eval(&format!(
            r"(() => {{ const results = []; for (const el of document.querySelectorAll('*')) {{ if (el.children.length === 0 && {condition}) {{ results.push({{ tag: el.tagName.toLowerCase(), text: el.textContent.trim().substring(0, 100) }}); if (results.length >= 20) break; }} }} return results; }})()"
        )).await
    }

    /// Find elements by associated label text.
    pub async fn find_by_label(&self, label: &str) -> Result<serde_json::Value> {
        let escaped = label.replace('\\', "\\\\").replace('\'', "\\'").to_lowercase();
        self.eval(&format!(
            r"(() => {{ const results = []; for (const lbl of document.querySelectorAll('label')) {{ if (lbl.textContent.toLowerCase().includes('{escaped}')) {{ const forId = lbl.getAttribute('for'); if (forId) {{ const input = document.getElementById(forId); if (input) results.push({{ label: lbl.textContent.trim(), tag: input.tagName.toLowerCase(), id: forId }}); }} else {{ const input = lbl.querySelector('input,select,textarea'); if (input) results.push({{ label: lbl.textContent.trim(), tag: input.tagName.toLowerCase() }}); }} }} }} return results; }})()"
        )).await
    }

    /// Find elements by placeholder attribute.
    pub async fn find_by_placeholder(&self, placeholder: &str) -> Result<serde_json::Value> {
        let escaped = placeholder.replace('\\', "\\\\").replace('\'', "\\'").to_lowercase();
        self.eval(&format!(
            r"(() => {{ const results = []; for (const el of document.querySelectorAll('[placeholder]')) {{ if (el.placeholder.toLowerCase().includes('{escaped}')) {{ results.push({{ tag: el.tagName.toLowerCase(), placeholder: el.placeholder, type: el.type || '' }}); }} }} return results; }})()"
        )).await
    }

    /// Find elements by `alt` attribute.
    pub async fn find_by_alt_text(&self, alt: &str, exact: bool) -> Result<serde_json::Value> {
        let escaped = alt.replace('\\', "\\\\").replace('\'', "\\'");
        let condition = if exact { format!("el.alt === '{escaped}'") } else { format!("el.alt.toLowerCase().includes('{}')", escaped.to_lowercase()) };
        self.eval(&format!(
            r"(() => {{ const results = []; for (const el of document.querySelectorAll('[alt]')) {{ if ({condition}) {{ results.push({{ tag: el.tagName.toLowerCase(), alt: el.alt, src: el.src || '' }}); }} }} return results; }})()"
        )).await
    }

    /// Find elements by `title` attribute.
    pub async fn find_by_title(&self, title: &str, exact: bool) -> Result<serde_json::Value> {
        let escaped = title.replace('\\', "\\\\").replace('\'', "\\'");
        let condition = if exact { format!("el.title === '{escaped}'") } else { format!("el.title.toLowerCase().includes('{}')", escaped.to_lowercase()) };
        self.eval(&format!(
            r"(() => {{ const results = []; for (const el of document.querySelectorAll('[title]')) {{ if ({condition}) {{ results.push({{ tag: el.tagName.toLowerCase(), title: el.title, text: el.textContent.trim().substring(0, 100) }}); }} }} return results; }})()"
        )).await
    }

    /// Find elements by `data-testid` attribute.
    pub async fn find_by_testid(&self, testid: &str) -> Result<serde_json::Value> {
        let escaped = testid.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!(
            r#"(() => {{ const results = []; for (const el of document.querySelectorAll('[data-testid="{escaped}"]')) {{ results.push({{ tag: el.tagName.toLowerCase(), testid: el.dataset.testid, text: el.textContent.trim().substring(0, 100) }}); }} return results; }})()"#
        )).await
    }

    /// Find an element by semantic locator and execute a sub-action on it.
    pub async fn locator_action(
        &self, by: &str, value: &str, name: Option<&str>, exact: bool,
        subaction: &str, fill_value: Option<&str>,
    ) -> Result<serde_json::Value> {
        let escaped_val = value.replace('\\', "\\\\").replace('\'', "\\'");
        if !matches!(by, "role" | "text" | "label" | "placeholder" | "testid" | "alttext" | "alt" | "title") {
            return Err(Error::InvalidArgument(format!("unknown locator type '{by}'. Use: role, text, label, placeholder, testid, alttext, title")));
        }
        let find_js = Self::build_locator_find_js(by, &escaped_val, name, exact);
        let action_js = match subaction {
            "click" => format!(r"(async () => {{ {find_js} if (!el) throw new Error('no element found for {by}={escaped_val}'); el.scrollIntoView({{ block: 'center' }}); el.click(); return {{ action: 'click', tag: el.tagName.toLowerCase(), text: (el.textContent || '').trim().substring(0, 80) }}; }})()"),
            "fill" => {
                let fv = fill_value.unwrap_or("").replace('\\', "\\\\").replace('\'', "\\'");
                format!(r"(async () => {{ {find_js} if (!el) throw new Error('no element found for {by}={escaped_val}'); el.scrollIntoView({{ block: 'center' }}); el.focus(); el.value = ''; el.dispatchEvent(new Event('input', {{ bubbles: true }})); el.value = '{fv}'; el.dispatchEvent(new Event('input', {{ bubbles: true }})); el.dispatchEvent(new Event('change', {{ bubbles: true }})); return {{ action: 'fill', tag: el.tagName.toLowerCase(), value: '{fv}' }}; }})()")
            }
            "check" => format!(r"(async () => {{ {find_js} if (!el) throw new Error('no element found for {by}={escaped_val}'); if (!el.checked) el.click(); return {{ action: 'check', tag: el.tagName.toLowerCase(), checked: el.checked }}; }})()"),
            "hover" => format!(r"(async () => {{ {find_js} if (!el) throw new Error('no element found for {by}={escaped_val}'); el.scrollIntoView({{ block: 'center' }}); el.dispatchEvent(new MouseEvent('mouseover', {{ bubbles: true }})); el.dispatchEvent(new MouseEvent('mouseenter', {{ bubbles: true }})); return {{ action: 'hover', tag: el.tagName.toLowerCase(), text: (el.textContent || '').trim().substring(0, 80) }}; }})()"),
            other => return Err(Error::InvalidArgument(format!("unknown subaction '{other}'. Use: click, fill, check, hover"))),
        };
        self.eval(&action_js).await
    }

    fn build_locator_find_js(by: &str, escaped_val: &str, name: Option<&str>, exact: bool) -> String {
        match by {
            "role" => {
                let role_lower = escaped_val.to_lowercase();
                let name_filter = name.map_or_else(String::new, |n| {
                    let en = n.replace('\\', "\\\\").replace('\'', "\\'").to_lowercase();
                    format!(" && (el.getAttribute('aria-label') || el.textContent || '').toLowerCase().includes('{en}')")
                });
                format!(r"const el = (() => {{ for (const e of document.querySelectorAll('[role]')) {{ if (e.getAttribute('role').toLowerCase() === '{role_lower}'{name_filter}) return e; }} const roleMap = {{ button: 'button', a: 'link', input: 'textbox', select: 'combobox', textarea: 'textbox', h1: 'heading', h2: 'heading', h3: 'heading', h4: 'heading', h5: 'heading', h6: 'heading' }}; for (const e of document.querySelectorAll('*')) {{ const implicit = roleMap[e.tagName.toLowerCase()]; if (implicit === '{role_lower}'{name_filter}) return e; }} return null; }})();")
            }
            "text" => {
                let cond = if exact { format!("el.textContent.trim() === '{escaped_val}'") } else { format!("el.textContent.toLowerCase().includes('{}')", escaped_val.to_lowercase()) };
                format!(r"const el = (() => {{ for (const el of document.querySelectorAll('*')) {{ if (el.children.length === 0 && {cond}) return el; }} return null; }})();")
            }
            "label" => {
                let lower = escaped_val.to_lowercase();
                format!(r"const el = (() => {{ for (const lbl of document.querySelectorAll('label')) {{ if (lbl.textContent.toLowerCase().includes('{lower}')) {{ const forId = lbl.getAttribute('for'); if (forId) return document.getElementById(forId); return lbl.querySelector('input,select,textarea'); }} }} return null; }})();")
            }
            "placeholder" => {
                let cond = if exact { format!("e.placeholder === '{escaped_val}'") } else { format!("e.placeholder.toLowerCase().includes('{}')", escaped_val.to_lowercase()) };
                format!(r"const el = (() => {{ for (const e of document.querySelectorAll('[placeholder]')) {{ if ({cond}) return e; }} return null; }})();")
            }
            "testid" => format!(r#"const el = document.querySelector('[data-testid="{escaped_val}"]');"#),
            "alttext" | "alt" => {
                let cond = if exact { format!("e.alt === '{escaped_val}'") } else { format!("e.alt.toLowerCase().includes('{}')", escaped_val.to_lowercase()) };
                format!(r"const el = (() => {{ for (const e of document.querySelectorAll('[alt]')) {{ if ({cond}) return e; }} return null; }})();")
            }
            "title" => {
                let cond = if exact { format!("e.title === '{escaped_val}'") } else { format!("e.title.toLowerCase().includes('{}')", escaped_val.to_lowercase()) };
                format!(r"const el = (() => {{ for (const e of document.querySelectorAll('[title]')) {{ if ({cond}) return e; }} return null; }})();")
            }
            _ => "const el = null;".to_owned(),
        }
    }

    /// Select the nth element (0-based, -1 = last) and optionally act on it.
    pub async fn nth_action(&self, selector: &str, index: i64, subaction: Option<&str>, fill_value: Option<&str>) -> Result<serde_json::Value> {
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let resolve = format!(r"const els = document.querySelectorAll('{escaped}'); const idx = {index} < 0 ? els.length + {index} : {index}; if (idx < 0 || idx >= els.length) throw new Error('index {index} out of range, found ' + els.length + ' elements'); const el = els[idx];");
        let action_body = match subaction {
            None => format!(r"{resolve} return {{ tag: el.tagName.toLowerCase(), text: el.textContent.trim().substring(0, 100), index: idx, total: els.length }};"),
            Some("click") => format!(r"{resolve} el.scrollIntoView({{ block: 'center' }}); el.click(); return {{ action: 'click', tag: el.tagName.toLowerCase(), text: (el.textContent || '').trim().substring(0, 80) }};"),
            Some("fill") => {
                let fv = fill_value.unwrap_or("").replace('\\', "\\\\").replace('\'', "\\'");
                format!(r"{resolve} el.scrollIntoView({{ block: 'center' }}); el.focus(); el.value = ''; el.dispatchEvent(new Event('input', {{ bubbles: true }})); el.value = '{fv}'; el.dispatchEvent(new Event('input', {{ bubbles: true }})); el.dispatchEvent(new Event('change', {{ bubbles: true }})); return {{ action: 'fill', tag: el.tagName.toLowerCase(), value: '{fv}' }};")
            }
            Some("check") => format!(r"{resolve} if (!el.checked) el.click(); return {{ action: 'check', tag: el.tagName.toLowerCase(), checked: el.checked }};"),
            Some("hover") => format!(r"{resolve} el.scrollIntoView({{ block: 'center' }}); el.dispatchEvent(new MouseEvent('mouseover', {{ bubbles: true }})); el.dispatchEvent(new MouseEvent('mouseenter', {{ bubbles: true }})); return {{ action: 'hover', tag: el.tagName.toLowerCase(), text: (el.textContent || '').trim().substring(0, 80) }};"),
            Some("text") => format!(r"{resolve} return {{ action: 'text', text: (el.textContent || '').trim() }};"),
            Some(other) => return Err(Error::InvalidArgument(format!("unknown nth subaction '{other}'. Use: click, fill, check, hover, text"))),
        };
        self.eval(&format!("(async () => {{ {action_body} }})()")).await
    }
}
