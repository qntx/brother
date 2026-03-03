//! `JavaScript` evaluation methods.

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Evaluate a `JavaScript` expression and return the raw result.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn eval(&self, expression: &str) -> Result<serde_json::Value> {
        let result = self.inner.evaluate(expression).await.map_err(Error::Cdp)?;
        Ok(result
            .into_value::<serde_json::Value>()
            .unwrap_or(serde_json::Value::Null))
    }

    /// Evaluate JS and deserialize the result.
    ///
    /// # Errors
    ///
    /// Returns an error if evaluation or deserialization fails.
    pub async fn eval_as<T: serde::de::DeserializeOwned>(&self, expression: &str) -> Result<T> {
        let result = self.inner.evaluate(expression).await.map_err(Error::Cdp)?;
        result
            .into_value()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e.to_string())))
    }
}
