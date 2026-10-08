//! Small, blocking MCP stdio server. Stdout is reserved for JSON-RPC frames.

use std::io::{self, BufRead, Write};

use serde_json::{json, Map, Value};

use crate::google;
use crate::model::{occurrence_indices, occurrences, validate_occurrence_range, Date, Event};
use crate::preferences::Preferences;
use crate::store::Store;

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn run() -> Result<(), String> {
    let store = Store::open()?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = io::BufReader::new(stdin.lock());
    let mut output = io::BufWriter::new(stdout.lock());
    let mut initialized = false;

    while let Some(frame) = read_frame(&mut input)? {
        let response = match serde_json::from_slice::<Value>(&frame) {
            Ok(request) => handle_request(request, &store, &mut initialized),
            Err(_) => Some(rpc_error(Value::Null, -32700, "Parse error")),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response).map_err(|e| e.to_string())?;
            output.write_all(b"\n").map_err(|e| e.to_string())?;
            output.flush().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn read_frame<R: BufRead>(input: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut frame = Vec::new();
    loop {
        let available = input.fill_buf().map_err(|e| e.to_string())?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Ok(Some(frame))
            };
        }
        let n = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |p| p + 1);
        if frame.len().saturating_add(n) > MAX_FRAME_BYTES {
            return Err(format!("MCP input line exceeds {MAX_FRAME_BYTES} bytes"));
        }
        let has_newline = available[n - 1] == b'\n';
        frame.extend_from_slice(&available[..n]);
        input.consume(n);
        if has_newline {
            return Ok(Some(frame));
        }
    }
}

fn handle_request(request: Value, store: &Store, initialized: &mut bool) -> Option<Value> {
    let Some(object) = request.as_object() else {
        return Some(rpc_error(Value::Null, -32600, "Invalid Request"));
    };
    let id = match object.get("id") {
        None => None,
        Some(Value::Null) => Some(Value::Null),
        Some(Value::String(s)) => Some(json!(s)),
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => Some(json!(n)),
        _ => return Some(rpc_error(Value::Null, -32600, "Invalid Request")),
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object.get("method").and_then(Value::as_str).is_none()
    {
        return Some(rpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "Invalid Request",
        ));
    }
    // MCP notifications never receive a response.
    let id = id?;
    let method = object["method"].as_str().unwrap();
    let params = object.get("params").unwrap_or(&Value::Null);
    let result = match method {
        "initialize" => {
            let version = params.get("protocolVersion").and_then(Value::as_str);
            if !params.is_object() || version.is_none() {
                return Some(rpc_error(id, -32602, "Invalid initialize parameters"));
            }
            *initialized = true;
            let selected = match version.unwrap() {
                "2024-11-05" | "2025-03-26" | "2025-06-18" => version.unwrap(),
                _ => PROTOCOL_VERSION,
            };
            json!({
                "protocolVersion": selected,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "dot-calendar", "version": env!("CARGO_PKG_VERSION")}
            })
        }
        "ping" => json!({}),
        "tools/list" if *initialized => json!({"tools": tool_definitions()}),
        "tools/call" if *initialized => {
            let Some(params) = params.as_object() else {
                return Some(rpc_error(id, -32602, "Invalid tool call parameters"));
            };
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(id, -32602, "Tool name is required"));
            };
            let args = params.get("arguments").unwrap_or(&Value::Null);
            if !is_tool(name) {
                return Some(rpc_error(id, -32602, "Unknown tool"));
            }
            tool_result(call_tool(name, args, store))
        }
        "tools/list" | "tools/call" => {
            return Some(rpc_error(id, -32002, "Server is not initialized"));
        }
        _ => return Some(rpc_error(id, -32601, "Method not found")),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool_result(result: Result<Value, String>) -> Value {
    let (text, is_error) = match result {
        Ok(value) => (
            serde_json::to_string(&value).expect("JSON Value serialization failed"),
            false,
        ),
        Err(message) => (message, true),
    };
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn is_tool(name: &str) -> bool {
    matches!(
        name,
        "calendar_today"
            | "calendar_list"
            | "calendar_occurrences"
            | "calendar_create"
            | "calendar_update"
            | "calendar_set_details"
            | "calendar_delete"
            | "calendar_deleted"
            | "calendar_restore"
    )
}

fn call_tool(name: &str, args: &Value, store: &Store) -> Result<Value, String> {
    let result = execute_tool(name, args, store)?;
    if matches!(
        name,
        "calendar_create"
            | "calendar_update"
            | "calendar_set_details"
            | "calendar_delete"
            | "calendar_restore"
    ) {
        let id = result["id"].as_str().ok_or("write returned no event id")?;
        let saved = store
            .list_events(None)?
            .into_iter()
            .find(|event| event.id == id);
        let verified = if name == "calendar_delete" {
            saved.is_none()
        } else {
            saved
                .map(serde_json::to_value)
                .transpose()
                .map_err(|e| e.to_string())?
                .as_ref()
                == Some(&result)
        };
        if !verified {
            return Err("write completed but read-back differs; re-read before retrying".into());
        }
    }
    Ok(result)
}

fn execute_tool(name: &str, args: &Value, store: &Store) -> Result<Value, String> {
    let empty = Map::new();
    let args = if args.is_null()
        && matches!(
            name,
            "calendar_list" | "calendar_today" | "calendar_deleted" | "calendar_restore"
        ) {
        &empty
    } else {
        args.as_object().ok_or("arguments must be an object")?
    };
    match name {
        "calendar_today" => {
            validate_keys(args, &[], &["include_completed"])?;
            let include_completed = optional_bool(args, "include_completed")?.unwrap_or(false);
            let date = Date::today();
            let events = store.list_events(None)?;
            let show_google = Preferences::load().show_google;
            let today_events: Vec<Value> = occurrence_indices(&events, date, date)
                .into_iter()
                .map(|(_, index)| &events[index])
                .filter(|event| {
                    (include_completed || !event.completed)
                        && (show_google || !google::is_imported_event(event))
                })
                .map(|event| {
                    json!({
                        "source": if google::is_imported_event(event) { "google" } else { "local" },
                        "event": event
                    })
                })
                .collect();
            Ok(json!({"date": date.to_string(), "events": today_events}))
        }
        "calendar_list" => {
            validate_keys(args, &[], &["date"])?;
            let date = optional_string(args, "date", 10)?;
            serde_json::to_value(store.list_events(date)?).map_err(|e| e.to_string())
        }
        "calendar_occurrences" => {
            validate_keys(args, &["from", "to"], &[])?;
            let from = Date::parse(required_string(args, "from", 10)?)?;
            let to = Date::parse(required_string(args, "to", 10)?)?;
            validate_occurrence_range(from, to)?;
            serde_json::to_value(occurrences(&store.list_events(None)?, from, to))
                .map_err(|e| e.to_string())
        }
        "calendar_create" => {
            validate_keys(
                args,
                &["date", "title", "request_id"],
                &[
                    "time",
                    "notes",
                    "completed",
                    "color",
                    "recurrence",
                    "reminder_minutes",
                ],
            )?;
            let date = required_string(args, "date", 10)?;
            let title = required_string(args, "title", 200)?;
            let request_id = required_string(args, "request_id", 128)?;
            let time = optional_string(args, "time", 5)?;
            let notes = optional_notes(args)?;
            serde_json::to_value(store.create_event_with_details(
                date,
                time,
                title,
                notes,
                Some(request_id),
                optional_bool(args, "completed")?.unwrap_or(false),
                optional_nullable_string(args, "color", 7)?.flatten(),
                optional_nullable_string(args, "recurrence", 7)?.flatten(),
                optional_nullable_u32(args, "reminder_minutes")?.flatten(),
            )?)
            .map_err(|e| e.to_string())
        }
        "calendar_update" => {
            validate_keys(
                args,
                &["id", "date", "title"],
                &[
                    "time",
                    "notes",
                    "completed",
                    "color",
                    "recurrence",
                    "reminder_minutes",
                    "expected",
                ],
            )?;
            let id = required_string(args, "id", 128)?;
            let expected = optional_expected(args, id)?;
            let date = required_string(args, "date", 10)?;
            let title = required_string(args, "title", 200)?;
            let time = optional_string(args, "time", 5)?;
            let notes = optional_notes(args)?;
            serde_json::to_value(store.update_event_with_details_expected(
                id,
                date,
                time,
                title,
                notes,
                optional_bool(args, "completed")?,
                optional_nullable_string(args, "color", 7)?,
                optional_nullable_string(args, "recurrence", 7)?,
                optional_nullable_u32(args, "reminder_minutes")?,
                expected.as_ref(),
            )?)
            .map_err(|e| e.to_string())
        }
        "calendar_set_details" => {
            validate_keys(
                args,
                &["id"],
                &[
                    "completed",
                    "color",
                    "recurrence",
                    "reminder_minutes",
                    "expected",
                ],
            )?;
            let id = required_string(args, "id", 128)?;
            let expected = optional_expected(args, id)?;
            serde_json::to_value(store.patch_details_expected(
                id,
                optional_bool(args, "completed")?,
                optional_nullable_string(args, "color", 7)?,
                optional_nullable_string(args, "recurrence", 7)?,
                optional_nullable_u32(args, "reminder_minutes")?,
                expected.as_ref(),
            )?)
            .map_err(|e| e.to_string())
        }
        "calendar_delete" => {
            let deleted = if args.contains_key("id") {
                validate_keys(args, &["id"], &["expected"])?;
                let id = required_string(args, "id", 128)?;
                let expected = optional_expected(args, id)?;
                store.delete_event_expected(id, expected.as_ref())?
            } else {
                validate_keys(args, &["date", "title"], &["time", "series"])?;
                let date = Date::parse(required_string(args, "date", 10)?)?;
                let title = required_string(args, "title", 200)?;
                let time = optional_string(args, "time", 5)?;
                let series = optional_bool(args, "series")?.unwrap_or(false);
                store.delete_event_selected(|events| {
                    matching_delete_event(events, date, title, time, series)
                        .map(|event| event.id.clone())
                })?
            };
            serde_json::to_value(deleted).map_err(|e| e.to_string())
        }
        "calendar_deleted" => {
            validate_keys(args, &[], &[])?;
            serde_json::to_value(store.list_deleted_events()?).map_err(|e| e.to_string())
        }
        "calendar_restore" => {
            validate_keys(args, &[], &["id"])?;
            let restored = match optional_string(args, "id", 128)? {
                Some(id) => store.restore_deleted_event(id)?,
                None => store.restore_last_deleted()?,
            };
            serde_json::to_value(restored).map_err(|e| e.to_string())
        }
        _ => Err("Unknown tool".to_owned()),
    }
}

fn matching_delete_event<'a>(
    events: &'a [Event],
    date: Date,
    title: &str,
    time: Option<&str>,
    series: bool,
) -> Result<&'a Event, String> {
    let mut matches = occurrence_indices(events, date, date)
        .into_iter()
        .map(|(_, index)| &events[index])
        .filter(|event| {
            event.title == title && time.is_none_or(|time| event.time.as_deref() == Some(time))
        });
    let event = matches
        .next()
        .ok_or("no event matches that exact date and title; nothing deleted")?;
    if matches.next().is_some() {
        return Err(
            "multiple events match; specify the time or read their IDs first; nothing deleted"
                .into(),
        );
    }
    if event.recurrence.is_some() && !series {
        return Err("this is a recurring series; delete affects all occurrences and requires series: true; nothing deleted".into());
    }
    Ok(event)
}

fn validate_keys(
    args: &Map<String, Value>,
    required: &[&str],
    optional: &[&str],
) -> Result<(), String> {
    for name in required {
        if !args.contains_key(*name) {
            return Err(format!("Missing required field: {name}"));
        }
    }
    for name in args.keys() {
        if !required.contains(&name.as_str()) && !optional.contains(&name.as_str()) {
            return Err(format!("Unexpected field: {name}"));
        }
    }
    Ok(())
}

fn required_string<'a>(
    args: &'a Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<&'a str, String> {
    let value = args
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} must be a string"))?;
    if value.is_empty() || value.chars().count() > max || value.chars().any(|c| c.is_control()) {
        return Err(format!(
            "{key} must contain 1 to {max} characters without control characters"
        ));
    }
    Ok(value)
}

fn optional_string<'a>(
    args: &'a Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::String(value))
            if value.chars().count() <= max && !value.chars().any(|c| c.is_control()) =>
        {
            Ok(Some(value))
        }
        _ => Err(format!(
            "{key} must be a string of at most {max} characters"
        )),
    }
}

fn optional_notes(args: &Map<String, Value>) -> Result<&str, String> {
    match args.get("notes") {
        None => Ok(""),
        Some(Value::String(value)) if value.chars().count() <= 4096 => Ok(value),
        _ => Err("notes must be a string of at most 4096 characters".to_owned()),
    }
}

fn optional_expected(args: &Map<String, Value>, id: &str) -> Result<Option<Event>, String> {
    let Some(value) = args.get("expected") else {
        return Ok(None);
    };
    let expected: Event = serde_json::from_value(value.clone())
        .map_err(|error| format!("expected must be a complete event object: {error}"))?;
    if serde_json::to_value(&expected).map_err(|error| error.to_string())? != *value {
        return Err("expected must include every event field and no unknown fields".into());
    }
    if expected.id != id {
        return Err("expected.id must match id".into());
    }
    Ok(Some(expected))
}

fn optional_bool(args: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(format!("{key} must be a boolean")),
    }
}

fn optional_nullable_string<'a>(
    args: &'a Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<Option<&'a str>>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(value)) if !value.is_empty() && value.chars().count() <= max => {
            Ok(Some(Some(value)))
        }
        _ => Err(format!(
            "{key} must be null or a string of 1 to {max} characters"
        )),
    }
}

fn optional_nullable_u32(
    args: &Map<String, Value>,
    key: &str,
) -> Result<Option<Option<u32>>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::Number(number)) => number
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(Some)
            .map(Some)
            .ok_or_else(|| format!("{key} must be a nonnegative integer within u32 range")),
        _ => Err(format!("{key} must be null or a nonnegative integer")),
    }
}

fn event_snapshot_schema() -> Value {
    json!({
        "type": "object",
        "description": "Complete original event from calendar_list. When supplied, a concurrent change rejects this write.",
        "properties": {
            "id": {"type": "string", "minLength": 1, "maxLength": 128},
            "date": {"type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$"},
            "time": {"type": ["string", "null"], "pattern": "^([01][0-9]|2[0-3]):[0-5][0-9]$"},
            "title": {"type": "string", "minLength": 1, "maxLength": 200},
            "notes": {"type": "string", "maxLength": 4096},
            "request_id": {"type": ["string", "null"], "maxLength": 128},
            "completed": {"type": "boolean"},
            "color": {"type": ["string", "null"], "pattern": "^#[0-9A-Fa-f]{6}$"},
            "recurrence": {"type": ["string", "null"], "enum": ["daily", "weekly", "monthly", "yearly", null]},
            "reminder_minutes": {"type": ["integer", "null"], "minimum": 0, "maximum": 525600}
        },
        "required": ["id", "date", "time", "title", "notes", "request_id", "completed", "color", "recurrence", "reminder_minutes"],
        "additionalProperties": false
    })
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "calendar_today", "description": "Today's local agenda, including recurring events and Google imports when visible in the widget. Completed events are hidden unless requested. Each item has source and a complete original event snapshot suitable as expected for a guarded write; the top-level date is the occurrence date. Completion of a recurring event affects its whole series.",
            "inputSchema": {"type": "object", "properties": {"include_completed": {"type": "boolean", "description": "Include completed event series; default false"}}, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "calendar_list", "description": "List saved event series, optionally filtering their start date. Use calendar_occurrences for recurring dates.",
            "inputSchema": {"type": "object", "properties": {"date": {"type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "description": "YYYY-MM-DD"}}, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "calendar_occurrences", "description": "List event occurrences in an inclusive date range. Recurring occurrences retain the series ID; completion applies to the whole series.",
            "inputSchema": {"type": "object", "properties": {
                "from": {"type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$"},
                "to": {"type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$"}
            }, "required": ["from", "to"], "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "calendar_create", "description": "Create a local event. Reuse request_id when retrying the same operation.",
            "inputSchema": {"type": "object", "properties": {
                "date": {"type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "description": "YYYY-MM-DD"}, "time": {"type": "string", "pattern": "^([01][0-9]|2[0-3]):[0-5][0-9]$", "description": "HH:MM, optional"},
                "title": {"type": "string", "minLength": 1, "maxLength": 200}, "notes": {"type": "string", "maxLength": 4096},
                "request_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Unique stable identifier for safe retries"},
                "completed": {"type": "boolean"}, "color": {"type": ["string", "null"], "pattern": "^#[0-9A-Fa-f]{6}$"},
                "recurrence": {"type": ["string", "null"], "enum": ["daily", "weekly", "monthly", "yearly", null]},
                "reminder_minutes": {"type": ["integer", "null"], "minimum": 0, "maximum": 525600}
            }, "required": ["date", "title", "request_id"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "calendar_update", "description": "Replace the date, time, title and notes of a local event by id. Omitted detail fields stay unchanged; null clears an optional detail. Supply expected from calendar_list to reject concurrent edits.",
            "inputSchema": {"type": "object", "properties": {
                "id": {"type": "string", "minLength": 1, "maxLength": 128}, "date": {"type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "description": "YYYY-MM-DD"},
                "time": {"type": "string", "pattern": "^([01][0-9]|2[0-3]):[0-5][0-9]$", "description": "HH:MM, optional; omit to clear"},
                "title": {"type": "string", "minLength": 1, "maxLength": 200}, "notes": {"type": "string", "maxLength": 4096},
                "completed": {"type": "boolean"}, "color": {"type": ["string", "null"], "pattern": "^#[0-9A-Fa-f]{6}$"},
                "recurrence": {"type": ["string", "null"], "enum": ["daily", "weekly", "monthly", "yearly", null]},
                "reminder_minutes": {"type": ["integer", "null"], "minimum": 0, "maximum": 525600},
                "expected": event_snapshot_schema()
            }, "required": ["id", "date", "title"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "calendar_set_details", "description": "Change completion, color, recurrence or reminder for a saved event series. Omitted fields stay unchanged; null clears an optional field. Supply expected from calendar_list to reject concurrent edits.",
            "inputSchema": {"type": "object", "properties": {
                "id": {"type": "string", "minLength": 1, "maxLength": 128},
                "completed": {"type": "boolean"}, "color": {"type": ["string", "null"], "pattern": "^#[0-9A-Fa-f]{6}$"},
                "recurrence": {"type": ["string", "null"], "enum": ["daily", "weekly", "monthly", "yearly", null]},
                "reminder_minutes": {"type": ["integer", "null"], "minimum": 0, "maximum": 525600},
                "expected": event_snapshot_schema()
            }, "required": ["id"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "calendar_delete", "description": "Move an event to local recoverable trash, then verify its absence before returning success. Use id plus expected, or exact occurrence date and title in one call; ambiguous matches are rejected. A matching recurring series requires series: true. Undo with calendar_restore or the app's last-deletion undo menu. Google sync is separate.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string", "minLength": 1, "maxLength": 128}, "expected": event_snapshot_schema(), "date": {"type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$"}, "title": {"type": "string", "minLength": 1, "maxLength": 200}, "time": {"type": "string", "pattern": "^([01][0-9]|2[0-3]):[0-5][0-9]$"}, "series": {"type": "boolean"}}, "oneOf": [{"required": ["id"]}, {"required": ["date", "title"]}], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false}
        }),
        json!({
            "name": "calendar_deleted", "description": "List recoverable local deleted events, newest first. These are not active calendar entries.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "calendar_restore", "description": "Restore a deleted event by original id, or the most recent deletion if id is omitted. Preserves original fields, rejects collisions and verifies local read-back. This does not promise Google restoration.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string", "minLength": 1, "maxLength": 128}}, "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_delete_selector_rejects_ambiguity_and_guards_recurring_series() {
        let first: Event = serde_json::from_value(json!({
            "id": "one", "date": "2026-10-08", "time": "09:00", "title": "회의", "notes": ""
        }))
        .unwrap();
        let mut second = first.clone();
        second.id = "two".into();
        second.time = Some("10:00".into());
        let date = Date::parse("2026-10-08").unwrap();
        let events = vec![first.clone(), second];
        assert!(matching_delete_event(&events, date, "회의", None, false).is_err());
        assert_eq!(
            matching_delete_event(&events, date, "회의", Some("09:00"), false)
                .unwrap()
                .id,
            "one"
        );
        assert!(matching_delete_event(&events, date, "다른 회의", None, false).is_err());
        assert!(matching_delete_event(&events, date, "회의", Some("11:00"), false).is_err());
        let mut weekly = first;
        weekly.recurrence = Some("weekly".into());
        let recurring = vec![weekly];
        let next_week = Date::parse("2026-10-15").unwrap();
        assert!(matching_delete_event(&recurring, next_week, "회의", None, false).is_err());
        let found = matching_delete_event(&recurring, next_week, "회의", None, true).unwrap();
        assert_eq!(found.date, "2026-10-08");
    }

    #[test]
    fn rejects_wrong_types_and_extra_keys() {
        let args = json!({"date":"2026-10-07", "title": 3, "request_id":"r"});
        let args = args.as_object().unwrap();
        assert!(required_string(args, "title", 200).is_err());
        assert!(validate_keys(args, &["date", "title", "request_id"], &[]).is_ok());
        let args = json!({"id":"x", "unexpected":true});
        assert!(validate_keys(args.as_object().unwrap(), &["id"], &[]).is_err());
        let args = json!({"notes": "first line\nsecond line"});
        assert_eq!(
            optional_notes(args.as_object().unwrap()).unwrap(),
            "first line\nsecond line"
        );
    }

    #[test]
    fn frame_limit_is_enforced() {
        let input = vec![b'a'; MAX_FRAME_BYTES + 1];
        assert!(read_frame(&mut io::BufReader::new(input.as_slice())).is_err());
    }

    #[test]
    fn detail_arguments_distinguish_omitted_null_and_value() {
        let absent = Map::new();
        assert_eq!(optional_nullable_string(&absent, "color", 7).unwrap(), None);
        let args = json!({"color": null, "completed": true, "reminder_minutes": 30});
        let args = args.as_object().unwrap();
        assert_eq!(
            optional_nullable_string(args, "color", 7).unwrap(),
            Some(None)
        );
        assert_eq!(optional_bool(args, "completed").unwrap(), Some(true));
        assert_eq!(
            optional_nullable_u32(args, "reminder_minutes").unwrap(),
            Some(Some(30))
        );
        assert!(is_tool("calendar_set_details"));
        assert!(is_tool("calendar_occurrences"));
        assert!(is_tool("calendar_today"));
    }

    #[test]
    fn expected_snapshot_requires_complete_matching_event() {
        let event = json!({
            "id": "event-1", "date": "2026-10-09", "time": "15:00",
            "title": "Meeting", "notes": "", "request_id": "request-1",
            "completed": false, "color": null, "recurrence": null,
            "reminder_minutes": null
        });
        let args = json!({"expected": event});
        let expected = optional_expected(args.as_object().unwrap(), "event-1")
            .unwrap()
            .unwrap();
        assert_eq!(expected.title, "Meeting");
        assert!(optional_expected(args.as_object().unwrap(), "other-id").is_err());

        let mut incomplete = args.clone();
        incomplete["expected"]
            .as_object_mut()
            .unwrap()
            .remove("completed");
        assert!(optional_expected(incomplete.as_object().unwrap(), "event-1").is_err());
        let mut extra = args.clone();
        extra["expected"]["unknown"] = json!(true);
        assert!(optional_expected(extra.as_object().unwrap(), "event-1").is_err());
        let invalid = json!({"expected": null});
        assert!(optional_expected(invalid.as_object().unwrap(), "event-1").is_err());
        let definitions = tool_definitions();
        for name in ["calendar_update", "calendar_set_details", "calendar_delete"] {
            let tool = definitions
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap();
            assert_eq!(
                tool["inputSchema"]["properties"]["expected"]["type"],
                "object"
            );
        }
    }
}
