//! Opt-in local-only MCP server: settings, bearer token persistence, JSON-RPC 2.0
//! protocol dispatch, and safe fleet tools (`list_agents`, `get_agent_status`,
//! `get_diff`, `read_summary`, `spawn_agent`, `send_message`).
//!
//! Security model:
//! - Strictly opt-in (`enabled = false` by default).
//! - Bound to `127.0.0.1` loopback ONLY (enforced by the Tauri shell HTTP server).
//! - Every HTTP request requires a random 256-bit bearer token stored in `data_dir`
//!   with owner-only (`0600`) file permissions (never in SQLite, logs, or diagnostics).
//! - Read tools (`allow_read_tools`, default `true`) and write tools
//!   (`allow_write_tools`, default `false`) are independently gated.
//! - Every returned text payload passes through `redact::redact_secrets`, strips the
//!   active bearer token if present, and enforces strict byte caps.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::manager::Core;
use crate::model::{
    Account, CreateAgentRequest, ItemBody, KeyValue, McpServer, McpTransport, Provider,
};

pub const DEFAULT_LOCAL_MCP_PORT: u16 = 47822;
pub const MIN_LOCAL_MCP_PORT: u16 = 1024;
pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "kvistcode";
pub const TOKEN_FILE_NAME: &str = "local-mcp-token";

const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;
const DEFAULT_DIFF_MAX_BYTES: usize = 48 * 1024;
const MAX_DIFF_BYTES: usize = 64 * 1024;
const DEFAULT_SUMMARY_MAX_BYTES: usize = 32 * 1024;
const MAX_SUMMARY_BYTES: usize = 64 * 1024;
const MAX_STATUS_BYTES: usize = 64 * 1024;
const GIT_DIFF_TIMEOUT: Duration = Duration::from_secs(30);

pub const READ_TOOL_NAMES: &[&str] = &[
    "list_agents",
    "get_agent_status",
    "get_diff",
    "read_summary",
];
pub const WRITE_TOOL_NAMES: &[&str] = &["spawn_agent", "send_message"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalMcpSettings {
    /// Serve Kvistcode's local MCP endpoint on `127.0.0.1:<port>/mcp`. Strictly opt-in.
    pub enabled: bool,
    /// Loopback port (`127.0.0.1` only; never bound to `0.0.0.0` or LAN).
    pub port: u16,
    /// Expose read-only fleet tools (`list_agents`, `get_agent_status`, `get_diff`, `read_summary`).
    pub allow_read_tools: bool,
    /// Explicit opt-in for mutating tools (`spawn_agent`, `send_message`).
    pub allow_write_tools: bool,
    /// Automatically attach Kvistcode's local MCP server to spawned Kvistcode agents.
    pub expose_to_agents: bool,
}

impl Default for LocalMcpSettings {
    fn default() -> Self {
        LocalMcpSettings {
            enabled: false,
            port: DEFAULT_LOCAL_MCP_PORT,
            allow_read_tools: true,
            allow_write_tools: false,
            expose_to_agents: false,
        }
    }
}

impl LocalMcpSettings {
    /// What is wrong with these settings, in plain words (`None` = valid).
    pub fn problem(&self) -> Option<String> {
        if self.port < MIN_LOCAL_MCP_PORT {
            return Some(format!(
                "The local MCP port must be between {MIN_LOCAL_MCP_PORT} and 65535"
            ));
        }
        None
    }

    /// Repairs invalid stored values to defaults; returns whether anything changed.
    pub fn sanitize(&mut self) -> bool {
        if self.port < MIN_LOCAL_MCP_PORT {
            self.port = DEFAULT_LOCAL_MCP_PORT;
            return true;
        }
        false
    }

    /// Loopback endpoint URL (`http://127.0.0.1:<port>/mcp`).
    pub fn endpoint_url(&self) -> String {
        endpoint_url_for_port(self.port)
    }

    /// Names of tools currently permitted by `allow_read_tools` and `allow_write_tools`.
    pub fn enabled_tool_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.allow_read_tools {
            out.extend(READ_TOOL_NAMES.iter().map(|s| (*s).to_string()));
        }
        if self.allow_write_tools {
            out.extend(WRITE_TOOL_NAMES.iter().map(|s| (*s).to_string()));
        }
        out
    }
}

/// Loopback MCP endpoint URL for `port`.
pub fn endpoint_url_for_port(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

// ---- bearer token storage ---------------------------------------------------

pub fn token_path(data_dir: &Path) -> PathBuf {
    data_dir.join(TOKEN_FILE_NAME)
}

pub fn generate_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn is_valid_token(raw: &str) -> bool {
    raw.len() >= 32
        && raw.len() <= 256
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Constant-time string comparison for bearer tokens.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Masked token preview for safe display (e.g. `••••a1b2`).
pub fn token_preview(token: &str) -> String {
    let tail: String = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if tail.len() == 4 && token.len() >= 8 {
        format!("••••{tail}")
    } else {
        "••••".into()
    }
}

/// Reads the persisted bearer token if present and valid; does not create one.
pub fn read_token(data_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(token_path(data_dir)).ok()?;
    let trimmed = raw.trim();
    is_valid_token(trimmed).then(|| trimmed.to_string())
}

/// Loads the persisted bearer token, or generates and saves a fresh one with `0600`
/// permissions on Unix.
pub fn load_or_create_token(data_dir: &Path) -> anyhow::Result<String> {
    if let Some(existing) = read_token(data_dir) {
        return Ok(existing);
    }
    rotate_token(data_dir)
}

/// Generates a fresh bearer token and writes it atomically with `0600` permissions.
pub fn rotate_token(data_dir: &Path) -> anyhow::Result<String> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let token = generate_token();
    let path = token_path(data_dir);
    write_private_atomic(&path, token.as_bytes())
        .with_context(|| format!("saving local MCP token ({})", path.display()))?;
    Ok(token)
}

/// Writes `data` to `path` atomically, readable by the owner only (`0600` on Unix).
pub(crate) fn write_private_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

// ---- agent auto-injection & client config -----------------------------------

/// Builds the synthetic `McpServer` entry injected into spawned Kvistcode agents when
/// `settings.enabled && settings.expose_to_agents`.
pub fn agent_mcp_server(settings: &LocalMcpSettings, data_dir: &Path) -> Option<McpServer> {
    if !(settings.enabled && settings.expose_to_agents) {
        return None;
    }
    let token = match load_or_create_token(data_dir) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("could not load local MCP token for agent injection: {e:#}");
            return None;
        }
    };
    Some(McpServer {
        id: "kvistcode-local-mcp".into(),
        name: SERVER_NAME.into(),
        enabled: true,
        transport: McpTransport::Http,
        command: String::new(),
        args: Vec::new(),
        env: Vec::new(),
        url: settings.endpoint_url(),
        headers: vec![KeyValue {
            name: "Authorization".into(),
            value: format!("Bearer {token}"),
        }],
        project_path: String::new(),
    })
}

/// Ready-to-copy MCP JSON config snippet for external clients (Claude Code, Codex, Cursor).
pub fn client_config_snippet(url: &str, token: &str) -> String {
    serde_json::to_string_pretty(&json!({
        "mcpServers": {
            SERVER_NAME: {
                "type": "http",
                "url": url,
                "headers": {
                    "Authorization": format!("Bearer {token}")
                }
            }
        }
    }))
    .unwrap_or_default()
}

// ---- MCP JSON-RPC 2.0 protocol ----------------------------------------------

fn rpc_result(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        }
    })
}

fn tool_ok(text: &str, active_token: Option<&str>) -> Value {
    let clean = redact_and_cap(text, active_token, MAX_TOOL_OUTPUT_BYTES);
    json!({
        "content": [{ "type": "text", "text": clean }],
        "isError": false,
    })
}

fn tool_err(message: &str, active_token: Option<&str>) -> Value {
    let clean = redact_and_cap(message, active_token, 4096);
    json!({
        "content": [{ "type": "text", "text": clean }],
        "isError": true,
    })
}

/// Redacts recognizable secrets plus the active local MCP bearer token (if any) and caps
/// length at a UTF-8 character boundary.
fn redact_and_cap(text: &str, active_token: Option<&str>, max_bytes: usize) -> String {
    let mut out = crate::redact::redact_secrets(text).into_owned();
    if let Some(tok) = active_token.filter(|t| t.len() >= 8) {
        if out.contains(tok) {
            out = out.replace(tok, crate::redact::REDACTED);
        }
    }
    if out.len() <= max_bytes {
        return out;
    }
    let suffix = "\n… [truncated]";
    let keep = max_bytes.saturating_sub(suffix.len());
    let mut end = keep.min(out.len());
    while end > 0 && !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out.push_str(suffix);
    out
}

fn tool_definitions(settings: &LocalMcpSettings) -> Vec<Value> {
    let mut tools = Vec::new();
    if settings.allow_read_tools {
        tools.push(json!({
            "name": "list_agents",
            "description": "List Kvistcode agents (non-archived by default) with provider, status, project path, branch, worktree flag, last activity timestamp, and cost.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_path": {
                        "type": "string",
                        "description": "Optional project folder path filter."
                    },
                    "provider": {
                        "type": "string",
                        "description": "Optional provider filter (claude, codex, grok, gemini, opencode, cursor, local, custom)."
                    },
                    "include_archived": {
                        "type": "boolean",
                        "description": "Include archived agents (default false)."
                    }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "openWorldHint": false
            }
        }));
        tools.push(json!({
            "name": "get_agent_status",
            "description": "Get detailed status of one Kvistcode agent, including running state, pending permissions, queued prompts, last message, branch, base branch, cost, and token usage.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "The agent id."
                    }
                },
                "required": ["agent_id"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "openWorldHint": false
            }
        }));
        tools.push(json!({
            "name": "get_diff",
            "description": "Get bounded git diff stats and unified patch (with secrets redacted) for a Kvistcode agent's workspace.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "The agent id."
                    },
                    "max_bytes": {
                        "type": "integer",
                        "description": "Optional patch size cap in bytes (1024..65536, default 49152)."
                    }
                },
                "required": ["agent_id"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "openWorldHint": false
            }
        }));
        tools.push(json!({
            "name": "read_summary",
            "description": "Read the latest assistant messages and turn summary (redacted and bounded) for a Kvistcode agent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "The agent id."
                    },
                    "max_messages": {
                        "type": "integer",
                        "description": "Maximum recent assistant messages to include (1..20, default 5)."
                    },
                    "max_bytes": {
                        "type": "integer",
                        "description": "Maximum total text bytes to include (1024..65536, default 32768)."
                    }
                },
                "required": ["agent_id"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "openWorldHint": false
            }
        }));
    }
    if settings.allow_write_tools {
        tools.push(json!({
            "name": "spawn_agent",
            "description": "Spawn a new Kvistcode agent on a chosen provider (e.g. claude, codex, gemini, grok, opencode, cursor) using that provider's active/default account, optionally in its own git worktree.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "provider": {
                        "type": "string",
                        "description": "Provider name: claude, codex, grok, gemini, opencode, cursor, local, or custom."
                    },
                    "project_path": {
                        "type": "string",
                        "description": "Absolute path to the project directory."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Initial prompt / task instructions for the new agent."
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional title for the agent."
                    },
                    "worktree": {
                        "type": "boolean",
                        "description": "Create an isolated git worktree (default true)."
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional model identifier."
                    },
                    "account_id": {
                        "type": "string",
                        "description": "Optional explicit account id (otherwise Kvistcode picks the provider's active/default account)."
                    },
                    "permission_mode": {
                        "type": "string",
                        "description": "Optional permission mode: auto or ask."
                    }
                },
                "required": ["provider", "project_path", "prompt"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "openWorldHint": false
            }
        }));
        tools.push(json!({
            "name": "send_message",
            "description": "Send a follow-up prompt/message to an existing Kvistcode agent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "The target agent id."
                    },
                    "message": {
                        "type": "string",
                        "description": "Prompt text to send to the agent."
                    },
                    "deliver": {
                        "type": "boolean",
                        "description": "When true (default), deliver mid-turn if the agent supports steering, otherwise queue/start."
                    }
                },
                "required": ["agent_id", "message"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "openWorldHint": false
            }
        }));
    }
    tools
}

/// Dispatches one MCP JSON-RPC 2.0 message against `core` under `settings`.
/// Returns `None` for notifications (no `id` field), where Streamable HTTP responds `202 Accepted`.
pub async fn handle_jsonrpc(
    core: &Core,
    settings: &LocalMcpSettings,
    body: Value,
) -> Option<Value> {
    let Some(obj) = body.as_object() else {
        return Some(rpc_error(
            Value::Null,
            -32600,
            "Invalid Request: expected a JSON object",
        ));
    };
    let id = obj.get("id").cloned();
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(rpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "Invalid Request: jsonrpc must be \"2.0\"",
        ));
    }
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        return Some(rpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "Invalid Request: missing method",
        ));
    };
    let params = obj.get("params").cloned().unwrap_or_else(|| json!({}));

    // Notifications carry no `id`: acknowledge silently with HTTP 202.
    let id = id?;

    match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            let negotiated = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
                requested
            } else {
                PROTOCOL_VERSION
            };
            Some(rpc_result(
                id,
                json!({
                    "protocolVersion": negotiated,
                    "capabilities": {
                        "tools": { "listChanged": false }
                    },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "title": "Kvistcode",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": "Kvistcode local fleet MCP server. Use read tools to inspect sibling agents and write tools (when enabled) to spawn or message agents."
                }),
            ))
        }
        "ping" => Some(rpc_result(id, json!({}))),
        "tools/list" => Some(rpc_result(
            id,
            json!({ "tools": tool_definitions(settings) }),
        )),
        "tools/call" => Some(handle_tool_call(core, settings, id, &params).await),
        _ => Some(rpc_error(id, -32601, format!("Method not found: {method}"))),
    }
}

async fn handle_tool_call(
    core: &Core,
    settings: &LocalMcpSettings,
    id: Value,
    params: &Value,
) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return rpc_error(id, -32602, "Invalid params: missing tool name");
    };
    let args = params
        .get("arguments")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let active_token = read_token(core.data_dir());

    if READ_TOOL_NAMES.contains(&name) && !settings.allow_read_tools {
        return rpc_result(
            id,
            tool_err(
                "Read tools are disabled in Kvistcode's Local MCP Server settings.",
                active_token.as_deref(),
            ),
        );
    }
    if WRITE_TOOL_NAMES.contains(&name) && !settings.allow_write_tools {
        return rpc_result(
            id,
            tool_err(
                "Write tools (spawn_agent, send_message) are disabled in Kvistcode's Local MCP Server settings.",
                active_token.as_deref(),
            ),
        );
    }

    let outcome = match name {
        "list_agents" => tool_list_agents(core, &args),
        "get_agent_status" => tool_get_agent_status(core, &args),
        "get_diff" => tool_get_diff(core, &args, active_token.as_deref()).await,
        "read_summary" => tool_read_summary(core, &args, active_token.as_deref()),
        "spawn_agent" => tool_spawn_agent(core, &args).await,
        "send_message" => tool_send_message(core, &args).await,
        _ => return rpc_error(id, -32602, format!("Unknown tool: {name}")),
    };

    match outcome {
        Ok(value) => {
            let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into());
            rpc_result(id, tool_ok(&text, active_token.as_deref()))
        }
        Err(e) => rpc_result(id, tool_err(&format!("{e:#}"), active_token.as_deref())),
    }
}

// ---- fleet tools ------------------------------------------------------------

fn required_str<'a>(
    args: &'a serde_json::Map<String, Value>,
    key: &str,
) -> anyhow::Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("Missing required parameter `{key}`"))
}

fn tool_list_agents(core: &Core, args: &serde_json::Map<String, Value>) -> anyhow::Result<Value> {
    let include_archived = args
        .get("include_archived")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let project_filter = args
        .get("project_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let provider_filter = args
        .get("provider")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());

    let mut agents = core.agents()?;
    if !include_archived {
        agents.retain(|a| !a.archived);
    }
    if let Some(proj) = project_filter {
        let want = crate::providers::expand_home(proj);
        agents.retain(|a| {
            let ap = crate::providers::expand_home(&a.project_path);
            ap == want
                || match (std::fs::canonicalize(&ap), std::fs::canonicalize(&want)) {
                    (Ok(x), Ok(y)) => x == y,
                    _ => false,
                }
        });
    }
    if let Some(prov) = provider_filter.as_deref() {
        agents.retain(|a| a.provider.as_str() == prov);
    }

    let list: Vec<Value> = agents
        .into_iter()
        .map(|a| {
            json!({
                "id": a.id,
                "title": crate::redact::redact_secrets(&a.title),
                "provider": a.provider.as_str(),
                "status": a.status.as_str(),
                "running": a.running,
                "projectPath": a.project_path,
                "branch": a.branch,
                "worktree": a.worktree,
                "lastActivityAt": a.last_activity_at,
                "costUsd": a.cost_usd,
            })
        })
        .collect();
    Ok(json!({ "count": list.len(), "agents": list }))
}

fn tool_get_agent_status(
    core: &Core,
    args: &serde_json::Map<String, Value>,
) -> anyhow::Result<Value> {
    let agent_id = required_str(args, "agent_id")?;
    let a = core.agent(agent_id)?;
    let last_message = a
        .last_message
        .as_deref()
        .map(|m| redact_and_cap(m, None, 2048));
    Ok(json!({
        "id": a.id,
        "title": crate::redact::redact_secrets(&a.title),
        "provider": a.provider.as_str(),
        "status": a.status.as_str(),
        "running": a.running,
        "projectPath": a.project_path,
        "cwd": a.cwd,
        "worktree": a.worktree,
        "branch": a.branch,
        "baseBranch": a.base_branch,
        "pendingPermissions": a.pending_permissions,
        "queuedPrompts": a.queued_prompts,
        "lastMessage": last_message,
        "costUsd": a.cost_usd,
        "tokensUsed": a.tokens_used,
        "contextUsed": a.context_used,
        "contextSize": a.context_size,
        "rateLimited": a.rate_limited,
        "archived": a.archived,
        "createdAt": a.created_at,
        "updatedAt": a.updated_at,
        "lastActivityAt": a.last_activity_at,
    }))
}

async fn run_git_capped(
    cwd: &Path,
    args: &[&str],
    max_stdout: usize,
) -> anyhow::Result<(String, bool, bool)> {
    use tokio::io::AsyncReadExt;

    let mut cmd = crate::git::hardened_command(cwd).await?;
    cmd.args(["-c", "core.quotepath=false"])
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("NO_COLOR", "1");
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().context("running git")?;
    let Some(mut stdout) = child.stdout.take() else {
        bail!("git produced no stdout pipe");
    };
    let Some(mut stderr) = child.stderr.take() else {
        bail!("git produced no stderr pipe");
    };
    let read_all = async {
        let out_fut = async {
            let mut buf = Vec::new();
            (&mut stdout)
                .take(max_stdout as u64 + 1)
                .read_to_end(&mut buf)
                .await?;
            let overflowed = buf.len() > max_stdout;
            if overflowed {
                buf.truncate(max_stdout);
                let _ = child.start_kill();
            }
            Ok::<_, std::io::Error>((buf, overflowed))
        };
        let err_fut = async {
            let mut buf = Vec::new();
            (&mut stderr).take(16 * 1024).read_to_end(&mut buf).await?;
            Ok::<_, std::io::Error>(buf)
        };
        let ((out_buf, overflowed), _err_buf) = tokio::try_join!(out_fut, err_fut)?;
        let status = child.wait().await?;
        Ok::<_, anyhow::Error>((
            String::from_utf8_lossy(&out_buf).into_owned(),
            overflowed,
            status.success(),
        ))
    };
    tokio::time::timeout(GIT_DIFF_TIMEOUT, read_all)
        .await
        .map_err(|_| anyhow!("git diff timed out"))?
}

async fn tool_get_diff(
    core: &Core,
    args: &serde_json::Map<String, Value>,
    active_token: Option<&str>,
) -> anyhow::Result<Value> {
    let agent_id = required_str(args, "agent_id")?;
    let max_bytes = args
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).clamp(1024, MAX_DIFF_BYTES))
        .unwrap_or(DEFAULT_DIFF_MAX_BYTES);

    let agent = core.agent(agent_id)?;
    let cwd = Path::new(&agent.cwd);
    if !cwd.is_dir() {
        bail!("Agent workspace folder does not exist: {}", agent.cwd);
    }
    if crate::git::repo_info(cwd).await.is_none() {
        return Ok(json!({
            "agentId": agent.id,
            "isGitRepo": false,
            "branch": agent.branch,
            "baseBranch": agent.base_branch,
            "stats": [],
            "uncommitted": 0,
            "truncated": false,
            "patch": "",
        }));
    }

    // Prefer the agent's base commit so worktree branches show both committed and
    // uncommitted work; fall back to HEAD when the base commit isn't reachable.
    let base_ref = match agent
        .base_commit
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        Some(commit) => {
            let rev = format!("{commit}^{{commit}}");
            match run_git_capped(cwd, &["rev-parse", "--verify", "--quiet", &rev], 256).await {
                Ok((_, _, true)) => commit.to_string(),
                _ => "HEAD".to_string(),
            }
        }
        None => "HEAD".to_string(),
    };

    let (numstat, _, _) = run_git_capped(
        cwd,
        &[
            "diff",
            "--numstat",
            "--no-ext-diff",
            "--no-textconv",
            &base_ref,
        ],
        MAX_STATUS_BYTES,
    )
    .await?;

    let mut stats: Vec<Value> = Vec::new();
    for line in numstat.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(add), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let binary = add == "-" || del == "-";
        stats.push(json!({
            "path": crate::redact::redact_secrets(path),
            "additions": add.parse::<i64>().unwrap_or(0),
            "deletions": del.parse::<i64>().unwrap_or(0),
            "binary": binary,
            "status": "changed",
        }));
    }

    let (status_out, _, _) = run_git_capped(
        cwd,
        &["status", "--porcelain=v1", "-uall"],
        MAX_STATUS_BYTES,
    )
    .await?;
    let mut uncommitted: i64 = 0;
    let mut untracked_added = 0usize;
    for line in status_out.lines() {
        if line.len() < 4 {
            continue;
        }
        uncommitted += 1;
        if line.starts_with("?? ") && untracked_added < 50 {
            let path = &line[3..];
            stats.push(json!({
                "path": crate::redact::redact_secrets(path),
                "additions": 0,
                "deletions": 0,
                "binary": false,
                "status": "untracked",
            }));
            untracked_added += 1;
        }
    }

    let (raw_patch, overflowed, _) = run_git_capped(
        cwd,
        &["diff", "--no-ext-diff", "--no-textconv", &base_ref],
        max_bytes,
    )
    .await?;
    let redacted_patch = redact_and_cap(&raw_patch, active_token, max_bytes);
    let truncated = overflowed || redacted_patch.ends_with("… [truncated]");

    Ok(json!({
        "agentId": agent.id,
        "isGitRepo": true,
        "branch": agent.branch,
        "baseBranch": agent.base_branch,
        "worktree": agent.worktree,
        "baseRef": base_ref,
        "stats": stats,
        "uncommitted": uncommitted,
        "truncated": truncated,
        "patch": redacted_patch,
    }))
}

fn tool_read_summary(
    core: &Core,
    args: &serde_json::Map<String, Value>,
    active_token: Option<&str>,
) -> anyhow::Result<Value> {
    let agent_id = required_str(args, "agent_id")?;
    let max_messages = args
        .get("max_messages")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).clamp(1, 20))
        .unwrap_or(5);
    let max_bytes = args
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).clamp(1024, MAX_SUMMARY_BYTES))
        .unwrap_or(DEFAULT_SUMMARY_MAX_BYTES);

    let agent = core.agent(agent_id)?;
    let items = core.items(agent_id)?;

    let latest_turn_summary = items.iter().rev().find_map(|item| match &item.body {
        ItemBody::TurnSummary {
            duration_ms,
            files,
            tool_calls,
            commands,
            cost_usd,
            tokens,
        } => Some(json!({
            "seq": item.seq,
            "ts": item.ts,
            "durationMs": duration_ms,
            "files": files
                .iter()
                .take(100)
                .map(|f| crate::redact::redact_secrets(f).into_owned())
                .collect::<Vec<_>>(),
            "toolCalls": tool_calls,
            "commands": commands,
            "costUsd": cost_usd,
            "tokens": tokens,
        })),
        _ => None,
    });

    // Take newest assistant messages first so we keep the most recent context within `max_bytes`.
    let mut picked: Vec<Value> = Vec::new();
    let mut used_bytes = 0usize;
    let mut truncated = false;
    for item in items.iter().rev() {
        let ItemBody::AgentMessage { text, .. } = &item.body else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        if picked.len() >= max_messages {
            truncated = true;
            break;
        }
        let remaining = max_bytes.saturating_sub(used_bytes);
        if remaining < 64 {
            truncated = true;
            break;
        }
        let clean = redact_and_cap(text, active_token, remaining);
        if clean.ends_with("… [truncated]") {
            truncated = true;
        }
        used_bytes += clean.len();
        picked.push(json!({
            "seq": item.seq,
            "ts": item.ts,
            "text": clean,
        }));
    }
    picked.reverse();

    let last_message = agent
        .last_message
        .as_deref()
        .map(|m| redact_and_cap(m, active_token, 2048));

    Ok(json!({
        "agentId": agent.id,
        "title": crate::redact::redact_secrets(&agent.title),
        "provider": agent.provider.as_str(),
        "status": agent.status.as_str(),
        "lastMessage": last_message,
        "latestTurnSummary": latest_turn_summary,
        "messages": picked,
        "truncated": truncated,
    }))
}

fn pick_default_account(accounts: &[Account], provider: Provider) -> Option<&Account> {
    let matching: Vec<&Account> = accounts.iter().filter(|a| a.provider == provider).collect();
    if matching.is_empty() {
        return None;
    }
    let logged_in = |a: &&Account| a.status.as_ref().is_some_and(|s| s.logged_in);
    matching
        .iter()
        .copied()
        .find(|a| logged_in(a) && a.is_system)
        .or_else(|| matching.iter().copied().find(logged_in))
        .or_else(|| matching.iter().copied().find(|a| a.is_system))
        .or_else(|| matching.first().copied())
}

async fn tool_spawn_agent(
    core: &Core,
    args: &serde_json::Map<String, Value>,
) -> anyhow::Result<Value> {
    let raw_provider = required_str(args, "provider")?;
    let provider = Provider::parse(&raw_provider.to_ascii_lowercase()).ok_or_else(|| {
        anyhow!(
            "Unknown provider `{raw_provider}` (use claude, codex, grok, gemini, opencode, cursor, local, or custom)"
        )
    })?;
    let project_path = required_str(args, "project_path")?.to_string();
    let prompt = required_str(args, "prompt")?.to_string();
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let worktree = args
        .get("worktree")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let model = args
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let permission_mode = args
        .get("permission_mode")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let accounts = core.accounts()?;
    let account_id = match args
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(explicit) => {
            let acct = accounts
                .iter()
                .find(|a| a.id == explicit)
                .ok_or_else(|| anyhow!("Account not found: {explicit}"))?;
            if acct.provider != provider {
                bail!(
                    "Account `{explicit}` belongs to {}, not {}",
                    acct.provider.as_str(),
                    provider.as_str()
                );
            }
            explicit.to_string()
        }
        None => pick_default_account(&accounts, provider)
            .map(|a| a.id.clone())
            .ok_or_else(|| {
                anyhow!(
                    "No account configured for provider `{}`. Add one in Kvistcode Settings → Providers.",
                    provider.as_str()
                )
            })?,
    };

    let req = CreateAgentRequest {
        provider,
        account_id,
        project_path,
        prompt,
        title,
        worktree,
        model,
        group_id: None,
        custom_agent_id: None,
        permission_mode,
        attachments: Vec::new(),
        config_choices: Default::default(),
    };
    let agent = core.create_agent(req).await?;
    Ok(json!({
        "id": agent.id,
        "title": crate::redact::redact_secrets(&agent.title),
        "provider": agent.provider.as_str(),
        "accountId": agent.account_id,
        "status": agent.status.as_str(),
        "projectPath": agent.project_path,
        "cwd": agent.cwd,
        "worktree": agent.worktree,
        "branch": agent.branch,
        "baseBranch": agent.base_branch,
    }))
}

async fn tool_send_message(
    core: &Core,
    args: &serde_json::Map<String, Value>,
) -> anyhow::Result<Value> {
    let agent_id = required_str(args, "agent_id")?;
    let message = args
        .get("message")
        .or_else(|| args.get("prompt"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("Missing required parameter `message`"))?;
    let deliver = args.get("deliver").and_then(Value::as_bool).unwrap_or(true);

    if deliver {
        core.deliver_prompt(agent_id, message, Vec::new()).await?;
    } else {
        core.send_prompt(agent_id, message, Vec::new()).await?;
    }
    let agent = core.agent(agent_id)?;
    Ok(json!({
        "ok": true,
        "agentId": agent.id,
        "status": agent.status.as_str(),
        "queuedPrompts": agent.queued_prompts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountStatus, AuthKind, KvistcodeEvent, Settings};
    use crate::EventSink;
    use std::sync::Arc;

    struct NoopSink;
    impl EventSink for NoopSink {
        fn emit(&self, _event: KvistcodeEvent) {}
    }

    #[test]
    fn settings_defaults_and_backward_compat() {
        let s: LocalMcpSettings = serde_json::from_str("{}").unwrap();
        assert!(!s.enabled);
        assert_eq!(s.port, DEFAULT_LOCAL_MCP_PORT);
        assert!(s.allow_read_tools);
        assert!(!s.allow_write_tools);
        assert!(!s.expose_to_agents);
        assert!(s.problem().is_none());

        let mut bad = LocalMcpSettings {
            port: 80,
            ..Default::default()
        };
        assert!(bad.problem().is_some());
        assert!(bad.sanitize());
        assert_eq!(bad.port, DEFAULT_LOCAL_MCP_PORT);

        // Older Settings JSON without `localMcp` deserializes cleanly.
        let full: Settings = serde_json::from_str(r#"{"theme":"gunmetal"}"#).unwrap();
        assert!(!full.local_mcp.enabled);
        assert_eq!(full.local_mcp.port, DEFAULT_LOCAL_MCP_PORT);
    }

    #[test]
    fn token_lifecycle_and_permissions() {
        let dir = crate::testutil::scratch("local-mcp-token");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_token(&dir).is_none());

        let first = load_or_create_token(&dir).unwrap();
        assert!(first.len() >= 64);
        assert_eq!(load_or_create_token(&dir).unwrap(), first);
        assert!(constant_time_eq(&first, &first));
        assert!(!constant_time_eq(&first, "wrong"));
        assert!(token_preview(&first).starts_with("••••"));
        assert!(!token_preview(&first).contains(&first));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(token_path(&dir))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        let rotated = rotate_token(&dir).unwrap();
        assert_ne!(rotated, first);
        assert_eq!(read_token(&dir).as_deref(), Some(rotated.as_str()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn redact_and_cap_strips_active_token_and_secrets() {
        let tok = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let secret = ["s", "k-ant-api03-AbCdEf_ghIJKL-mnopqrstuvwx1234"].concat();
        let text = format!("token={tok} api={secret} ok");
        let clean = redact_and_cap(&text, Some(tok), 4096);
        assert!(!clean.contains(tok), "{clean}");
        assert!(!clean.contains(&secret), "{clean}");
        assert!(clean.contains("ok"), "{clean}");

        let long = "a".repeat(200);
        let capped = redact_and_cap(&long, None, 64);
        assert!(capped.len() <= 64);
        assert!(capped.ends_with("… [truncated]"));
    }

    #[test]
    fn agent_mcp_entry_requires_enabled_and_expose() {
        let dir = crate::testutil::scratch("local-mcp-entry");
        std::fs::create_dir_all(&dir).unwrap();

        let mut cfg = LocalMcpSettings::default();
        assert!(agent_mcp_server(&cfg, &dir).is_none());

        cfg.enabled = true;
        assert!(agent_mcp_server(&cfg, &dir).is_none());

        cfg.expose_to_agents = true;
        cfg.port = 48123;
        let srv = agent_mcp_server(&cfg, &dir).unwrap();
        assert_eq!(srv.name, "kvistcode");
        assert_eq!(srv.transport, McpTransport::Http);
        assert_eq!(srv.url, "http://127.0.0.1:48123/mcp");
        assert_eq!(srv.headers.len(), 1);
        assert_eq!(srv.headers[0].name, "Authorization");
        assert!(srv.headers[0].value.starts_with("Bearer "));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pick_default_account_prefers_logged_in_system() {
        let mk = |id: &str, provider: Provider, is_system: bool, logged_in: bool| Account {
            id: id.into(),
            provider,
            name: id.into(),
            color: "#fff".into(),
            auth_kind: AuthKind::Default,
            is_system,
            config_dir: None,
            created_at: 0,
            status: Some(AccountStatus {
                logged_in,
                label: None,
                email: None,
                plan: None,
                checked_at: 0,
                error: None,
            }),
            usage: None,
            local: None,
        };
        let list = vec![
            mk("a1", Provider::Codex, false, false),
            mk("a2", Provider::Codex, false, true),
            mk("a3", Provider::Codex, true, true),
        ];
        assert_eq!(
            pick_default_account(&list, Provider::Codex).map(|a| a.id.as_str()),
            Some("a3")
        );
        assert!(pick_default_account(&list, Provider::Grok).is_none());
    }

    #[tokio::test]
    async fn jsonrpc_protocol_and_permission_gates() {
        let dir = crate::testutil::scratch("local-mcp-rpc");
        std::fs::create_dir_all(&dir).unwrap();
        let core = Core::open(dir.clone(), Arc::new(NoopSink)).unwrap();
        let read_only = LocalMcpSettings {
            enabled: true,
            allow_read_tools: true,
            allow_write_tools: false,
            ..Default::default()
        };

        // Initialize handshake
        let init = handle_jsonrpc(
            &core,
            &read_only,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-06-18" }
            }),
        )
        .await
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(init["result"]["serverInfo"]["name"], "kvistcode");

        // Notification returns None (HTTP 202)
        assert!(handle_jsonrpc(
            &core,
            &read_only,
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .await
        .is_none());

        // Ping
        let ping = handle_jsonrpc(
            &core,
            &read_only,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }),
        )
        .await
        .unwrap();
        assert_eq!(ping["result"], json!({}));

        // tools/list only advertises read tools when write tools are off
        let list = handle_jsonrpc(
            &core,
            &read_only,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
        )
        .await
        .unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(names, READ_TOOL_NAMES);

        // Calling a write tool while disabled returns isError=true
        let blocked = handle_jsonrpc(
            &core,
            &read_only,
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "spawn_agent",
                    "arguments": {
                        "provider": "claude",
                        "project_path": dir.to_string_lossy(),
                        "prompt": "hi"
                    }
                }
            }),
        )
        .await
        .unwrap();
        assert_eq!(blocked["result"]["isError"], true);
        assert!(blocked["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("disabled"));

        // Calling list_agents succeeds
        let agents_res = handle_jsonrpc(
            &core,
            &read_only,
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": { "name": "list_agents", "arguments": {} }
            }),
        )
        .await
        .unwrap();
        assert_eq!(agents_res["result"]["isError"], false);
        let parsed: Value =
            serde_json::from_str(agents_res["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(parsed["count"], 0);

        // With write tools enabled, all 6 tools are advertised
        let read_write = LocalMcpSettings {
            allow_write_tools: true,
            ..read_only
        };
        let list_all = handle_jsonrpc(
            &core,
            &read_write,
            json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/list" }),
        )
        .await
        .unwrap();
        assert_eq!(list_all["result"]["tools"].as_array().unwrap().len(), 6);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
