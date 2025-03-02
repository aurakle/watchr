use std::{fs, io::{BufRead, BufReader, BufWriter, Write}, os::unix::net::UnixStream, process, sync::{Mutex, OnceLock}, thread::{sleep, spawn}, time::Duration};

use actix_files::NamedFile;
use actix_web::{rt::time::timeout, web::{self, Data, Payload}, App, Error, HttpRequest, HttpResponse, HttpServer, Responder};
use actix_ws::Session;
use awc::Client;
use awc::ws::Frame::Binary;
use clap::{command, Parser, Subcommand};
use env_logger::Target;
use futures::StreamExt;
use log::{error, info, warn, LevelFilter};
use serde::Serialize;
use serde_json::json;

use crate::util::make_command;

mod util;

#[derive(Debug, Parser)]
#[command(name = "watchr")]
#[command(about = "TODO", long_about = None)]
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

#[derive(Debug, Deserialize)]
struct IpcEvent {
    event: String,
    id: i32,
    data: String,
    name: String,
}

struct Clients {
    connections: Mutex<Vec<Session>>
}

impl Clients {
    pub fn empty() -> Self {
        Self {
            connections: Mutex::new(Vec::new())
        }
    }

    pub fn push(&self, session: Session) {
        self.connections.lock().unwrap().push(session);
    }

    pub async fn update(&self, property: String, value: String) {
        let mut response = Vec::new();
        response.append(property.as_bytes());
        response.push(0u8);
        response.append(value.as_bytes());

        let sessions = self.connections.lock().unwrap();
        sessions.retain(|session| async {
            match session.binary(response.clone()).await {
                Ok(_) => true,
                Err(_) => {
                    info!("A client has disconnected");
                    false
                }
            }
        });

        let len = sessions.len();
        info!("{} client{} updated", len, if len == 1 { "" } else { "s" });
    }
}

fn get_clients() -> &'static Clients {
    static CLIENTS: OnceLock<Clients> = OnceLock::new();
    CLIENTS.get_or_init(|| Clients::empty())
}

#[actix_web::main]
async fn main() {
    env_logger::Builder::from_default_env()
        .target(Target::Stdout)
        .filter_level(LevelFilter::Info)
        .init();

    match run().await {
        Err(message) => error!("Server failure: {message}"),
        _ => ()
    }
}

async fn run() -> Result<(), String> {
    fs::create_dir_all(shellexpand::tilde("~/.config/watchr").as_ref());

    match Cli::parse().command {
        Command::Connect(args) => {
            let client = Client::default();

            loop {
                match client.ws(format!("ws://{}:{}/api", args.addr, args.port)).connect().await {
                    Ok((res, mut ws)) => {
                        info!("Connected! HTTP response: {res:?}");

                        //TODO: open mpv with the file at /media

                        loop {
                            match timeout(Duration::from_secs(20), ws.next()).await {
                                Ok(Some(msg)) => {
                                    if let Ok(Binary(msg)) = msg {
                                        let property = Vec::from(msg);
                                        //TODO: the question mark on this line will cause a crash
                                        //on the slightest malformed message from the host
                                        let value = property.split_off(property.binary_search(&0u8).map_err(|e| format!("Received message is not valid: {e}"))? + 1);
                                        //TODO: update mpv
                                    }
                                },
                                Ok(None) => {
                                    warn!("Got disconnected! Attempting to reconnect in 5 seconds.");
                                    sleep(Duration::from_secs(5)).await;
                                    break;
                                },
                                Err(_) => {
                                    warn!("Timed out! Attempting to reconnect in 5 seconds.");
                                    sleep(Duration::from_secs(5)).await;
                                    break;
                                }
                            }
                        }
                    },
                    Err(e) => {
                        warn!("Failed to connect to websocket: {e}");
                        sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        },
        Command::Host(args) => {
            let ServerArgs { addr, port, file } = args.clone();
            let server = HttpServer::new(move || {
                App::new()
                    .app_data(web::Data::new(args.clone()))
                    .service(api)
            })
                .bind((addr, port))
                .or_else(|e| Err(format!("Failed to bind to address: {e}")))?;

            process::Command::new("mpv")
                .arg(format!("--input-ipc-server={}", shellexpand::tilde("~/.config/watchr/sock")))
                .arg(shellexpand::tilde(file.as_ref()))
                .output()
                .or_else(|e| Err(format!("Failed to execute mpv: {e}")))?;

            spawn(move || async {
                let mut stream = UnixStream::connect(shellexpand::tilde("~/.config/watchr/sock").as_ref())
                    .or_else(|e| Err(format!("Failed to open UNIX socket: {e}")))?;
                let mut writer = BufWriter::new(stream);
                let mut reader = BufReader::new(stream);

                //TODO: probably not necessary
                // writer.write(make_command(json!(["disable_event", "all"])));
                // writer.write(&['\n']);

                writer.write(make_command(json!(["observe_property_string", 1, "pause"])));
                writer.write(&['\n']);

                writer.write(make_command(json!(["observe_property_string", 2, "playback-time"]))); //TODO: this might get weird
                writer.write(&['\n']);

                writer.flush();

                loop {
                    for line in reader.lines() {
                        match line {
                            Ok(string) => match serde_json::from_str::<IpcEvent>(string.as_ref()) {
                                Ok(event) => if (event.event == String::from("property-change")) {
                                    get_clients().update(event.name, event.data).await;
                                },
                                Err(_) => {}
                            },
                            Err(_) => {}
                        }
                    }
                }
            });

            info!("Server configured, running...");
            server.run().await.or_else(|e| Err(format!("{e}")))
        }
    }
}

#[get("/api")]
async fn api(req: HttpRequest, stream: Payload, args: Data<ServerArgs>) -> Result<HttpResponse, Error> {
    info!("Client {} attempting to connect...", req.peer_addr().map_or("<unknown>".to_string(), |addr| addr.ip().to_canonical().to_string()));
    let (res, session, stream) = actix_ws::handle(&req, stream)?;

    get_clients().push(session);
    info!("Client connected!");
    Ok(res)
}

#[get("/media")]
async fn media(req: HttpRequest, args: Data<ServerArgs>) -> impl Responder {
    match NamedFile::open_async(shellexpand::tilde(args.file.as_ref())).await {
        Ok(res) => res,
        Err(_) => HttpResponse::InternalServerError().finish()
    }
}
