//! MCP server for Sunburst Disk Analyser.
//!
//! Speaks the Model Context Protocol over stdio (one JSON-RPC message per
//! line) and lets an AI assistant scan folders and ask where the space goes.
//! It reuses the desktop app's scanner and tree unchanged, and never modifies,
//! moves or deletes anything: every tool is read-only.

// The shared modules carry GUI helpers this binary does not call.
#![allow(dead_code)]

#[path = "../../src/filter.rs"]
mod filter;
#[cfg(unix)]
#[path = "../../src/pinned.rs"]
mod pinned;
#[path = "../../src/scan.rs"]
mod scan;
#[path = "../../src/tree.rs"]
mod tree;

mod format;
mod tools;

use serde_json::{json, Value};
use std::io::{BufRead, Write};

const USAGE: &str = "sunburst-mcp: MCP server (stdio) for Sunburst Disk Analyser, read-only

Usage: sunburst-mcp [--deny <path>]...

  --deny <path>   Refuse any scan that would cover <path> whole: <path> itself
                  or one of its parent folders. Its subfolders can still be
                  scanned. Repeat for several paths.
  -h, --help      Show this help
  -V, --version   Show the version

Register it with an MCP client, for example Claude Code:
  claude mcp add -s user local-disk-sunburst -- /path/to/sunburst-mcp --deny / --deny ~";

/// Protocol revisions this server speaks, newest first.
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Read-only disk usage analysis. Typical flow: `disks` to see which \
filesystem is short of space, `scan` on a folder (results stay in memory for the session), then \
`list` to walk down the biggest folders, `stale_folders` for big folders nobody touched in a long \
time, `duplicates` for probable copies, `biggest_files`, `by_type` and `search`. Sizes are space \
allocated on disk, in decimal units. Scans stay on the filesystem of the scanned folder and do not \
follow symbolic links. Nothing is ever deleted or moved by this server: suggest actions to the user.";

fn main() {
    let mut deny = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            "-V" | "--version" => {
                println!("{}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--deny" => match args.next() {
                Some(p) => deny.push(tools::absolute(&p)),
                None => fail("--deny needs a path"),
            },
            other => fail(&format!("Unknown option: {other}")),
        }
    }

    let mut server = tools::Server::new(deny);
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&mut server, msg),
            Err(e) => Some(error(Value::Null, -32700, &format!("Parse error: {e}"))),
        };
        if let Some(reply) = reply {
            if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}\n\n{USAGE}");
    std::process::exit(2);
}

/// Answers one message; notifications and responses get no reply.
fn handle(server: &mut tools::Server, msg: Value) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str)?;
    let id = msg.get("id").cloned()?;
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
            let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOL_VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "sunburst-disk-analyser", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools::definitions() }),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error(id, -32602, "tools/call needs a tool name"));
            };
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match server.call(name, &args) {
                Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                Err(tools::CallError::UnknownTool) => return Some(error(id, -32602, &format!("Unknown tool: {name}"))),
                Err(tools::CallError::Failed(text)) => json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
            }
        }
        _ => return Some(error(id, -32601, &format!("Method not found: {method}"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// musl has no `renameat2` wrapper, and the shared tests of pinned.rs call
/// it. This server itself never renames anything.
#[cfg(all(test, target_os = "linux", target_env = "musl"))]
#[no_mangle]
pub extern "C" fn renameat2(
    olddirfd: libc::c_int,
    oldpath: *const libc::c_char,
    newdirfd: libc::c_int,
    newpath: *const libc::c_char,
    flags: libc::c_uint,
) -> libc::c_int {
    unsafe { libc::syscall(libc::SYS_renameat2, olddirfd, oldpath, newdirfd, newpath, flags) as libc::c_int }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
