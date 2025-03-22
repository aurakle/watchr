use std::{path::Path, process::{self}, time::Duration};

use anyhow::{bail, Context, Result};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, net::UnixStream, spawn, time::sleep};

use crate::{data::{IpcEvent, UpdatePropertyMessage}, error::TimeoutError, get_clients, get_state, path::PATHS};

pub async fn start_mpv(file: &str, suffix: &str) -> Result<UnixStream> {
    let socket_path = PATHS.socket_path(suffix);
    let socket_path_str = socket_path.display();
    let file_path = if file.starts_with("~/") {
        shellexpand::tilde(file).to_string()
    } else {
        file.to_string()
    };

    // this might error but we don't care
    let _ = tokio::fs::remove_file(&socket_path).await;
    let child = process::Command::new("mpv")
        .arg(format!("--input-ipc-server={socket_path_str}"))
        .arg(file_path)
        .spawn()
        .context("Failed to start mpv")?;

    wait_until_file_exists(&socket_path, 5).await?;

    UnixStream::connect(&socket_path)
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
        .truncate(true)
        .open(PATHS.media_file())
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

    spawn(async {
        loop {
            sleep(Duration::from_secs(10)).await;
            get_clients().lock().await.ping().await;
        }
    });

    let mut reader = BufReader::new(reader);
    loop {
        let mut line = "".to_string();
        reader.read_line(&mut line).await?;

        if let Ok(event) = serde_json::from_str::<IpcEvent>(&line) {
            if event.event == "property-change" {
                get_state().update(event.name.clone(), event.data.clone()).await;
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
