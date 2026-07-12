//! MCP stdio client with Content-Length framing.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};
use tracing::{debug, warn};

use crate::error::McpError;
use crate::protocol::{
    CallToolParams, CallToolResult, ClientCapabilities, Implementation, InitializeParams,
    JsonRpcRequest, JsonRpcResponse, ListToolsResult, McpToolDefinition,
};

/// Configuration for an MCP server process.
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

/// Metadata for a tool exposed by an MCP server.
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub server_name: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// A connected MCP server session.
struct McpSession {
    #[allow(dead_code)]
    config: McpServerConfig,
    child: Child,
    stdin: ChildStdin,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    next_id: AtomicU64,
    tools: Vec<McpToolInfo>,
}

/// MCP client managing one or more stdio server connections.
pub struct McpClient {
    sessions: Mutex<HashMap<String, McpSession>>,
}

impl Default for McpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl McpClient {
    /// Create an empty MCP client.
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Connect to an MCP server, initialize it, and list tools.
    pub async fn connect(&self, config: McpServerConfig) -> Result<Vec<McpToolInfo>, McpError> {
        let mut sessions = self.sessions.lock().await;
        if sessions.contains_key(&config.name) {
            return Err(McpError::AlreadyConnected(config.name));
        }

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::Spawn(format!("{}: {e}", config.command)))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Io("Failed to open stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Io("Failed to open stdout".into()))?;

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Spawn reader task
        let pending_reader = Arc::clone(&pending);
        let server_name = config.name.clone();
        tokio::spawn(async move {
            if let Err(e) = read_loop(stdout, pending_reader).await {
                warn!(server = %server_name, error = %e, "MCP reader loop ended");
            }
        });

        let mut session = McpSession {
            config: config.clone(),
            child,
            stdin,
            pending,
            next_id: AtomicU64::new(1),
            tools: Vec::new(),
        };

        // initialize
        let init_params = InitializeParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "pigs".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };
        let init_value = serde_json::to_value(init_params)
            .map_err(|e| McpError::InvalidResponse(e.to_string()))?;
        let _ = session
            .request("initialize", Some(init_value))
            .await?;

        // notifications/initialized (no response expected)
        let notify = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        session.write_message(&notify).await?;

        // tools/list
        let list_result = session.request("tools/list", Some(serde_json::json!({}))).await?;
        let tools_result: ListToolsResult = serde_json::from_value(list_result)
            .map_err(|e| McpError::InvalidResponse(format!("tools/list parse: {e}")))?;

        session.tools = tools_result
            .tools
            .into_iter()
            .map(|t: McpToolDefinition| McpToolInfo {
                server_name: config.name.clone(),
                name: t.name,
                description: t.description.unwrap_or_default(),
                input_schema: t.input_schema.unwrap_or_else(|| serde_json::json!({"type":"object"})),
            })
            .collect();

        let tools = session.tools.clone();
        debug!(
            server = %config.name,
            tool_count = tools.len(),
            "MCP server connected"
        );
        sessions.insert(config.name.clone(), session);
        Ok(tools)
    }

    /// Disconnect a server by name.
    pub async fn disconnect(&self, name: &str) -> Result<(), McpError> {
        let mut sessions = self.sessions.lock().await;
        if let Some(mut session) = sessions.remove(name) {
            let _ = session.child.kill().await;
            Ok(())
        } else {
            Err(McpError::ServerNotFound(name.to_string()))
        }
    }

    /// List connected server names.
    pub async fn list_servers(&self) -> Vec<String> {
        let sessions = self.sessions.lock().await;
        sessions.keys().cloned().collect()
    }

    /// List all tools across connected servers.
    pub async fn list_tools(&self) -> Vec<McpToolInfo> {
        let sessions = self.sessions.lock().await;
        sessions
            .values()
            .flat_map(|s| s.tools.clone())
            .collect()
    }

    /// Call a tool on a specific server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<CallToolResult, McpError> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;

        let params = CallToolParams {
            name: tool_name.to_string(),
            arguments: Some(arguments),
        };
        let params_value =
            serde_json::to_value(params).map_err(|e| McpError::InvalidResponse(e.to_string()))?;
        let result = session.request("tools/call", Some(params_value)).await?;
        serde_json::from_value(result)
            .map_err(|e| McpError::InvalidResponse(format!("tools/call parse: {e}")))
    }
}

impl McpSession {
    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        let req = JsonRpcRequest::new(id, method, params);
        let value =
            serde_json::to_value(&req).map_err(|e| McpError::InvalidResponse(e.to_string()))?;
        self.write_message(&value).await?;

        let response = tokio::time::timeout(Duration::from_secs(60), rx)
            .await
            .map_err(|_| McpError::Timeout)?
            .map_err(|_| McpError::Io("Response channel closed".into()))?;

        if let Some(err) = response.error {
            return Err(McpError::Rpc {
                code: err.code,
                message: err.message,
            });
        }

        response
            .result
            .ok_or_else(|| McpError::InvalidResponse("Missing result field".into()))
    }

    async fn write_message(&mut self, value: &Value) -> Result<(), McpError> {
        let body = serde_json::to_vec(value)
            .map_err(|e| McpError::InvalidResponse(e.to_string()))?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.stdin
            .write_all(header.as_bytes())
            .await
            .map_err(|e| McpError::Io(e.to_string()))?;
        self.stdin
            .write_all(&body)
            .await
            .map_err(|e| McpError::Io(e.to_string()))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| McpError::Io(e.to_string()))?;
        Ok(())
    }
}

/// Read Content-Length framed messages and dispatch responses.
async fn read_loop(
    stdout: ChildStdout,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
) -> Result<(), McpError> {
    let mut reader = BufReader::new(stdout);
    loop {
        // Read headers
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let n = reader
                .read_line(&mut line)
                .await
                .map_err(|e| McpError::Io(e.to_string()))?;
            if n == 0 {
                return Ok(()); // EOF
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                let len = rest
                    .trim()
                    .parse::<usize>()
                    .map_err(|e| McpError::InvalidResponse(format!("Bad Content-Length: {e}")))?;
                content_length = Some(len);
            }
        }

        let len = content_length
            .ok_or_else(|| McpError::InvalidResponse("Missing Content-Length".into()))?;
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .await
            .map_err(|e| McpError::Io(e.to_string()))?;

        let response: JsonRpcResponse = match serde_json::from_slice(&buf) {
            Ok(r) => r,
            Err(e) => {
                // May be a notification — ignore
                debug!(error = %e, "Ignoring non-response MCP message");
                continue;
            }
        };

        if let Some(id_value) = response.id.clone() {
            let id = match id_value {
                Value::Number(n) => n.as_u64(),
                Value::String(s) => s.parse::<u64>().ok(),
                _ => None,
            };
            if let Some(id) = id {
                let mut pending_map = pending.lock().await;
                if let Some(tx) = pending_map.remove(&id) {
                    let _ = tx.send(response);
                }
            }
        }
    }
}
