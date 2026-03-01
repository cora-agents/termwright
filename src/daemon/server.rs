use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::daemon::protocol::*;
use crate::error::{Result, TermwrightError};
use crate::input::{Key, MouseButton, ScrollDirection};
use crate::terminal::Terminal;

const PROTOCOL_VERSION: u32 = 1;

/// Result from serving a client connection.
enum ClientResult {
    /// Client disconnected normally, ready to accept next client.
    Continue,
    /// Client sent `close` command, daemon should exit.
    Close,
}

pub struct DaemonConfig {
    pub socket_path: PathBuf,
}

impl DaemonConfig {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }
}

pub async fn run_daemon(config: DaemonConfig, terminal: Terminal) -> Result<()> {
    let socket_path = config.socket_path;
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)
            .map_err(|e| TermwrightError::Ipc(format!("failed to remove socket: {e}")))?;
    }

    let listener = UnixListener::bind(&socket_path)
        .map_err(|e| TermwrightError::Ipc(format!("failed to bind socket: {e}")))?;

    let result = accept_clients(listener, &terminal).await;

    // Best-effort cleanup
    let _ = terminal.kill().await;
    let _ = std::fs::remove_file(&socket_path);

    result
}

/// Accept multiple client connections until `close` is called or process exits.
async fn accept_clients(listener: UnixListener, terminal: &Terminal) -> Result<()> {
    loop {
        // Check if the spawned process has exited
        if terminal.has_exited().await {
            return Ok(());
        }

        // Accept with a timeout so we can periodically check process status
        let accept_result =
            tokio::time::timeout(Duration::from_millis(500), listener.accept()).await;

        let stream = match accept_result {
            Ok(Ok((stream, _))) => stream,
            Ok(Err(e)) => {
                return Err(TermwrightError::Ipc(format!("accept failed: {e}")));
            }
            Err(_) => {
                // Timeout - loop back to check process status
                continue;
            }
        };

        // Serve this client; if they send `close`, we exit the loop
        match serve_client(stream, terminal).await {
            Ok(ClientResult::Continue) => {
                // Client disconnected normally, accept next client
                continue;
            }
            Ok(ClientResult::Close) => {
                // Client sent `close` command
                return Ok(());
            }
            Err(e) => {
                // Log error but keep accepting clients
                eprintln!("Client error: {e}");
                continue;
            }
        }
    }
}

async fn serve_client(stream: UnixStream, terminal: &Terminal) -> Result<ClientResult> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| TermwrightError::Ipc(format!("read failed: {e}")))?;
        if n == 0 {
            // Client disconnected, ready for next client
            return Ok(ClientResult::Continue);
        }

        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::err(0, "parse_error", e.to_string());
                write_response(&mut write_half, &resp).await?;
                continue;
            }
        };

        let resp = handle_request(terminal, req).await;
        write_response(&mut write_half, &resp).await?;

        if resp.error.as_ref().is_some_and(|e| e.code == "closing") {
            // Client sent `close` command, daemon should exit
            return Ok(ClientResult::Close);
        }
    }
}

async fn write_response(
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
    resp: &Response,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(resp).map_err(TermwrightError::Json)?;
    bytes.push(b'\n');
    write_half
        .write_all(&bytes)
        .await
        .map_err(|e| TermwrightError::Ipc(format!("write failed: {e}")))?;
    write_half
        .flush()
        .await
        .map_err(|e| TermwrightError::Ipc(format!("flush failed: {e}")))?;
    Ok(())
}

async fn handle_request(terminal: &Terminal, req: Request) -> Response {
    let id = req.id;

    let result: Result<Response> = (|| async {
        match req.method.as_str() {
            "handshake" => {
                let value = HandshakeResult {
                    protocol_version: PROTOCOL_VERSION,
                    termwright_version: env!("CARGO_PKG_VERSION").to_string(),
                    pid: std::process::id(),
                    child_pid: terminal.child_pid().await,
                };
                Ok(Response::ok(id, value)?)
            }
            "status" => {
                let exited = terminal.has_exited().await;
                let exit_code = if exited {
                    Some(terminal.wait_exit().await.unwrap_or(-1))
                } else {
                    None
                };
                Ok(Response::ok(id, StatusResult { exited, exit_code })?)
            }
            "screen" => {
                let params: ScreenParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let screen = terminal.screen().await;

                match params.format {
                    ScreenFormat::Text => Ok(Response::ok(id, screen.text())?),
                    ScreenFormat::Json => Ok(Response::ok(id, screen)?),
                    ScreenFormat::JsonCompact => Ok(Response::ok(id, screen.to_json_compact()?)?),
                }
            }
            "screenshot" => {
                let params: ScreenshotParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let mut screenshot = terminal.screenshot().await;
                if let Some(font) = params.font {
                    screenshot = screenshot.font(&font, params.font_size.unwrap_or(14.0));
                }
                if let Some(line_height) = params.line_height {
                    screenshot = screenshot.line_height(line_height);
                }

                let png = screenshot.to_png()?;
                let png_base64 = base64::engine::general_purpose::STANDARD.encode(png);
                Ok(Response::ok(id, ScreenshotResult { png_base64 })?)
            }
            "scrollback" => {
                let params: ScrollbackParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;
                let lines = terminal.scrollback_text(params.limit).await;
                Ok(Response::ok(id, lines)?)
            }
            "type" => {
                let params: TypeParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;
                terminal.type_str(&params.text).await?;
                Ok(Response::ok_empty(id))
            }
            "press" => {
                let params: PressParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;
                let key = parse_key(&params.key)?;
                terminal.send_key(key).await?;
                Ok(Response::ok_empty(id))
            }
            "hotkey" => {
                let params: HotkeyParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let ctrl = params.ctrl.unwrap_or(false);
                let alt = params.alt.unwrap_or(false);
                let shift = params.shift.unwrap_or(false);

                let key = match (ctrl, alt, shift) {
                    // Single modifier
                    (true, false, false) => Key::Ctrl(params.ch),
                    (false, true, false) => Key::Alt(params.ch),
                    // Combined modifiers — send raw escape sequence
                    _ if ctrl || alt || shift => {
                        // Build xterm modifier: 1 + (Shift?1:0) + (Alt?2:0) + (Ctrl?4:0)
                        let modifier: u8 = 1
                            + if shift { 1 } else { 0 }
                            + if alt { 2 } else { 0 }
                            + if ctrl { 4 } else { 0 };
                        // For single chars with combined modifiers, send raw
                        let mut seq = vec![0x1b];
                        seq.push(b'[');
                        seq.extend_from_slice(b"27;");
                        seq.extend_from_slice(modifier.to_string().as_bytes());
                        seq.push(b';');
                        seq.extend_from_slice((params.ch as u32).to_string().as_bytes());
                        seq.push(b'~');
                        terminal.send_raw(&seq).await?;
                        return Ok(Response::ok_empty(id));
                    }
                    // No modifiers
                    _ => Key::Char(params.ch),
                };
                terminal.send_key(key).await?;

                Ok(Response::ok_empty(id))
            }
            "raw" => {
                let params: RawParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(params.bytes_base64)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;
                terminal.send_raw(&bytes).await?;
                Ok(Response::ok_empty(id))
            }
            "mouse_move" => {
                let params: MouseMoveParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let held = parse_mouse_buttons(params.buttons.as_deref())?;
                terminal.mouse_move(params.row, params.col, held).await?;
                Ok(Response::ok_empty(id))
            }
            "mouse_click" => {
                let params: MouseClickParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let button = params
                    .button
                    .as_deref()
                    .unwrap_or("left")
                    .parse::<MouseButton>()
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                terminal.mouse_click(params.row, params.col, button).await?;
                Ok(Response::ok_empty(id))
            }
            "mouse_scroll" => {
                let params: MouseScrollParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let direction = params
                    .direction
                    .parse::<ScrollDirection>()
                    .map_err(|e| TermwrightError::Protocol(e))?;

                let count = params.count.unwrap_or(3);
                terminal
                    .mouse_scroll(params.row, params.col, direction, count)
                    .await?;
                Ok(Response::ok_empty(id))
            }
            "mouse_drag" => {
                let params: MouseDragParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let button = params
                    .button
                    .as_deref()
                    .unwrap_or("left")
                    .parse::<MouseButton>()
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                terminal
                    .mouse_drag(
                        params.start_row,
                        params.start_col,
                        params.end_row,
                        params.end_col,
                        button,
                    )
                    .await?;
                Ok(Response::ok_empty(id))
            }
            "mouse_double_click" => {
                let params: MouseClickParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let button = params
                    .button
                    .as_deref()
                    .unwrap_or("left")
                    .parse::<MouseButton>()
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                terminal
                    .mouse_double_click(params.row, params.col, button)
                    .await?;
                Ok(Response::ok_empty(id))
            }
            "wait_for_text" => {
                let params: WaitForTextParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let mut waiter = terminal.expect(&params.text);
                if let Some(timeout_ms) = params.timeout_ms {
                    waiter = waiter.timeout(Duration::from_millis(timeout_ms));
                }
                waiter.await?;
                Ok(Response::ok_empty(id))
            }
            "wait_for_pattern" => {
                let params: WaitForPatternParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let mut waiter = terminal.expect_pattern(&params.pattern);
                if let Some(timeout_ms) = params.timeout_ms {
                    waiter = waiter.timeout(Duration::from_millis(timeout_ms));
                }
                waiter.await?;
                Ok(Response::ok_empty(id))
            }
            "wait_for_idle" => {
                let params: WaitForIdleParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let mut waiter = terminal.wait_idle(Duration::from_millis(params.idle_ms));
                if let Some(timeout_ms) = params.timeout_ms {
                    waiter = waiter.timeout(Duration::from_millis(timeout_ms));
                }
                waiter.await?;
                Ok(Response::ok_empty(id))
            }
            "wait_for_text_gone" => {
                let params: WaitForTextGoneParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let mut waiter = terminal.expect_gone(&params.text);
                if let Some(timeout_ms) = params.timeout_ms {
                    waiter = waiter.timeout(Duration::from_millis(timeout_ms));
                }
                waiter.await?;
                Ok(Response::ok_empty(id))
            }
            "wait_for_pattern_gone" => {
                let params: WaitForPatternGoneParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let mut waiter = terminal.expect_pattern_gone(&params.pattern);
                if let Some(timeout_ms) = params.timeout_ms {
                    waiter = waiter.timeout(Duration::from_millis(timeout_ms));
                }
                waiter.await?;
                Ok(Response::ok_empty(id))
            }
            "not_expect_text" => {
                let params: NotExpectTextParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let screen = terminal.screen().await;
                if screen.contains(&params.text) {
                    return Err(TermwrightError::Protocol(format!(
                        "text '{}' was found on screen (expected not present)",
                        params.text
                    )));
                }
                Ok(Response::ok_empty(id))
            }
            "not_expect_pattern" => {
                let params: NotExpectPatternParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let screen = terminal.screen().await;
                let re = regex::Regex::new(&params.pattern)
                    .map_err(|e| TermwrightError::Protocol(format!("invalid regex: {}", e)))?;
                if re.is_match(&screen.text()) {
                    return Err(TermwrightError::Protocol(format!(
                        "pattern '{}' matched on screen (expected no match)",
                        params.pattern
                    )));
                }
                Ok(Response::ok_empty(id))
            }
            "find_text" => {
                let params: FindTextParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let screen = terminal.screen().await;
                let matches = screen.find_text(&params.text);
                Ok(Response::ok(id, matches)?)
            }
            "find_pattern" => {
                let params: FindPatternParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let screen = terminal.screen().await;
                let matches = screen.find_pattern(&params.pattern)
                    .map_err(|e| TermwrightError::Protocol(format!("invalid regex: {}", e)))?;
                Ok(Response::ok(id, matches)?)
            }
            "detect_boxes" => {
                let screen = terminal.screen().await;
                let boxes = screen.detect_boxes();
                Ok(Response::ok(id, boxes)?)
            }
            "cell_at" => {
                let params: CellAtParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;
                let screen = terminal.screen().await;
                match screen.cell(params.row, params.col) {
                    Some(cell) => Ok(Response::ok(id, cell)?),
                    None => Err(TermwrightError::Protocol(format!(
                        "cell ({}, {}) out of bounds",
                        params.row, params.col
                    ))),
                }
            }
            "screen_region" => {
                let params: ScreenRegionParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;
                let screen = terminal.screen().await;
                let region = crate::screen::Region {
                    start: crate::screen::Position::new(params.start_row, params.start_col),
                    end: crate::screen::Position::new(params.end_row, params.end_col),
                };
                match params.format {
                    ScreenFormat::Text => {
                        let cells = screen.cells_in_region(&region);
                        let text: String = cells
                            .iter()
                            .map(|row| {
                                row.iter()
                                    .map(|c| c.char)
                                    .collect::<String>()
                                    .trim_end()
                                    .to_string()
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        Ok(Response::ok(id, text)?)
                    }
                    _ => {
                        let cells = screen.cells_in_region(&region);
                        Ok(Response::ok(id, cells)?)
                    }
                }
            }
            "wait_for_cursor_at" => {
                let params: WaitForCursorAtParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;
                let position = crate::screen::Position {
                    row: params.row,
                    col: params.col,
                };
                let builder = terminal.wait_cursor(position);
                if let Some(timeout_ms) = params.timeout_ms {
                    builder.timeout(Duration::from_millis(timeout_ms)).await?;
                } else {
                    builder.await?;
                }
                Ok(Response::ok_empty(id))
            }
            "wait_for_exit" => {
                let params: WaitForExitParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;

                let exit_code = if let Some(timeout_ms) = params.timeout_ms {
                    tokio::time::timeout(Duration::from_millis(timeout_ms), terminal.wait_exit())
                        .await
                        .map_err(|_| TermwrightError::Timeout {
                            condition: "process to exit".to_string(),
                            timeout: Duration::from_millis(timeout_ms),
                        })??
                } else {
                    terminal.wait_exit().await?
                };

                Ok(Response::ok(id, WaitForExitResult { exit_code })?)
            }
            "resize" => {
                let params: ResizeParams = serde_json::from_value(req.params)
                    .map_err(|e| TermwrightError::Protocol(e.to_string()))?;
                terminal.resize(params.cols, params.rows).await?;
                Ok(Response::ok_empty(id))
            }
            "close" => {
                let _ = terminal.kill().await;
                Ok(Response::err(id, "closing", "closing"))
            }
            other => Ok(Response::err(
                id,
                "unknown_method",
                format!("unknown method: {other}"),
            )),
        }
    })()
    .await;

    match result {
        Ok(r) => r,
        Err(e) => Response::err(id, "error", e.to_string()),
    }
}

fn parse_key(input: &str) -> Result<Key> {
    input
        .parse::<Key>()
        .map_err(|e| TermwrightError::Protocol(format!("Protocol error: {e}")))
}

fn parse_mouse_buttons(buttons: Option<&[String]>) -> Result<Vec<MouseButton>> {
    let Some(buttons) = buttons else {
        return Ok(Vec::new());
    };

    buttons
        .iter()
        .map(|b| {
            b.parse::<MouseButton>()
                .map_err(|e| TermwrightError::Protocol(e.to_string()))
        })
        .collect()
}
