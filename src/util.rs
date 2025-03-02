use std::{process::{self}, sync::OnceLock, thread::sleep, time::Duration};

use serde_json::Value;
use tokio::net::{unix::{OwnedReadHalf, OwnedWriteHalf, ReadHalf, WriteHalf}, UnixStream};

pub async fn start_mpv(file: &str) -> Result<UnixStream, String> {
    process::Command::new("mpv")
        .arg(format!("--input-ipc-server={}", shellexpand::tilde("~/.config/watchr/sock")))
        .arg(shellexpand::tilde(file).to_string())
        .spawn()
        .or_else(|e| Err(format!("Failed to start mpv: {e}")))?;
    sleep(Duration::from_secs(5));
    UnixStream::connect(shellexpand::tilde("~/.config/watchr/sock").as_ref())
        .await
        .map_err(|e| format!("Failed to connect to mpv: {e}"))
}

pub fn make_command(v: Value) -> String {
    format!("{{\"command\":{v}}}")
}
