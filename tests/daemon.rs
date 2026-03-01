use std::path::PathBuf;
use std::time::Duration;

use tempfile::tempdir;
use termwright::daemon::client::DaemonClient;
use termwright::daemon::server::{DaemonConfig, run_daemon};
use termwright::prelude::*;

/// Helper to spin up a daemon + connect a client.
async fn setup(cmd: &str) -> (DaemonClient, tokio::task::JoinHandle<Result<()>>) {
    let dir = tempdir().unwrap();
    let socket: PathBuf = dir.path().join("termwright.sock");

    let term = Terminal::builder()
        .size(80, 24)
        .spawn("sh", &["-c", cmd])
        .await
        .expect("spawn failed");

    let sock = socket.clone();
    let handle = tokio::spawn(run_daemon(DaemonConfig::new(sock), term));

    let client = loop {
        match DaemonClient::connect_unix(&socket).await {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    };

    client.handshake().await.expect("handshake failed");
    (client, handle)
}

async fn teardown(client: DaemonClient, handle: tokio::task::JoinHandle<Result<()>>) {
    let _ = client.close().await;
    let _ = handle.await;
}

// ── Screen tests ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_smoke_test_screen_and_wait() -> Result<()> {
    let (client, handle) = setup("printf READY; sleep 2").await;

    client
        .wait_for_text("READY", Some(Duration::from_secs(2)))
        .await?;

    let text = client.screen_text().await?;
    assert!(text.contains("READY"));

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn screenshot_returns_png_data() -> Result<()> {
    let (client, handle) = setup("printf 'Hello screenshot'; sleep 2").await;
    client
        .wait_for_text("Hello screenshot", Some(Duration::from_secs(2)))
        .await?;

    let png = client.screenshot_png().await?;
    // PNG header magic bytes
    assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);

    teardown(client, handle).await;
    Ok(())
}

// ── Input tests ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn type_and_press_enter() -> Result<()> {
    let (client, handle) = setup("read -r line; echo got:$line; sleep 2").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    client.r#type("hello").await?;
    client.press("Enter").await?;

    client
        .wait_for_text("got:hello", Some(Duration::from_secs(2)))
        .await?;

    let text = client.screen_text().await?;
    assert!(text.contains("got:hello"));

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hotkey_ctrl_c_terminates() -> Result<()> {
    let (client, handle) = setup("sleep 60").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    client.hotkey_ctrl('c').await?;

    // Give it a moment to exit
    tokio::time::sleep(Duration::from_millis(500)).await;

    let status: serde_json::Value = client
        .call_raw("status", serde_json::Value::Null)
        .await?;
    assert!(status["exited"].as_bool().unwrap_or(false));

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn press_modified_key() -> Result<()> {
    // Modified keys should not error
    let (client, handle) = setup("sleep 2").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    client.press("Ctrl+Right").await?;
    client.press("Shift+Up").await?;
    client.press("Ctrl+Shift+Left").await?;
    client.press("BackTab").await?;

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn press_function_keys() -> Result<()> {
    let (client, handle) = setup("sleep 2").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // F1-F12 should all succeed
    for n in 1..=12 {
        client.press(&format!("F{n}")).await?;
    }

    teardown(client, handle).await;
    Ok(())
}

// ── Wait tests ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_pattern_matches_regex() -> Result<()> {
    let (client, handle) = setup("printf 'version: v1.2.3'; sleep 2").await;

    client
        .wait_for_pattern(r"v\d+\.\d+\.\d+", Some(Duration::from_secs(2)))
        .await?;

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_text_gone() -> Result<()> {
    let (client, handle) =
        setup("printf 'Loading...'; sleep 0.5; printf '\\r          \\rDone'; sleep 2").await;

    client
        .wait_for_text("Loading...", Some(Duration::from_secs(2)))
        .await?;
    client
        .wait_for_text_gone("Loading...", Some(Duration::from_secs(3)))
        .await?;

    let text = client.screen_text().await?;
    assert!(!text.contains("Loading..."));

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_idle_stabilizes() -> Result<()> {
    let (client, handle) =
        setup("for i in 1 2 3; do printf \"$i\"; sleep 0.1; done; sleep 2").await;

    client
        .wait_for_idle(Duration::from_millis(500), Some(Duration::from_secs(5)))
        .await?;

    let text = client.screen_text().await?;
    assert!(text.contains("3"));

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_exit() -> Result<()> {
    let (client, handle) = setup("sleep 0.3; exit 0").await;

    let exit_code = client.wait_for_exit(Some(Duration::from_secs(3))).await?;
    assert_eq!(exit_code, 0);

    teardown(client, handle).await;
    Ok(())
}

// ── Search tests ────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn find_text_returns_positions() -> Result<()> {
    let (client, handle) =
        setup("printf 'Hello World\\nHello Again'; sleep 2").await;
    client
        .wait_for_text("Hello Again", Some(Duration::from_secs(2)))
        .await?;

    let matches = client.find_text("Hello").await?;
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].text, "Hello");
    assert_eq!(matches[0].position.row, 0);
    assert_eq!(matches[0].position.col, 0);
    assert_eq!(matches[1].position.row, 1);

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn find_pattern_with_regex() -> Result<()> {
    let (client, handle) =
        setup("printf 'v1.0 and v2.5'; sleep 2").await;
    client
        .wait_for_text("v2.5", Some(Duration::from_secs(2)))
        .await?;

    let matches = client.find_pattern(r"v\d+\.\d+").await?;
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].text, "v1.0");
    assert_eq!(matches[1].text, "v2.5");

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn find_text_no_match_returns_empty() -> Result<()> {
    let (client, handle) = setup("printf 'Hello'; sleep 2").await;
    client
        .wait_for_text("Hello", Some(Duration::from_secs(2)))
        .await?;

    let matches = client.find_text("nonexistent").await?;
    assert!(matches.is_empty());

    teardown(client, handle).await;
    Ok(())
}

// ── Assert tests ────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_expect_text_passes_when_absent() -> Result<()> {
    let (client, handle) = setup("printf 'Hello'; sleep 2").await;
    client
        .wait_for_text("Hello", Some(Duration::from_secs(2)))
        .await?;

    client.not_expect_text("ERROR").await?;

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_expect_text_fails_when_present() -> Result<()> {
    let (client, handle) = setup("printf 'ERROR found'; sleep 2").await;
    client
        .wait_for_text("ERROR", Some(Duration::from_secs(2)))
        .await?;

    let result = client.not_expect_text("ERROR").await;
    assert!(result.is_err());

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_expect_pattern_passes_when_no_match() -> Result<()> {
    let (client, handle) = setup("printf 'Hello world'; sleep 2").await;
    client
        .wait_for_text("Hello", Some(Duration::from_secs(2)))
        .await?;

    client.not_expect_pattern(r"ERROR|FATAL").await?;

    teardown(client, handle).await;
    Ok(())
}

// ── Box detection tests ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detect_boxes_finds_box_drawing() -> Result<()> {
    let (client, handle) = setup(
        r#"printf '┌────────┐\n│ Hello  │\n└────────┘\n'; sleep 2"#,
    )
    .await;
    client
        .wait_for_text("Hello", Some(Duration::from_secs(2)))
        .await?;

    let boxes = client.detect_boxes().await?;
    assert!(!boxes.is_empty(), "should detect at least one box");

    let b = &boxes[0];
    assert_eq!(b.region.start.row, 0);
    assert_eq!(b.region.start.col, 0);

    teardown(client, handle).await;
    Ok(())
}

// ── Resize tests ────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resize_succeeds() -> Result<()> {
    let (client, handle) = setup("sleep 5").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Resize should not error
    client.resize(120, 40).await?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Screen text should still be retrievable after resize
    let text = client.screen_text().await?;
    assert!(!text.is_empty());

    teardown(client, handle).await;
    Ok(())
}

// ── Mouse tests ─────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mouse_operations_succeed() -> Result<()> {
    let (client, handle) = setup("sleep 2").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // These should not error even if the program doesn't handle mouse
    use termwright::input::MouseButton;
    client.mouse_click(5, 10, MouseButton::Left).await?;
    client.mouse_move(3, 7).await?;

    teardown(client, handle).await;
    Ok(())
}

// ── Scrollback tests ────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scrollback_captures_scrolled_content() -> Result<()> {
    // Print more lines than the 24-row terminal can display
    let (client, handle) = setup(
        "for i in $(seq 1 50); do echo \"line-$i\"; done; sleep 2",
    )
    .await;

    // Wait for the last line to appear
    client
        .wait_for_text("line-50", Some(Duration::from_secs(3)))
        .await?;

    let scrollback = client.scrollback(None).await?;
    // Lines 1-26 should have scrolled off (50 lines, 24 visible)
    assert!(!scrollback.is_empty(), "scrollback should have content");

    // Check that early lines are in scrollback
    let joined = scrollback.join("\n");
    assert!(joined.contains("line-1"), "should contain early lines");

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scrollback_with_limit() -> Result<()> {
    let (client, handle) = setup(
        "for i in $(seq 1 50); do echo \"line-$i\"; done; sleep 2",
    )
    .await;

    client
        .wait_for_text("line-50", Some(Duration::from_secs(3)))
        .await?;

    let scrollback = client.scrollback(Some(5)).await?;
    assert!(scrollback.len() <= 5, "limit should cap result");

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scrollback_empty_when_no_overflow() -> Result<()> {
    let (client, handle) = setup("echo hello; sleep 2").await;
    client
        .wait_for_text("hello", Some(Duration::from_secs(2)))
        .await?;

    let scrollback = client.scrollback(None).await?;
    assert!(scrollback.is_empty(), "no scrollback when content fits");

    teardown(client, handle).await;
    Ok(())
}

// ── Cell and region tests ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cell_at_returns_character() -> Result<()> {
    let (client, handle) = setup("printf 'ABCD'; sleep 2").await;
    client
        .wait_for_text("ABCD", Some(Duration::from_secs(2)))
        .await?;

    let cell = client.cell_at(0, 0).await?;
    assert_eq!(cell.char, 'A');

    let cell = client.cell_at(0, 2).await?;
    assert_eq!(cell.char, 'C');

    teardown(client, handle).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn screen_region_returns_text() -> Result<()> {
    let (client, handle) = setup("printf 'Hello World\\nSecond Line'; sleep 2").await;
    client
        .wait_for_text("Second", Some(Duration::from_secs(2)))
        .await?;

    let region = client.screen_region(0, 0, 2, 11).await?;
    assert!(region.contains("Hello World"));
    assert!(region.contains("Second Line"));

    teardown(client, handle).await;
    Ok(())
}

// ── Handshake tests ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_includes_child_pid() -> Result<()> {
    let (client, handle) = setup("sleep 5").await;

    let result: serde_json::Value = client
        .call_raw("handshake", serde_json::Value::Null)
        .await?;

    assert!(result["protocol_version"].as_u64().is_some());
    assert!(result["child_pid"].as_u64().is_some());

    teardown(client, handle).await;
    Ok(())
}

// ── Status tests ────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_shows_running_process() -> Result<()> {
    let (client, handle) = setup("sleep 5").await;

    let status: serde_json::Value = client
        .call_raw("status", serde_json::Value::Null)
        .await?;
    assert!(!status["exited"].as_bool().unwrap_or(true));

    teardown(client, handle).await;
    Ok(())
}
