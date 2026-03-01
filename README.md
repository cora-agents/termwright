# Termwright

A Playwright-like automation framework for terminal TUI applications.

Termwright enables AI agents and integration tests to interact with and observe terminal user interfaces by wrapping applications in a pseudo-terminal (PTY).

## Features

- **PTY Wrapping**: Spawn and control any terminal application
- **Screen Reading**: Access text, colors, cursor position, and cell attributes
- **Wait Conditions**: Wait for text, regex patterns, screen stability, cursor position, color, screen change, or process exit
- **Input Simulation**: Send keystrokes, modifier combos, mouse clicks/scroll/drag, and bracketed paste
- **Multiple Output Formats**: Plain text, JSON (for AI agents), ANSI SGR, and PNG screenshots
- **GIF Recording**: Record terminal sessions as animated GIFs with configurable frame intervals
- **Box Detection**: Detect UI boundaries using box-drawing characters (single, double, heavy, rounded, ASCII)
- **Screen Inspection**: Cell-level attribute access, rectangular region extraction, scrollback buffer
- **Screenshot Theming**: Configurable foreground/background colors with cursor rendering
- **Framework Agnostic**: Works with any TUI framework (ratatui, crossterm, ncurses, etc.)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
termwright = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

Or install the CLI:

```bash
cargo install termwright
```

## Quick Start

### Library Usage

```rust
use termwright::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Spawn a terminal application
    let term = Terminal::builder()
        .size(80, 24)
        .spawn("vim", &["test.txt"])
        .await?;

    // Wait for the application to be ready
    term.expect("VIM")
        .timeout(Duration::from_secs(5))
        .await?;

    // Send input
    term.send_key(Key::Char('i')).await?;
    term.type_str("Hello, world!").await?;
    term.send_key(Key::Escape).await?;

    // Query screen state
    let screen = term.screen().await;
    assert!(screen.contains("Hello, world!"));

    // Get structured output for AI agents
    println!("{}", screen.to_json()?);

    // Take a screenshot
    term.screenshot().await.save("vim.png")?;

    // Quit the application
    term.type_str(":q!").await?;
    term.enter().await?;
    term.wait_exit().await?;

    Ok(())
}
```

### CLI Usage

Capture terminal output as text:

```bash
termwright run -- ls -la
```

Take a screenshot of a TUI application:

```bash
termwright screenshot --wait-for "VIM" -o vim.png -- vim test.txt
```

Get JSON output for AI processing:

```bash
termwright run --format json -- htop
```

### Daemon Usage

The `daemon` subcommand runs a long-lived terminal session and exposes a local Unix socket for automation. This is useful when you want to keep an app running and interact with it incrementally (similar to how Playwright keeps a browser process alive).

Start a daemon (foreground; blocks until you close it):

```bash
termwright daemon -- vim test.txt
# prints a socket path like:
# /tmp/termwright-12345.sock
```

Start a daemon in the background (returns immediately):

```bash
SOCK=$(termwright daemon --background -- vim test.txt)
echo "$SOCK"
```

Stop a daemon (sends a `close` request):

```bash
printf '{"id":1,"method":"close","params":null}\n' | nc -U "$SOCK"
```

## Shell Scripting Quick Start

The daemon mode makes termwright ideal for shell-based E2E testing of TUI applications. Here's how to get started:

### Prerequisites

```bash
# Install termwright
cargo install termwright

# Install helper tools
brew install socat jq  # macOS
# or: sudo apt-get install socat jq  # Ubuntu/Debian
```

### Basic Pattern

```bash
#!/bin/bash
set -euo pipefail

# 1. Start the daemon with your TUI app
SOCK="/tmp/my-test-$$.sock"
termwright daemon --socket "$SOCK" --cols 80 --rows 24 -- ./my-tui-app &

# Wait for socket
while [ ! -S "$SOCK" ]; do sleep 0.1; done

# 2. Helper function to send commands
tw() {
    echo "$1" | socat - UNIX-CONNECT:"$SOCK"
}

# 3. Wait for app to be ready
tw '{"id":1,"method":"wait_for_text","params":{"text":"Welcome","timeout_ms":5000}}'

# 4. Interact with the app
tw '{"id":2,"method":"press","params":{"key":"Enter"}}'
tw '{"id":3,"method":"type","params":{"text":"hello world"}}'
tw '{"id":4,"method":"hotkey","params":{"ctrl":true,"ch":"s"}}'  # Ctrl+S

# 5. Read screen content
SCREEN=$(tw '{"id":5,"method":"screen","params":{"format":"text"}}' | jq -r '.result')
echo "$SCREEN"

# 6. Take a screenshot
RESULT=$(tw '{"id":6,"method":"screenshot","params":{}}')
echo "$RESULT" | jq -r '.result.png_base64' | base64 -d > screenshot.png

# 7. Clean up
tw '{"id":99,"method":"close","params":null}'
```

### Available Daemon Commands

**Session**

| Method | Params | Description |
|--------|--------|-------------|
| `handshake` | `null` | Get daemon info (pid, child_pid, version) |
| `status` | `null` | Check if process is still running |
| `close` | `null` | Terminate the daemon and child process |
| `resize` | `{"cols":120,"rows":40}` | Resize the terminal |
| `start_recording` | `{"interval_ms":100}` | Start GIF recording (default 100ms between frames) |
| `stop_recording` | `null` | Stop recording, returns `{gif_base64, frames}` |

**Screen**

| Method | Params | Description |
|--------|--------|-------------|
| `screen` | `{"format":"text"\|"json"\|"ansi"}` | Get screen content |
| `screenshot` | `{"fg_color":"#fff","bg_color":"#000"}` | Get PNG screenshot as base64 (colors optional) |
| `scrollback` | `{"limit":50}` | Get scrollback buffer lines |

**Input**

| Method | Params | Description |
|--------|--------|-------------|
| `type` | `{"text":"..."}` | Type text |
| `paste` | `{"text":"..."}` | Paste text (bracketed paste mode) |
| `press` | `{"key":"Ctrl+Shift+Right"}` | Press a key or key combo |
| `hotkey` | `{"ctrl":true,"alt":false,"shift":false,"ch":"c"}` | Press key with modifiers |
| `raw` | `{"bytes_base64":"..."}` | Send raw bytes |
| `mouse_click` | `{"row":5,"col":10,"button":"left"}` | Click at position |
| `mouse_double_click` | `{"row":5,"col":10}` | Double-click at position |
| `mouse_scroll` | `{"row":5,"col":10,"direction":"down","count":3}` | Scroll wheel |
| `mouse_drag` | `{"start_row":1,"start_col":5,"end_row":1,"end_col":20}` | Drag from A to B |
| `mouse_move` | `{"row":5,"col":10}` | Move mouse cursor |

**Wait Conditions**

| Method | Params | Description |
|--------|--------|-------------|
| `wait_for_text` | `{"text":"...","timeout_ms":5000}` | Wait for text to appear |
| `wait_for_pattern` | `{"pattern":"v\\d+","timeout_ms":5000}` | Wait for regex match |
| `wait_for_idle` | `{"idle_ms":500,"timeout_ms":5000}` | Wait for screen to stabilize |
| `wait_for_exit` | `{"timeout_ms":5000}` | Wait for process to exit, returns exit code |
| `wait_for_text_gone` | `{"text":"Loading...","timeout_ms":5000}` | Wait for text to disappear |
| `wait_for_pattern_gone` | `{"pattern":"\\d+%","timeout_ms":5000}` | Wait for pattern to stop matching |
| `wait_for_cursor_at` | `{"row":0,"col":5,"timeout_ms":5000}` | Wait for cursor to reach position |
| `wait_for_color_at` | `{"row":0,"col":0,"color":"red","target":"fg"}` | Wait for cell color to match |
| `wait_for_screen_change` | `{"last_hash":"abc","timeout_ms":5000}` | Wait for screen content to change |

**Search & Inspect**

| Method | Params | Description |
|--------|--------|-------------|
| `find_text` | `{"text":"Ready"}` | Find all occurrences, returns positions |
| `find_pattern` | `{"pattern":"v\\d+\\.\\d+"}` | Find all regex matches |
| `detect_boxes` | `null` | Detect box-drawing rectangles |
| `cell_at` | `{"row":0,"col":5}` | Get cell char, colors, and attributes |
| `screen_region` | `{"start_row":0,"start_col":0,"end_row":2,"end_col":10}` | Extract rectangular region |

**Assertions**

| Method | Params | Description |
|--------|--------|-------------|
| `not_expect_text` | `{"text":"ERROR"}` | Assert text is NOT on screen (error if found) |
| `not_expect_pattern` | `{"pattern":"error\|fail"}` | Assert pattern does NOT match |

### Reusable Test Library

For multiple tests, create a shared library (e.g., `lib.sh`):

```bash
#!/bin/bash
# lib.sh - Shared test helpers

REQUEST_ID=1

next_id() {
    local id=$REQUEST_ID
    REQUEST_ID=$((REQUEST_ID + 1))
    echo $id
}

# Send command with auto-incrementing ID
tw_auto() {
    local sock="$1"
    local method="$2"
    local params="${3:-null}"
    local id=$(next_id)
    echo "{\"id\":$id,\"method\":\"$method\",\"params\":$params}" | \
        socat - UNIX-CONNECT:"$sock"
}

# Convenience wrappers
get_screen() { tw_auto "$1" "screen" '{"format":"text"}' | jq -r '.result'; }
press()      { tw_auto "$1" "press" "{\"key\":\"$2\"}" > /dev/null; }
type_text()  { tw_auto "$1" "type" "{\"text\":\"$2\"}" > /dev/null; }
ctrl()       { tw_auto "$1" "hotkey" "{\"ctrl\":true,\"ch\":\"$2\"}" > /dev/null; }
wait_idle()  { tw_auto "$1" "wait_for_idle" "{\"idle_ms\":${2:-500},\"timeout_ms\":${3:-5000}}"; }

# Assert screen contains text
assert_contains() {
    local sock="$1" expected="$2" desc="${3:-contains '$2'}"
    if get_screen "$sock" | grep -q "$expected"; then
        echo "PASS: $desc"
        return 0
    else
        echo "FAIL: $desc"
        return 1
    fi
}
```

### Example Test Script

```bash
#!/bin/bash
# test_my_app.sh
source "$(dirname "$0")/lib.sh"

SOCK="/tmp/test-$$.sock"
termwright daemon --socket "$SOCK" -- ./my-app &
while [ ! -S "$SOCK" ]; do sleep 0.1; done

# Wait for app to initialize
wait_idle "$SOCK" > /dev/null

# Run tests
assert_contains "$SOCK" "Main Menu" "App shows main menu"

press "$SOCK" "Enter"
wait_idle "$SOCK" > /dev/null
assert_contains "$SOCK" "Settings" "Enter opens settings"

ctrl "$SOCK" "q"
echo "All tests passed!"
```

### Recording a Session as GIF

Capture an animated GIF of a terminal workflow:

```bash
# Start recording (100ms between frames)
tw '{"id":1,"method":"start_recording","params":{"interval_ms":100}}'

# Interact with the app
tw '{"id":2,"method":"type","params":{"text":"hello world"}}'
tw '{"id":3,"method":"press","params":{"key":"Enter"}}'
sleep 1  # let things happen visually

# Stop recording and save GIF
RESULT=$(tw '{"id":4,"method":"stop_recording","params":null}')
echo "$RESULT" | jq -r '.result.gif_base64' | base64 -d > session.gif
FRAMES=$(echo "$RESULT" | jq '.result.frames')
echo "Captured $FRAMES frames"
```

Recording persists across separate `exec` calls, so you can start recording, run multiple interactions, and stop later.

### Themed Screenshots

Customize screenshot colors:

```bash
# Dark theme with green text
tw '{"id":1,"method":"screenshot","params":{"fg_color":"#00ff00","bg_color":"#1a1a2e"}}'

# Named colors: black, red, green, yellow, blue, magenta, cyan, white
# Hex colors: #rrggbb
# Indexed colors: 0-255
```

Screenshots include the cursor as an inverted block at its current position.

### Screen Change Detection

Efficiently wait for screen updates without polling:

```bash
# Get initial screen state
RESULT=$(tw '{"id":1,"method":"wait_for_screen_change","params":null}')
HASH=$(echo "$RESULT" | jq -r '.result.hash')

# Block until screen changes
RESULT=$(tw '{"id":2,"method":"wait_for_screen_change","params":{"last_hash":"'$HASH'","timeout_ms":10000}}')
echo "$RESULT" | jq -r '.result.text'  # new screen content
```

### Key Names Reference

Common key names for the `press` command:

- Navigation: `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`
- Actions: `Enter`, `Escape`, `Tab`, `Backspace`, `Delete`, `Insert`
- Function keys: `F1` through `F12`
- Characters: Any single character like `a`, `1`, `?`

## CLI Reference

Global options (apply to any command that spawns a terminal):

- `--no-default-env`: Disable default terminal env handling (`TERM`/`COLORTERM` injection and clearing inherited `NO_COLOR`).
- `--no-osc-emulation`: Disable OSC 10/11/12 color query emulation.
- Terminal query emulation defaults to on for OSC 10/11/12 and CSI 6n/?6n cursor-position requests.

### `termwright fonts`

List available font families on the system (helpful for selecting a monospace font for screenshots).

```
termwright fonts
```

### `termwright run`

Run a command and capture its output.

```
termwright run [OPTIONS] -- <COMMAND> [ARGS]...

Options:
  --cols <COLS>          Terminal width [default: 80]
  --rows <ROWS>          Terminal height [default: 24]
  --wait-for <TEXT>      Wait for this text to appear before capturing
  --delay <MS>           Delay in milliseconds before capturing [default: 500]
  --format <FORMAT>      Output format: text, json, json-compact [default: text]
  --timeout <SECS>       Timeout for wait conditions [default: 30]
```

### `termwright screenshot`

Take a PNG screenshot of a terminal application.

```
termwright screenshot [OPTIONS] -- <COMMAND> [ARGS]...

Options:
  --cols <COLS>          Terminal width [default: 80]
  --rows <ROWS>          Terminal height [default: 24]
  --wait-for <TEXT>      Wait for this text to appear before capturing
  --delay <MS>           Delay in milliseconds before capturing [default: 500]
  -o, --output <PATH>    Output file path (defaults to stdout)
  --font <NAME>          Font name for rendering
  --font-size <SIZE>     Font size in pixels [default: 14]
  --timeout <SECS>       Timeout for wait conditions [default: 30]
```

### `termwright run-steps`

Run a YAML or JSON steps file for end-to-end testing.

```
termwright run-steps [OPTIONS] <FILE>

Options:
  --connect <PATH>       Connect to an existing daemon socket instead of spawning
  --trace                Write a trace.json file in the artifacts directory
```

When using `run-steps`, you can also control spawn behavior per session:

- `session.noDefaultEnv: true` disables default terminal env handling (`TERM`/`COLORTERM` injection and clearing inherited `NO_COLOR`).
- `session.noOscEmulation: true` disables OSC 10/11/12 emulation for that session.

### `termwright exec`

Execute a single daemon request and print the response.

```
termwright exec --socket <PATH> --method <NAME> [--params <JSON>]
```

### `termwright hub`

Start or stop multiple daemon sessions for parallel agents.

```
termwright hub start --count <N> [--cols <COLS>] [--rows <ROWS>] [--output <FILE>] -- <COMMAND> [ARGS]...
termwright hub stop --socket <PATH>... [--input <FILE>]
```

### `termwright daemon`

Run a single TUI session and expose it over a Unix socket.

```
termwright daemon [OPTIONS] -- <COMMAND> [ARGS]...

Options:
  --cols <COLS>          Terminal width [default: 80]
  --rows <ROWS>          Terminal height [default: 24]
  --socket <PATH>        Unix socket path (defaults to a temp path)
  --background           Start daemon in the background
```

The command prints the socket path to stdout.

## Daemon User Guide

### Connecting from Rust

```rust
use std::time::Duration;
use termwright::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let client = DaemonClient::connect_unix("/tmp/termwright-12345.sock").await?;

    // Sanity check: talk to the server
    let info = client.handshake().await?;
    println!("daemon pid={}, child={:?}", info.pid, info.child_pid);

    // Wait and read screen
    client.wait_for_text("VIM", Some(Duration::from_secs(5))).await?;
    println!("{}", client.screen_text().await?);

    // Keyboard
    client.press("Escape").await?;
    client.hotkey_ctrl('r').await?;
    client.r#type(":q!\n").await?;
    client.paste("multi\nline\ncontent").await?;  // Bracketed paste

    // Mouse (row/col are 0-based cell coordinates)
    client.mouse_click(10, 10, MouseButton::Left).await?;
    client.mouse_scroll(5, 5, ScrollDirection::Down, Some(3)).await?;
    client.mouse_drag(1, 5, 1, 20, MouseButton::Left).await?;

    // Search
    let matches = client.find_text("error").await?;
    let pattern_matches = client.find_pattern(r"\d+\.\d+").await?;

    // Inspect
    let cell = client.cell_at(0, 5).await?;
    let region = client.screen_region(0, 0, 5, 40).await?;
    let boxes = client.detect_boxes().await?;
    let scrollback = client.scrollback(Some(100)).await?;

    // Wait conditions
    client.wait_for_cursor_at(0, 5, Some(Duration::from_secs(5))).await?;
    client.wait_for_color_at(0, 0, "red", "fg", Some(Duration::from_secs(5))).await?;
    client.wait_for_text_gone("Loading...", Some(Duration::from_secs(10))).await?;

    // Screen change detection
    let change = client.wait_for_screen_change(None, None).await?;
    let next = client.wait_for_screen_change(
        Some(change.hash), Some(Duration::from_secs(5))
    ).await?;

    // GIF recording
    client.start_recording(100).await?;  // 100ms between frames
    // ... interact with the app ...
    let recording = client.stop_recording().await?;
    println!("Recorded {} frames", recording.frames);
    // recording.gif_base64 contains the GIF data

    // Screenshots (with optional theming)
    let png_bytes = client.screenshot_png().await?;

    // Shut down daemon + child process
    client.close().await?;
    Ok(())
}
```

### Notes / Caveats

- The daemon is local-only: it listens on a Unix socket you control.
- Mouse events are best-effort: many TUIs ignore mouse input unless they explicitly enable mouse reporting.
- Coordinate system for `mouse_move`/`mouse_click` is `row`/`col` in terminal cells (0-based).

## API Overview

### Terminal

The main entry point for controlling terminal applications:

```rust
let term = Terminal::builder()
    .size(80, 24)
    .spawn("vim", &["file.txt"])
    .await?;

// Input
term.type_str("hello").await?;
term.send_key(Key::Enter).await?;
term.enter().await?;  // Shorthand for Enter key

// Screen access
let screen = term.screen().await;

// Wait conditions
term.expect("Ready").timeout(Duration::from_secs(5)).await?;
term.wait_exit().await?;

// Screenshots
term.screenshot().await.save("output.png")?;
```

### Screen

Query the terminal screen state:

```rust
let screen = term.screen().await;

// Text access
let text = screen.text();
let line = screen.line(0);
assert!(screen.contains("hello"));

// Cell-level access
let cell = screen.cell(0, 0);
println!("Char: {}, FG: {:?}, BG: {:?}", cell.char, cell.fg, cell.bg);

// Cursor position
let cursor = screen.cursor();
println!("Cursor at row={}, col={}", cursor.row, cursor.col);

// Region extraction
let region = screen.region(0..10, 0..5);

// Pattern matching
if let Some(pos) = screen.find_text("error") {
    println!("Found at row={}, col={}", pos.row, pos.col);
}

// Box detection (UI boundaries)
let boxes = screen.detect_boxes();

// Output formats
println!("{}", screen.to_json()?);        // Pretty JSON
println!("{}", screen.to_json_compact()?); // Compact JSON
```

### Keys

Available key types for input:

```rust
Key::Char('a')      // Regular characters
Key::Enter          // Enter/Return
Key::Tab            // Tab
Key::Escape         // Escape
Key::Backspace      // Backspace
Key::Up, Key::Down, Key::Left, Key::Right  // Arrow keys
Key::Home, Key::End
Key::PageUp, Key::PageDown
Key::Insert, Key::Delete
Key::F(1)..Key::F(12)  // Function keys
Key::Ctrl('c')      // Ctrl combinations
Key::Alt('x')       // Alt combinations
```

## Requirements

- Rust 1.85.0 or later (Edition 2024)
- macOS or Linux (Windows not supported)
- For screenshots: A monospace font (uses system fonts via font-kit)

## Use Cases

- **AI Agents**: Enable LLMs to observe and interact with terminal UIs via JSON output
- **Integration Testing**: Automated testing of TUI applications
- **Documentation**: Generate screenshots for documentation
- **Accessibility**: Extract text content from visual terminal applications

## License

MIT License - see [LICENSE](LICENSE) for details.
