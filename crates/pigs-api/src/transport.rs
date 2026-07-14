//! Transport abstraction for protocol-native phase subrequests.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::protocol::HttpRequestEnvelope;

/// Complete non-streaming response returned by one phase subrequest.
#[derive(Debug, Clone, PartialEq)]
pub struct TransportResponse {
    /// Complete protocol-native JSON response body.
    pub body: Value,
}

/// Errors returned by a phase transport.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The proxy or upstream returned a non-success status.
    #[error("phase subrequest failed with HTTP {status}: {message}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Response error text.
        message: String,
    },
    /// The transport could not complete or decode the request.
    #[error("phase transport failed: {0}")]
    Other(String),
}

/// Receives raw visible text deltas from a streaming phase subrequest.
pub type TransportTextSink = Arc<dyn Fn(String) + Send + Sync>;

/// Sends complete protocol-native phase requests without prescribing HTTP machinery.
#[async_trait]
pub trait PhaseTransport: Send + Sync {
    /// Sends one phase request and returns its complete native JSON response.
    async fn send(&self, request: HttpRequestEnvelope)
        -> Result<TransportResponse, TransportError>;

    /// Sends one phase request while reporting raw visible text deltas.
    async fn send_streaming(
        &self,
        request: HttpRequestEnvelope,
        text: TransportTextSink,
    ) -> Result<TransportResponse, TransportError> {
        let response = self.send(request.clone()).await?;
        let output = request
            .extract_response(&response.body)
            .map_err(|error| TransportError::Other(error.to_string()))?;
        if !output.visible_text.is_empty() {
            text(output.visible_text);
        }
        Ok(response)
    }
}
