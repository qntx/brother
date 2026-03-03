//! File upload, drag, clear, scroll-into-view, bounding-box, set-content, PDF.

use std::time::Duration;

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::input::DispatchMouseEventType;
use chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams;

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Upload files to a `<input type="file">` element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found or the CDP call fails.
    pub async fn upload(&self, target: &str, files: &[String]) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::dom::{
            DescribeNodeParams, SetFileInputFilesParams,
        };

        let object_id = self.resolve_target_object(target).await?;

        // Resolve the DOM node (backendNodeId) from the remote object.
        let desc_params = DescribeNodeParams {
            object_id: Some(object_id),
            ..Default::default()
        };
        let desc = self.inner.execute(desc_params).await.map_err(Error::Cdp)?;
        let backend_node_id = desc.result.node.backend_node_id;

        let mut params = SetFileInputFilesParams::new(files.to_vec());
        params.backend_node_id = Some(backend_node_id);
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Drag one element onto another.
    ///
    /// Simulates a full drag gesture: mousedown on source center, mousemove to
    /// target center, mouseup on target center.
    ///
    /// # Errors
    ///
    /// Returns an error if either element is not found.
    pub async fn drag(&self, source: &str, target: &str) -> Result<()> {
        let src = self.resolve_target_center(source).await?;
        let dst = self.resolve_target_center(target).await?;

        // mousedown on source
        self.dispatch_mouse(DispatchMouseEventType::MousePressed, src, 1)
            .await?;
        // small pause to let drag start
        tokio::time::sleep(Duration::from_millis(50)).await;
        // mousemove to target
        self.dispatch_mouse(DispatchMouseEventType::MouseMoved, dst, 0)
            .await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        // mouseup on target
        self.dispatch_mouse(DispatchMouseEventType::MouseReleased, dst, 1)
            .await?;
        Ok(())
    }

    /// Clear an input field by filling it with an empty string.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn clear(&self, target: &str) -> Result<()> {
        self.fill(target, "").await
    }

    /// Scroll an element into the visible viewport.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn scroll_into_view(&self, target: &str) -> Result<()> {
        let object_id = self.resolve_target_object(target).await?;
        let js = "function(){this.scrollIntoView({block:'center',inline:'center'})}";
        let params = CallFunctionOnParams::builder()
            .object_id(object_id)
            .function_declaration(js)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Get the bounding box (x, y, width, height) of an element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn bounding_box(&self, target: &str) -> Result<(f64, f64, f64, f64)> {
        let object_id = self.resolve_target_object(target).await?;
        let js = "function(){const r=this.getBoundingClientRect();return JSON.stringify({x:r.x,y:r.y,width:r.width,height:r.height})}";
        let params = CallFunctionOnParams::builder()
            .object_id(object_id)
            .function_declaration(js)
            .return_by_value(true)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        let json_str: String = resp
            .result
            .result
            .value
            .as_ref()
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| Error::Browser("bounding_box returned no value".into()))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).map_err(|e| Error::Browser(e.to_string()))?;
        let x = parsed["x"].as_f64().unwrap_or(0.0);
        let y = parsed["y"].as_f64().unwrap_or(0.0);
        let w = parsed["width"].as_f64().unwrap_or(0.0);
        let h = parsed["height"].as_f64().unwrap_or(0.0);
        Ok((x, y, w, h))
    }

    /// Set the page HTML content directly.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP call fails.
    pub async fn set_content(&self, html: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::SetDocumentContentParams;
        let frame_id = self
            .inner
            .mainframe()
            .await
            .map_err(Error::Cdp)?
            .ok_or_else(|| Error::Navigation("no main frame".into()))?;
        let params = SetDocumentContentParams::new(frame_id, html.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Export the page as PDF and write to the given path.
    ///
    /// Only works in headless mode.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP call fails or writing fails.
    pub async fn pdf(&self, path: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
        let params = PrintToPdfParams::default();
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&resp.result.data)
            .map_err(|e| Error::Browser(format!("base64 decode: {e}")))?;
        tokio::fs::write(path, bytes)
            .await
            .map_err(|e| Error::Browser(format!("write PDF: {e}")))?;
        Ok(())
    }
}
