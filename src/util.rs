use std::{path::Path, process::{self}, time::Duration};

use anyhow::{bail, Context, Result};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, net::UnixStream, time::sleep};

use crate::{data::{IpcEvent, UpdatePropertyMessage}, error::TimeoutError, get_clients, get_state};

pub async fn start_mpv(file: &str, suffix: &str) -> Result<UnixStream> {
    let socket_path = format!("~/.config/watchr/sock.{suffix}");
    let socket_path = shellexpand::tilde(&socket_path);

    // this might error but we don't care
    let _ = tokio::fs::remove_file(socket_path.as_ref()).await;
    let child = process::Command::new("mpv")
        .arg(format!("--input-ipc-server={socket_path}"))
        .arg(shellexpand::tilde(file).to_string())
        .spawn()
        .context("Failed to start mpv")?;

    wait_until_file_exists(socket_path.as_ref(), 5).await?;

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

    tokio::spawn(async {
        loop {
            sleep(Duration::from_secs(5)).await;
            get_clients().lock().unwrap().ping().await;
        }
    });

    let mut reader = BufReader::new(reader);
    loop {
        let mut line = "".to_string();
        reader.read_line(&mut line).await?;

        if let Ok(event) = serde_json::from_str::<IpcEvent>(&line) {
            if event.event == "property-change" {
                get_state().update(event.name.clone(), event.data.clone());
                let _ = UpdatePropertyMessage::new(event.name, event.data).send_to_all().await;
            }
        }
    }
}

pub async fn wait_until_file_exists<P>(path: P, timeout: u32) -> Result<()>
where
    P: AsRef<Path>
{
    let mut time_left = timeout;
    while !path.as_ref().exists() {
        if time_left == 0 {
            bail!(TimeoutError("waiting for file existence".to_string()));
        }

        sleep(Duration::from_secs(1)).await;
        time_left -= 1;
    }

    Ok(())
}
