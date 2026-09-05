//! The harness payload, read leniently.
//!
//! The documented common fields are `session_id`, `transcript_path`, `cwd`,
//! `hook_event_name` and, per event, `source` (session-start), `user_input` / `prompt`
//! (user-prompt-submit), `tool_name` / `tool_input` / `tool_use_id` (tool use), `trigger`
//! (pre-compact) and `stop_hook_active` (stop). Nothing here is trusted: `session_id` is
//! sanitised before it names a file, `cwd` is only ever joined with a path it is asked
//! about, and unknown fields are ignored rather than refused. A payload that does not parse
//! is *not* an error: the hook proceeds with an empty payload and says what was wrong.

use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct Payload {
    pub session_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub hook_event_name: Option<String>,
    /// `session-start`: `startup` | `resume` | `clear` | `compact` | `fork`.
    pub source: Option<String>,
    /// `pre-compact`: `manual` | `auto`.
    pub trigger: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Value,
    pub stop_hook_active: bool,
    /// Everything odd about the payload, for stderr. Empty for a well-formed one.
    pub notes: Vec<String>,
}

impl Payload {
    pub fn parse(stdin: &str) -> Payload {
        let mut p = Payload::default();
        let text = stdin.trim();
        if text.is_empty() {
            p.notes
                .push("stdin was empty; proceeding as if the payload were {}".into());
            return p;
        }
        let v: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                p.notes.push(format!(
                    "stdin is not JSON ({e}); proceeding as if the payload were {{}}"
                ));
                return p;
            }
        };
        let Some(obj) = v.as_object() else {
            p.notes.push(format!(
                "payload is a JSON {} rather than an object; proceeding as if the payload were {{}}",
                json_kind(&v)
            ));
            return p;
        };
        let string = |k: &str| obj.get(k).and_then(Value::as_str).map(str::to_owned);
        p.session_id = string("session_id").filter(|s| !s.trim().is_empty());
        p.cwd = string("cwd")
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);
        p.hook_event_name = string("hook_event_name");
        p.source = string("source");
        p.trigger = string("trigger");
        p.tool_name = string("tool_name");
        p.tool_input = obj.get("tool_input").cloned().unwrap_or(Value::Null);
        p.stop_hook_active = obj
            .get("stop_hook_active")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        p
    }

    /// `file_path` (Edit, Write, MultiEdit) or `notebook_path` (NotebookEdit) of a
    /// file-editing tool; `None` for any other tool or a malformed input.
    pub fn edited_file(&self) -> Option<PathBuf> {
        let tool = self.tool_name.as_deref()?;
        let key = match tool {
            "Edit" | "Write" | "MultiEdit" => "file_path",
            "NotebookEdit" => "notebook_path",
            _ => return None,
        };
        self.tool_input
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
    }
}

fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
