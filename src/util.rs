use std::{process::{self}, sync::OnceLock};

use futures::executor::block_on;
use serde_json::Value;
use tokio::net::{unix::{OwnedReadHalf, OwnedWriteHalf, ReadHalf, WriteHalf}, UnixStream};

pub fn start_mpv() -> Result<UnixStream, String> {
    process::Command::new("mpv")
        .arg(format!("--input-ipc-server={}", shellexpand::tilde("~/.config/watchr/sock")))
        .arg(shellexpand::tilde("~/.config/watchr/media").to_string())
        .spawn()
        .or_else(|e| Err(format!("Failed to start mpv: {e}")))?;
    block_on(UnixStream::connect(shellexpand::tilde("~/.config/watchr/sock").as_ref())).map_err(|e| format!("Failed to connect to mpv: {e}"))
}

pub fn make_command(v: Value) -> String {
    format!("{{\"command\":{v}}}")
}
