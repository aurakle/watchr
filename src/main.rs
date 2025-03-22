use std::{collections::HashMap, sync::OnceLock, time::Duration};

use actix_files::NamedFile;
use actix_web::{
    rt::time::timeout,
    web::{self, Data, Payload},
    App, Error, HttpRequest, HttpResponse, HttpServer, Responder,
};
use actix_ws::Session;
use anyhow::{anyhow, Context, Result};
use awc::ws::Frame::Binary;
use awc::Client;
use clap::{Parser, Subcommand};
use data::UpdatePropertyMessage;
use env_logger::Target;
use futures::{select, FutureExt, StreamExt};
use log::{error, info, warn, LevelFilter};
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    spawn,
    sync::Mutex,
    time::sleep,
};
use util::{start_download, start_mpv, watch_mpv};

use crate::{path::PATHS, util::make_command};

mod data;
mod error;
mod path;
mod util;

#[derive(Debug, Parser)]
#[command(name = "watchr")]
#[command(about = "Peer-to-peer Watch Together for local files", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Host(ServerArgs),
    Connect(ClientArgs),
}

#[derive(clap::Args, Debug, Clone)]
#[command(about = "Run watchr as the host", long_about = None)]
struct ServerArgs {
    #[arg(short, long, help = "The address to listen on")]
    addr: String,
    #[arg(short, long, default_value_t = 63063, help = "The port to listen on")]
    port: u16,
    #[arg(short, long, help = "The *.mkv file to play")]
    file: String,
}

#[derive(clap::Args, Debug, Clone)]
#[command(about = "Run watchr as a client", long_about = None)]
struct ClientArgs {
    #[arg(short, long, help = "The address to connect to")]
    addr: String,
    #[arg(short, long, default_value_t = 63063, help = "The port to connect to")]
    port: u16,
}

struct State {
    properties: Mutex<HashMap<String, String>>,
}

impl State {
    pub fn empty() -> Self {
        Self {
            properties: Mutex::new(HashMap::new()),
        }
    }

    pub async fn update(&self, property: String, value: String) {
        self.properties.lock().await.insert(property, value);
    }

    pub async fn sync_to(&self, session: &mut Session) {
        //TODO: make these actually fail
        for (k, v) in self.properties.lock().await.iter() {
            let _ = UpdatePropertyMessage::new(k, v).send_to(session).await;
        }
    }
}

fn get_state() -> &'static State {
    static STATE: OnceLock<State> = OnceLock::new();
    STATE.get_or_init(|| State::empty())
}

struct Clients {
    connections: Vec<Session>,
    last_time: f32,
}

impl Clients {
    pub fn empty() -> Self {
        Self {
            connections: Vec::new(),
            last_time: f32::MIN,
        }
    }

    pub fn push(&mut self, session: Session) {
        self.connections.push(session);
    }

    pub async fn ping(&mut self) {
        let mut do_retain = Vec::new();

        for session in self.connections.iter_mut().rev() {
            match session.binary(vec![0u8]).await {
                Ok(_) => {
                    do_retain.push(true);
                }
                Err(e) => {
                    info!("A client has disconnected: {e}");
                    do_retain.push(false);
                }
            }
        }

        self.connections
            .retain_mut(|_session| do_retain.pop().unwrap());
    }

    pub async fn update(&mut self, message: UpdatePropertyMessage) -> Result<()> {
        if message.property == "playback-time" {
            let value = message.value.parse::<f32>()?;
            if value - self.last_time < 0.1 && value - self.last_time > 0.0 {
                self.last_time = value;
                return Ok(());
            }
            self.last_time = value;
        }

        let mut do_retain = Vec::new();

        for session in self.connections.iter_mut().rev() {
            match session.binary(message.clone()).await {
                Ok(_) => {
                    do_retain.push(true);
                }
                Err(e) => {
                    info!("A client has disconnected: {e}");
                    do_retain.push(false);
                }
            }
        }

        self.connections
            .retain_mut(|_session| do_retain.pop().unwrap());

        let len = self.connections.len();
        info!(
            "{} client{} updated ({} = {})",
            len,
            if len == 1 { "" } else { "s" },
            message.property,
            message.value
        );

        Ok(())
    }
}

fn get_clients() -> &'static Mutex<Clients> {
    static CLIENTS: OnceLock<Mutex<Clients>> = OnceLock::new();
    CLIENTS.get_or_init(|| Mutex::new(Clients::empty()))
}

#[actix_web::main]
async fn main() {
    env_logger::Builder::from_default_env()
        .target(Target::Stdout)
        .filter_level(LevelFilter::Info)
        .init();

    if let Err(e) = PATHS.make_dirs() {
        error!("failed to make dirs: {e}");
        return;
    }

    match run().await {
        Err(message) => error!("Failure: {message}"),
        _ => (),
    }
}

async fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Connect(args) => {
            let client = Client::default();

            loop {
                match client
                    .ws(format!("ws://{}:{}/api", args.addr, args.port))
                    .connect()
                    .await
                {
                    Ok((res, mut ws)) => {
                        info!("Connected! HTTP response: {res:?}");

                        let addr = args.addr.clone();
                        spawn(async move {
                            if let Err(e) = start_download(&addr, args.port).await {
                                error!("Failure in download task: {e}");
                            }
                        });

                        sleep(Duration::from_secs(3)).await;

                        let socket =
                            start_mpv(PATHS.media_file().to_str().unwrap(), "client").await?;
                        let (reader, mut writer) = socket.into_split();
                        let mut reader = BufReader::new(reader);

                        // disable events so that the pipe doesn't block
                        writer
                            .write(make_command(json!(["disable_event", "all"])).as_bytes())
                            .await?;
                        writer.write(&[b'\n']).await?;

                        // wait for a response
                        let mut line = "".to_string();
                        reader.read_line(&mut line).await?;

                        info!("Initialized mpv, listening for property updates...");

                        spawn(async move {
                            let mut writer = tokio::fs::OpenOptions::new()
                                .write(true)
                                .create(true)
                                .open(PATHS.log_path("mpv"))
                                .await
                                .unwrap();

                            tokio::io::copy(&mut reader, &mut writer).await;
                        });

                        loop {
                            match ws.next().await {
                                Some(Ok(msg)) => {
                                    if let Binary(msg) = msg {
                                        if msg[0] == 0u8 {
                                            // heartbeat
                                        } else {
                                            let mut msg = Vec::from(msg);

                                            let value = msg.split_off(
                                                msg.iter()
                                                    .position(|b| *b == 0u8)
                                                    .context("Missing null byte")?
                                                    + 1,
                                            );
                                            let property_string = String::from_utf8(msg)
                                                .context("Invalid UTF-8 in property")?;
                                            let value_string = String::from_utf8(value)
                                                .context("Invalid UTF-8 in value")?;

                                            info!(
                                                "Setting property by IPC command ({} = {})",
                                                property_string, value_string
                                            );
                                            writer
                                                .write(
                                                    make_command(json!([
                                                        "set_property_string",
                                                        property_string,
                                                        value_string
                                                    ]))
                                                    .as_bytes(),
                                                )
                                                .await?;
                                            writer.write(&[b'\n']).await?;
                                        }
                                    } else {
                                        warn!("Received message, but it was not binary: {msg:?}");
                                    }
                                }
                                Some(Err(e)) => {
                                    warn!("Protocol error! Attempting to reconnect in 5 seconds. ({e})");
                                    sleep(Duration::from_secs(5)).await;
                                    break;
                                }
                                None => {
                                    warn!(
                                        "Got disconnected! Attempting to reconnect in 5 seconds."
                                    );
                                    sleep(Duration::from_secs(5)).await;
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to connect to websocket: {e}");
                        sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        }
        Command::Host(args) => {
            let ServerArgs { addr, port, file } = args.clone();
            let server = HttpServer::new(move || {
                App::new()
                    .app_data(web::Data::new(args.clone()))
                    .service(media)
                    .service(api)
            })
            .bind((addr, port))
            .context("Failed to bind to address")?;

            info!("Server configured, running...");
            let mut server = server.run().fuse();
            select! {
                result = watch_mpv(&file).fuse() => result,
                result = server => result.map_err(|e| anyhow!("Failed to run server {}", e)),
            }
        }
    }
}

#[actix_web::get("/api")]
async fn api(
    req: HttpRequest,
    stream: Payload,
    _args: Data<ServerArgs>,
) -> Result<HttpResponse, Error> {
    info!(
        "Client {} attempting to connect...",
        req.peer_addr().map_or("<unknown>".to_string(), |addr| addr
            .ip()
            .to_canonical()
            .to_string())
    );
    let (res, mut session, _stream) = actix_ws::handle(&req, stream)?;

    info!("Session acquired, sending current state...");
    get_state().sync_to(&mut session).await;
    get_clients().lock().await.push(session);
    info!("Client connected!");

    Ok(res)
}

#[actix_web::get("/media.mkv")]
async fn media(req: HttpRequest, args: Data<ServerArgs>) -> impl Responder {
    match NamedFile::open_async(shellexpand::tilde(&args.file).as_ref()).await {
        Ok(res) => res.respond_to(&req),
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}
