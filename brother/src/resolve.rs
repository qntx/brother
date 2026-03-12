//! Target resolution — translating refs and CSS selectors to CDP objects.
//!
//! All page interaction methods accept a unified **target** string:
//! - A ref from a prior snapshot: `"@e1"`, `"e1"`, or `"ref=e1"`
//! - A CSS selector: `"#submit"`, `".btn-primary"`

use chromiumoxide::cdp::browser_protocol::dom::{BackendNodeId, FocusParams, ResolveNodeParams};
use chromiumoxide::cdp::js_protocol::runtime::{CallFunctionOnParams, EvaluateParams};
use chromiumoxide::layout::Point;

use crate::error::{Error, Result};
use crate::page::Page;
use crate::snapshot::Ref;

impl Page {
    fn is_ref(target: &str) -> bool {
        target.starts_with('@')
            || target.starts_with("ref=")
            || (target.starts_with('e')
                && target.len() > 1
                && target[1..].bytes().all(|b| b.is_ascii_digit()))
    }

    fn normalize_ref(target: &str) -> &str {
        let s = target.strip_prefix('@').unwrap_or(target);
        s.strip_prefix("ref=").unwrap_or(s)
    }

    pub(crate) async fn try_resolve_ref(&self, target: &str) -> Option<Ref> {
        if !Self::is_ref(target) {
            return None;
        }
        let id = Self::normalize_ref(target);
        self.refs.lock().await.get(id).cloned()
    }

    pub(crate) async fn resolve_target_object(
        &self,
        target: &str,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        if let Some(r) = self.try_resolve_ref(target).await {
            return self.resolve_ref_to_object(&r).await;
        }
        let escaped = target.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!("document.querySelector('{escaped}')");
        let params = EvaluateParams::builder()
            .expression(js)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let result = self.inner.execute(params).await.map_err(Error::Cdp)?;
        result
            .result
            .result
            .object_id
            .ok_or_else(|| Error::ElementNotFound(format!("selector \"{target}\" not found")))
    }

    /// Resolve any target to its center point for click/hover.
    pub async fn resolve_target_center(&self, target: &str) -> Result<Point> {
        let oid = self.resolve_target_object(target).await?;
        self.get_center_from_object(oid).await
    }

    pub(crate) async fn call_fn_on(
        &self,
        oid: chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId,
        function: &str,
    ) -> Result<Option<serde_json::Value>> {
        let params = CallFunctionOnParams::builder()
            .object_id(oid)
            .function_declaration(function)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(resp.result.result.value)
    }

    pub(crate) async fn call_on_target(&self, target: &str, function: &str) -> Result<()> {
        let oid = self.resolve_target_object(target).await?;
        self.call_fn_on(oid, function).await?;
        Ok(())
    }

    pub(crate) async fn call_bool_on_target(
        &self,
        target: &str,
        function: &str,
    ) -> Result<bool> {
        let oid = self.resolve_target_object(target).await?;
        let val = self.call_fn_on(oid, function).await?;
        Ok(val.and_then(|v| v.as_bool()).unwrap_or(false))
    }

    pub(crate) async fn call_text_on_target(
        &self,
        target: &str,
        function: &str,
    ) -> Result<String> {
        let oid = self.resolve_target_object(target).await?;
        let val = self.call_fn_on(oid, function).await?;
        Ok(val
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default())
    }

    async fn resolve_ref_to_object(
        &self,
        r: &Ref,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        if r.backend_node_id != 0 {
            if let Ok(oid) = self.resolve_backend_node(r.backend_node_id).await {
                return Ok(oid);
            }
            tracing::debug!(role = %r.role, name = %r.name, "backend_node_id stale, falling back to role+name");
        }
        self.resolve_by_role_name(&r.role, &r.name, r.nth).await
    }

    pub(crate) async fn focus_ref_element(&self, r: &Ref) -> Result<()> {
        if r.backend_node_id != 0 {
            let ok = self
                .inner
                .execute(FocusParams {
                    node_id: None,
                    backend_node_id: Some(BackendNodeId::new(r.backend_node_id)),
                    object_id: None,
                })
                .await;
            if ok.is_ok() {
                return Ok(());
            }
        }
        let oid = self.resolve_by_role_name(&r.role, &r.name, r.nth).await?;
        let params = CallFunctionOnParams::builder()
            .object_id(oid)
            .function_declaration("function() { this.focus(); }")
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    async fn resolve_by_role_name(
        &self,
        role: &str,
        name: &str,
        nth: Option<usize>,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        let nth_idx = nth.unwrap_or(0);
        let esc_name = name.replace('\\', "\\\\").replace('\'', "\\'");
        let esc_role = role.replace('\\', "\\\\").replace('\'', "\\'");

        let js = format!(
            r#"(() => {{
                const R = {{
                    button: 'button,[role="button"],input[type="button"],input[type="submit"]',
                    link: 'a[href],[role="link"]',
                    textbox: 'input:not([type]),input[type="text"],input[type="email"],input[type="password"],input[type="search"],input[type="url"],input[type="tel"],input[type="number"],textarea,[role="textbox"],[contenteditable="true"]',
                    checkbox: 'input[type="checkbox"],[role="checkbox"]',
                    radio: 'input[type="radio"],[role="radio"]',
                    combobox: 'select,[role="combobox"]',
                    heading: 'h1,h2,h3,h4,h5,h6,[role="heading"]',
                    listbox: 'select[multiple],[role="listbox"]',
                    menuitem: '[role="menuitem"]',
                    option: 'option,[role="option"]',
                    slider: 'input[type="range"],[role="slider"]',
                    switch: '[role="switch"]',
                    tab: '[role="tab"]',
                    searchbox: 'input[type="search"],[role="searchbox"]',
                    spinbutton: 'input[type="number"],[role="spinbutton"]',
                }};
                const sel = R['{esc_role}'] || '[role="{esc_role}"]';
                const m = [];
                for (const el of document.querySelectorAll(sel)) {{
                    const n = el.getAttribute('aria-label')
                        || el.getAttribute('title')
                        || el.getAttribute('alt')
                        || el.getAttribute('placeholder')
                        || el.textContent?.trim()
                        || el.value || '';
                    if ('{esc_name}' === '' || n.trim() === '{esc_name}' || n.trim().startsWith('{esc_name}'))
                        m.push(el);
                }}
                return m[Math.min({nth_idx}, m.length - 1)] || null;
            }})()"#
        );

        let params = EvaluateParams::builder()
            .expression(js)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let result = self.inner.execute(params).await.map_err(Error::Cdp)?;
        result.result.result.object_id.ok_or_else(|| {
            Error::ElementNotFound(format!(
                "element role={role} name=\"{name}\" not found in DOM"
            ))
        })
    }

    async fn resolve_backend_node(
        &self,
        id: i64,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        let result = self
            .inner
            .execute(ResolveNodeParams {
                node_id: None,
                backend_node_id: Some(BackendNodeId::new(id)),
                object_group: Some("brother".to_owned()),
                execution_context_id: None,
            })
            .await
            .map_err(Error::Cdp)?;
        result
            .result
            .object
            .object_id
            .ok_or_else(|| Error::ElementNotFound("node has no object id".into()))
    }

    pub(crate) async fn get_center_from_object(
        &self,
        oid: chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId,
    ) -> Result<Point> {
        let params = CallFunctionOnParams::builder()
            .object_id(oid)
            .function_declaration(
                "function(){const r=this.getBoundingClientRect();\
                 return JSON.stringify({x:r.x+r.width/2,y:r.y+r.height/2})}",
            )
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        let s = resp
            .result
            .result
            .value
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| Error::ElementNotFound("failed to get bounding rect".into()))?;
        let v: serde_json::Value = serde_json::from_str(&s)?;
        Ok(Point {
            x: v["x"].as_f64().unwrap_or(0.0),
            y: v["y"].as_f64().unwrap_or(0.0),
        })
    }

    pub(crate) async fn find_element(
        &self,
        selector: &str,
    ) -> Result<chromiumoxide::element::Element> {
        self.inner
            .find_element(selector)
            .await
            .map_err(|_| Error::ElementNotFound(format!("selector \"{selector}\" not found")))
    }
}
