//! MCP (Model Context Protocol) server for gif-that-for-you.
//!
//! Exposes two tools over stdin/stdout newline-delimited JSON-RPC:
//!   - start_recording  – opens the XDG portal picker and starts capturing
//!   - stop_recording   – finalises the capture and returns the GIF path

use std::io::{BufRead, Write};
use std::sync::mpsc;

use gif_that_for_you::recorder::Recorder;
use glib::MainContext;
use serde_json::{json, Value};

fn main() {
    // Dedicated GLib context/loop for the recording thread.  The portal and
    // GStreamer flows rely on D-Bus signal callbacks that require a running
    // GLib main loop.
    let ctx = glib::MainContext::new();
    let ml = glib::MainLoop::new(Some(&ctx), false);

    let recorder = Recorder::new();

    // GLib thread: run the event loop so portal callbacks can fire.
    let ctx_glib = ctx.clone();
    let ml_glib = ml.clone();
    std::thread::spawn(move || {
        ctx_glib
            .with_thread_default(|| ml_glib.run())
            .expect("failed to set GLib thread-default context");
    });

    // Main thread: newline-delimited JSON-RPC over stdin / stdout.
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(response) = handle_message(&request, &recorder, &ctx) {
            let mut out = stdout.lock();
            writeln!(out, "{}", response).unwrap();
            out.flush().unwrap();
        }
    }

    ml.quit();
}

// ---------------------------------------------------------------------------
// JSON-RPC dispatch
// ---------------------------------------------------------------------------

fn handle_message(
    msg: &Value,
    recorder: &Recorder,
    ctx: &MainContext,
) -> Option<Value> {
    let method = msg.get("method")?.as_str()?;

    // Notifications have no "id" — send no response.
    let id = msg.get("id").cloned()?;

    match method {
        "initialize" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "gif-that-for-you", "version": "0.1.0" }
            }
        })),

        "ping" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {} })),

        "tools/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "tools": tools_schema() }
        })),

        "tools/call" => {
            let params = msg.get("params")?;
            let name = params.get("name")?.as_str()?;
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Some(dispatch_tool(name, &args, recorder, ctx, &id))
        }

        _ => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "Method not found" }
        })),
    }
}

fn dispatch_tool(
    name: &str,
    args: &Value,
    recorder: &Recorder,
    ctx: &MainContext,
    id: &Value,
) -> Value {
    match name {
        "start_recording" => {
            // Create a one-shot reply channel.  ctx.invoke schedules the
            // portal call on the GLib thread; we block here until it reports
            // success or failure via the sync channel.
            let (reply_tx, reply_rx) = mpsc::sync_channel::<Result<(), String>>(1);
            let rec = recorder.clone();
            ctx.invoke(move || {
                // source_types=3: let portal show both monitors and windows for AI use.
                rec.start_portal(3, None, move |result| {
                    let _ = reply_tx.send(result);
                });
            });
            match reply_rx.recv() {
                Ok(Ok(())) => tool_result(
                    id,
                    "Recording started. A system dialog was shown for the user to \
                     select which screen or window to share. \
                     Call stop_recording when done.",
                    false,
                ),
                Ok(Err(e)) => tool_result(id, &e, true),
                Err(_) => tool_result(id, "Internal error: result channel closed", true),
            }
        }

        "stop_recording" => {
            let fps = args.get("fps")
                .and_then(|v| v.as_u64())
                .map(|v| v.clamp(1, 30) as u32)
                .unwrap_or(15);
            match recorder.stop(fps) {
                Ok(path) => tool_result(
                    id,
                    &format!("GIF saved to {}", path.display()),
                    false,
                ),
                Err(e) => tool_result(id, &e, true),
            }
        }

        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("Unknown tool: {name}") }
        }),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tool_result(id: &Value, text: &str, is_error: bool) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": text }],
            "isError": is_error
        }
    })
}

fn tools_schema() -> Value {
    json!([
        {
            "name": "start_recording",
            "description": "Start recording the screen as a GIF. A system dialog \
                            will appear for the user to select which screen or window \
                            to capture. Blocks until the user confirms and recording begins.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        },
        {
            "name": "stop_recording",
            "description": "Stop the current screen recording, convert it to an \
                            animated GIF, and return the path to the saved file. \
                            Blocks until conversion is complete.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "fps": {
                        "type": "integer",
                        "description": "Output GIF frame rate (1–30). Defaults to 15.",
                        "minimum": 1,
                        "maximum": 30
                    }
                },
                "required": []
            }
        }
    ])
}
