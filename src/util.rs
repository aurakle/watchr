use std::{process::{self}, time::Duration};

use anyhow::{Context, Result};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, net::UnixStream, time::sleep};

use crate::{get_clients, IpcEvent};

pub async fn start_mpv(file: &str, suffix: &str) -> Result<UnixStream> {
    let socket_path = format!("~/.config/watchr/sock.{suffix}");
    let socket_path = shellexpand::tilde(&socket_path);
    process::Command::new("mpv")
        .arg(format!("--input-ipc-server={socket_path}"))
        .arg(shellexpand::tilde(file).to_string())
        .spawn()
        .context("Failed to start mpv")?;
    sleep(Duration::from_secs(5)).await;
    UnixStream::connect(socket_path.as_ref())
        .await
        .context("Failed to connect to mpv")
}

pub fn make_command(v: Value) -> String {
    format!("{{\"command\":{v}}}")
}

pub async fn start_download(addr: &str, port: u16) -> Result<()> {
    let mut stream = reqwest::get(format!("http://{}:{}/media.mkv", addr, port)).await?.bytes_stream();

    let mut writer = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .open(shellexpand::tilde("~/.config/watchr/media.mkv").to_string())
        .await?;

    while let Some(item) = stream.next().await {
        match item {
            Ok(bytes) => {
                tokio::io::copy(&mut bytes.as_ref(), &mut writer).await?;
            }
            Err(_) => {}
        }
    }

    Ok(())
}

pub async fn watch_mpv(file: &str) -> Result<()> {
    let mut socket = start_mpv(&file, "server").await?;
    let (reader, writer) = &mut socket.split();

    writer.write(make_command(json!(["observe_property_string", 1, "pause"])).as_bytes()).await?;
    writer.write(&[b'\n']).await?;

    writer.write(make_command(json!(["observe_property_string", 2, "playback-time"])).as_bytes()).await?;
    writer.write(&[b'\n']).await?;

    writer.flush().await?;

    let mut reader = BufReader::new(reader);
    loop {
        let mut line = "".to_string();
        reader.read_line(&mut line).await?;

        if let Ok(event) = serde_json::from_str::<IpcEvent>(&line) {
            if event.event == "property-change" {
                get_clients().lock().unwrap().update(event.name, event.data).await?;
            }
        }
    }
}
