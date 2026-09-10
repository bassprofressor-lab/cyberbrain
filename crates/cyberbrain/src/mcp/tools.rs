//! The five tools (SPEC §9.2) and the shape of what they return.
//!
//! Every tool result is `{content: [text], structuredContent, isError}`. `content` carries
//! the same human rendering the CLI prints, `structuredContent` the same JSON `--json`
//! prints, so the three front ends cannot disagree about the numbers. A recall's caveats
//! are in both: as `caveat:` lines in the text and verbatim in `structuredContent.caveats`.
//!
//! What is a tool result and what is a JSON-RPC error follows one rule: if the operation
//! ran, the answer is a result, `isError` says whether it did what was asked. A policy
//! refusal, a held write, a note that does not exist — all results, because the calling
//! agent has to read them and act. JSON-RPC errors are reserved for requests the server
//! could not even begin: an unknown tool, an argument that does not fit the schema.

use crate::app::{App, RecallRequest, WriteOutcome, WriteRequest};
use crate::mcp::describe;
use crate::mcp::jsonrpc::RpcError;
use crate::render;
use cyberbrain_core::{Error, NoteKind, Ring};
use cyberbrain_policy::OperatorChoice;
use serde::Serialize;
use serde_json::{Map, Value, json};

pub const TOOL_NAMES: [&str; 5] = ["recall", "recall_id", "find", "write", "status"];

/// One entry of `tools/list`.
pub struct Tool {
    pub name: &'static str,
    pub title: &'static str,
    /// From the CLI help (`describe`), plus at most one MCP-only sentence about the
    /// result envelope, separated by a blank line so a test can check the first part.
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
}

fn schema_prop(cmd: &str, arg: &str, mut base: Value) -> Value {
    if let Some(h) = describe::arg_help(cmd, arg) {
        base["description"] = Value::String(h);
    }
    base
}

fn note_kinds() -> Vec<Value> {
    [
        NoteKind::Knowledge,
        NoteKind::Bug,
        NoteKind::Lesson,
        NoteKind::Decision,
        NoteKind::Reference,
        NoteKind::Session,
    ]
    .iter()
    .map(|k| serde_json::to_value(k).unwrap_or(Value::Null))
    .collect()
}

fn choices() -> Vec<Value> {
    [
        OperatorChoice::Redact,
        OperatorChoice::MarkReviewed,
        OperatorChoice::ProceedFlagged,
    ]
    .iter()
    .map(|c| Value::String(c.as_str().into()))
    .collect()
}

fn ring_schema(cmd: &str) -> Value {
    schema_prop(
        cmd,
        "ring",
        json!({
            "type": "integer",
            "minimum": Ring::ALL[0].as_u8(),
            "maximum": Ring::ALL[Ring::ALL.len() - 1].as_u8(),
        }),
    )
}

fn description(cli_about: String, envelope: &str) -> String {
    format!("{cli_about}\n\n{envelope}")
}

pub fn catalogue() -> Vec<Tool> {
    vec![
        Tool {
            name: "recall",
            title: "Recall",
            description: description(
                describe::about("recall"),
                "Result: hits with citation, ring, note name and block text; conflicts; and \
                 `caveats`, which say what was not checked (for example that the \
                 contradiction check was skipped). Read the caveats before trusting the hits.",
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": schema_prop("recall", "query", json!({ "type": "string" })),
                    "n": schema_prop("recall", "n", json!({ "type": "integer", "minimum": 1 })),
                    "ring": ring_schema("recall"),
                },
                "required": ["query"],
                "additionalProperties": false,
            }),
            read_only: true,
        },
        Tool {
            name: "recall_id",
            title: "Expand a citation",
            description: description(
                describe::arg_help("recall", "id").unwrap_or_else(|| {
                    "(no description: `cyberbrain recall --id` has no help text in cli.rs)".into()
                }),
                "Equivalent to `cyberbrain recall --id <citation>`. Result: the cited block and \
                 the whole note it belongs to.",
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "citation": schema_prop(
                        "recall",
                        "id",
                        json!({ "type": "string", "pattern": "^r[0-4]-[0-9a-f]{12}$" }),
                    ),
                },
                "required": ["citation"],
                "additionalProperties": false,
            }),
            read_only: true,
        },
        Tool {
            name: "find",
            title: "Find a symbol",
            description: description(
                describe::about("find"),
                "Result: file paths and line ranges; read the slice, not the file.",
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": schema_prop("find", "symbol", json!({ "type": "string" })),
                    "limit": schema_prop("find", "limit", json!({ "type": "integer", "minimum": 1 })),
                },
                "required": ["symbol"],
                "additionalProperties": false,
            }),
            read_only: true,
        },
        Tool {
            name: "write",
            title: "Write a note",
            description: description(
                describe::about("write"),
                "Result: `outcome` is `written`, `held` or `conflict`. A held write is a \
                 decision, not a failure: the PII findings are returned and nothing was \
                 written; answer with `choice` (redact, mark-reviewed, proceed-flagged), \
                 `force` (same as proceed-flagged), or a changed body.",
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "ring": ring_schema("write"),
                    "kind": schema_prop("write", "kind", json!({ "type": "string", "enum": note_kinds() })),
                    "name": schema_prop("write", "name", json!({ "type": "string" })),
                    "body": schema_prop("write", "body", json!({ "type": "string" })),
                    "tags": schema_prop("write", "tags", json!({ "type": "array", "items": { "type": "string" } })),
                    "bereich": schema_prop("write", "bereich", json!({ "type": "string" })),
                    "retention": schema_prop("write", "retention", json!({ "type": "string" })),
                    "force": schema_prop("write", "force", json!({ "type": "boolean" })),
                    "dry_run": schema_prop("write", "dry_run", json!({ "type": "boolean" })),
                    "choice": {
                        "type": "string",
                        "enum": choices(),
                        "description": "Answer to a previous hold on this same body: redact the findings, write with them marked reviewed, or write with them flagged (SPEC §12.4).",
                    },
                    "expected_updated": {
                        "type": "string",
                        "description": "RFC 3339 timestamp the note had when it was read; the write is refused with outcome `conflict` if it changed since (SPEC §8.1).",
                    },
                },
                "required": ["ring", "kind", "name", "body"],
                "additionalProperties": false,
            }),
            read_only: false,
        },
        Tool {
            name: "status",
            title: "Status",
            description: description(
                describe::about("status"),
                "Result: the same report as `cyberbrain status --json`.",
            ),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            read_only: true,
        },
    ]
}

/// The `tools/list` result.
pub fn list_value() -> Value {
    let tools: Vec<Value> = catalogue()
        .into_iter()
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": t.input_schema,
                "annotations": {
                    "title": t.title,
                    "readOnlyHint": t.read_only,
                    "destructiveHint": false,
                    "idempotentHint": t.read_only,
                    "openWorldHint": false,
                },
            })
        })
        .collect();
    json!({ "tools": tools })
}

// ---------------------------------------------------------------------------------------
// Argument access. Every failure names the tool, the argument and what was expected.

struct Args<'a> {
    tool: &'a str,
    map: Map<String, Value>,
}

impl<'a> Args<'a> {
    fn new(tool: &'a str, raw: &Value, allowed: &[&str]) -> Result<Self, RpcError> {
        let map = match raw {
            Value::Null => Map::new(),
            Value::Object(m) => m.clone(),
            other => {
                return Err(RpcError::invalid_params(format!(
                    "tool {tool}: arguments must be an object, got {}",
                    kind_of(other)
                )));
            }
        };
        // An argument the tool does not know is refused, not ignored: an agent that sends
        // `ring: 0` misspelt as `rng` must hear about it rather than write to the default.
        if let Some(bad) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(RpcError::invalid_params(format!(
                "tool {tool}: unknown argument `{bad}`; accepted: {}",
                allowed.join(", ")
            )));
        }
        Ok(Args { tool, map })
    }

    fn expect<T>(
        &self,
        key: &str,
        what: &str,
        got: Option<Result<T, String>>,
    ) -> Result<Option<T>, RpcError> {
        match got {
            None => Ok(None),
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(why)) => Err(RpcError::invalid_params(format!(
                "tool {}: argument `{key}` must be {what}: {why}",
                self.tool
            ))),
        }
    }

    fn required<T>(&self, key: &str, v: Option<T>) -> Result<T, RpcError> {
        v.ok_or_else(|| {
            RpcError::invalid_params(format!("tool {}: argument `{key}` is required", self.tool))
        })
    }

    fn string(&self, key: &str) -> Result<Option<String>, RpcError> {
        self.expect(
            key,
            "a string",
            self.map.get(key).map(|v| match v {
                Value::String(s) => Ok(s.clone()),
                other => Err(format!("got {}", kind_of(other))),
            }),
        )
    }

    fn string_required(&self, key: &str) -> Result<String, RpcError> {
        let v = self.string(key)?;
        self.required(key, v)
    }

    fn uint(&self, key: &str, min: u64) -> Result<Option<u64>, RpcError> {
        self.expect(
            key,
            &format!("an integer >= {min}"),
            self.map.get(key).map(|v| match v.as_u64() {
                Some(n) if n >= min => Ok(n),
                Some(n) => Err(format!("got {n}")),
                None => Err(format!("got {}", kind_of(v))),
            }),
        )
    }

    fn boolean(&self, key: &str) -> Result<Option<bool>, RpcError> {
        self.expect(
            key,
            "a boolean",
            self.map
                .get(key)
                .map(|v| v.as_bool().ok_or_else(|| format!("got {}", kind_of(v)))),
        )
    }

    fn string_list(&self, key: &str) -> Result<Option<Vec<String>>, RpcError> {
        self.expect(
            key,
            "an array of strings",
            self.map.get(key).map(|v| match v {
                Value::Array(items) => items
                    .iter()
                    .map(|i| {
                        i.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| format!("element is {}", kind_of(i)))
                    })
                    .collect(),
                other => Err(format!("got {}", kind_of(other))),
            }),
        )
    }

    fn ring(&self, key: &str) -> Result<Option<Ring>, RpcError> {
        self.expect(
            key,
            "a ring number 0..=4",
            self.map.get(key).map(|v| {
                let n = v
                    .as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(|| format!("got {}", kind_of(v)))?;
                Ring::try_from(n).map_err(|e| e.to_string())
            }),
        )
    }
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

// ---------------------------------------------------------------------------------------
// Result envelope.

fn envelope(text: String, structured: Value, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": is_error,
    })
}

fn structured<T: Serialize>(v: &T) -> Result<Value, RpcError> {
    serde_json::to_value(v)
        .map_err(|e| RpcError::internal(format!("report does not serialise: {e}")))
}

/// An error from `App`, as a tool result. The structured shape is §8.1's
/// `{error: {code, message, exit_code}}`: one failure taxonomy, three front ends.
fn app_error(e: &Error) -> Value {
    let exit = e.exit_code();
    let prefix = match exit {
        3 => "refused by policy",
        1 => "error",
        _ => "internal error",
    };
    envelope(
        format!("{prefix}: {e}\n"),
        json!({
            "error": {
                "code": crate::error_code(e),
                "message": e.to_string(),
                "exit_code": exit,
            }
        }),
        true,
    )
}

/// Run one tool. `Err` is a protocol failure (unknown tool, bad arguments); everything
/// the tool itself has to say comes back as `Ok(result)`.
pub async fn call(app: &App, name: &str, raw_args: &Value) -> Result<Value, RpcError> {
    match name {
        "recall" => recall(app, raw_args).await,
        "recall_id" => recall_id(app, raw_args),
        "find" => find(app, raw_args),
        "write" => write(app, raw_args),
        "status" => status(app, raw_args).await,
        other => Err(RpcError::invalid_params(format!(
            "unknown tool `{other}`; this server offers: {}",
            TOOL_NAMES.join(", ")
        ))),
    }
}

async fn recall(app: &App, raw: &Value) -> Result<Value, RpcError> {
    let a = Args::new("recall", raw, &["query", "n", "ring", "bereich"])?;
    let query = a.string_required("query")?;
    let req = RecallRequest {
        n: a.uint("n", 1)?
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
        ring: a.ring("ring")?,
        bereich: a.string("bereich")?,
    };
    Ok(match app.recall(&query, &req).await {
        // Caveats travel twice on purpose: verbatim in `structuredContent.caveats`, and as
        // `caveat:` lines in the text for a client that only reads text.
        Ok(r) => envelope(render::recall(&r), structured(&r)?, false),
        Err(e) => app_error(&e),
    })
}

fn recall_id(app: &App, raw: &Value) -> Result<Value, RpcError> {
    let a = Args::new("recall_id", raw, &["citation"])?;
    let citation = a.string_required("citation")?;
    Ok(match app.recall_id(&citation) {
        Ok(e) => envelope(render::expanded(&e), structured(&e)?, false),
        Err(e) => app_error(&e),
    })
}

fn find(app: &App, raw: &Value) -> Result<Value, RpcError> {
    let a = Args::new("find", raw, &["symbol", "limit"])?;
    let symbol = a.string_required("symbol")?;
    let limit = a
        .uint("limit", 1)?
        .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
        .unwrap_or(20);
    Ok(match find_report(app, &symbol, limit) {
        Ok(v) => {
            let text = serde_json::to_string_pretty(&v).unwrap_or_default();
            envelope(text, v, false)
        }
        Err(e) => app_error(&e),
    })
}

/// The one place `find` touches `App`, so the seam is a single line.
///
/// It was a refusal while `App::find` did not exist yet. It does now, and the refusal
/// outlived it: `cyberbrain find` worked from the CLI while the same operation over MCP
/// still answered "not available in this build". A stub that survives the thing it stood
/// in for is worse than no stub, because it reports a missing feature that is present.
fn find_report(app: &App, symbol: &str, limit: usize) -> cyberbrain_core::Result<Value> {
    let report = app.find(symbol, limit)?;
    serde_json::to_value(&report)
        .map_err(|e| Error::Index(format!("find report does not serialise: {e}")))
}
fn write(app: &App, raw: &Value) -> Result<Value, RpcError> {
    let a = Args::new(
        "write",
        raw,
        &[
            "ring",
            "kind",
            "name",
            "body",
            "tags",
            "bereich",
            "retention",
            "force",
            "dry_run",
            "choice",
            "expected_updated",
        ],
    )?;
    let ring = a.ring("ring")?;
    let ring = a.required("ring", ring)?;
    let kind = a.string_required("kind")?;
    let kind: NoteKind = serde_json::from_value(Value::String(kind.clone())).map_err(|_| {
        RpcError::invalid_params(format!(
            "tool write: argument `kind` must be one of {}; got `{kind}`",
            note_kinds()
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    let choice = match a.string("choice")? {
        None => None,
        Some(c) => Some(
            serde_json::from_value::<OperatorChoice>(Value::String(c.clone())).map_err(|_| {
                RpcError::invalid_params(format!(
                    "tool write: argument `choice` must be one of redact, mark-reviewed, \
                     proceed-flagged; got `{c}`"
                ))
            })?,
        ),
    };
    let expected_updated = match a.string("expected_updated")? {
        None => None,
        Some(s) => Some(s.parse::<jiff::Timestamp>().map_err(|e| {
            RpcError::invalid_params(format!(
                "tool write: argument `expected_updated` must be an RFC 3339 timestamp: {e}"
            ))
        })?),
    };
    let req = WriteRequest {
        ring,
        kind,
        name: a.string_required("name")?,
        body: a.string_required("body")?,
        tags: a.string_list("tags")?.unwrap_or_default(),
        bereich: a.string("bereich")?,
        retention: a.string("retention")?,
        force: a.boolean("force")?.unwrap_or(false),
        choice,
        expected_updated,
        dry_run: a.boolean("dry_run")?.unwrap_or(false),
    };
    Ok(match app.write(req) {
        Ok(outcome) => {
            let structured = structured(&outcome)?;
            match &outcome {
                WriteOutcome::Written(w) => envelope(render::written(w), structured, false),
                // SPEC §12.4: the hold is a decision point. The findings go back with the
                // four options spelled out; `isError` is set because nothing was written.
                WriteOutcome::Held { rendered, .. } => envelope(
                    format!(
                        "{rendered}Nothing was written. Call write again with the same body \
                         and `choice`: \"redact\" (replace the findings with placeholders), \
                         \"mark-reviewed\" (write as is, findings accepted) or \
                         \"proceed-flagged\" (write as is, findings stand); or `force: true` \
                         (same as proceed-flagged); or change the body.\n"
                    ),
                    structured,
                    true,
                ),
                WriteOutcome::Conflict {
                    name,
                    current_updated,
                } => envelope(
                    format!(
                        "{name} changed at {current_updated} since it was read; nothing was \
                         written. Re-read it and send its current `updated` as \
                         `expected_updated`.\n"
                    ),
                    structured,
                    true,
                ),
            }
        }
        Err(e) => app_error(&e),
    })
}

async fn status(app: &App, raw: &Value) -> Result<Value, RpcError> {
    // No arguments, and that is checked: an argument the tool ignores is a silent early
    // return with a friendlier face.
    let _ = Args::new("status", raw, &[])?;
    Ok(match app.status().await {
        Ok(s) => envelope(render::status(&s), structured(&s)?, false),
        Err(e) => app_error(&e),
    })
}
