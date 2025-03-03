use std::{io::{BufRead, Read, Write}, path::PathBuf, sync::{Mutex, OnceLock}, thread::spawn, time::Duration};

use actix_files::NamedFile;
use actix_web::{rt::time::timeout, web::{self, Data, Payload}, App, Error, HttpRequest, HttpResponse, HttpServer, Responder};
use actix_ws::Session;
use awc::Client;
use awc::ws::Frame::Binary;
use clap::{Parser, Subcommand};
use env_logger::Target;
use futures::StreamExt;
use log::{error, info, warn, LevelFilter};
use serde::Deserialize;
use tokio::{io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader}, time::sleep};
use serde_json::json;
use util::start_mpv;

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
        response.append(&mut property.as_bytes().to_vec());
        response.push(0u8);
        response.append(&mut value.as_bytes().to_vec());

        let mut sessions = self.connections.lock().unwrap();
        let mut do_retain = Vec::new();

        for session in sessions.iter_mut().rev() {
            match session.binary(response.clone()).await {
                Ok(_) => {
                    do_retain.push(true);
                },
                Err(_) => {
                    info!("A client has disconnected");
                    do_retain.push(false);
                }
            }
        }

        sessions.retain_mut(|session| do_retain.pop().unwrap());

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
        Err(message) => error!("Failure: {message}"),
        _ => ()
    }
}

async fn run() -> Result<(), String> {
    std::fs::create_dir_all(shellexpand::tilde("~/.config/watchr").as_ref());

    match Cli::parse().command {
        Command::Connect(args) => {
            let client = Client::default();

            loop {
                match client.ws(format!("ws://{}:{}/api", args.addr, args.port)).connect().await {
                    Ok((res, mut ws)) => {
                        info!("Connected! HTTP response: {res:?}");

                        //TODO: remove unwrap
                        let mut stream = reqwest::get(format!("http://{}:{}/media.mkv", args.addr, args.port)).await.unwrap().bytes_stream();

                        spawn(move || {
                            let rt = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .unwrap(); //TODO: is this unwrap bad?
                            let mut writer = rt.block_on(tokio::fs::OpenOptions::new().write(true).create(true).open(shellexpand::tilde("~/.config/watchr/media.mkv").to_string())).unwrap();

                            while let Some(item) = rt.block_on(stream.next()) {
                                match item {
                                    Ok(bytes) => {
                                        rt.block_on(tokio::io::copy(&mut bytes.as_ref(), &mut writer));
                                    },
                                    Err(_) => {}
                                }
                            }
                        });

                        sleep(Duration::from_secs(5)).await;

                        let mut socket = start_mpv("~/.config/watchr/media.mkv").await?;
                        let (reader, writer) = &mut socket.split();

                        loop {
                            match timeout(Duration::from_secs(20), ws.next()).await {
                                Ok(Some(msg)) => {
                                    if let Ok(Binary(msg)) = msg {
                                        let mut property = Vec::from(msg);
                                        //TODO: the question mark on this line will cause a crash
                                        //on the slightest malformed message from the host
                                        //TODO: also stop unwrapping!
                                        let value = property.split_off(property.binary_search(&0u8).map_err(|e| format!("Received message is not valid: {e}"))? + 1);
                                        writer.write(make_command(json!(["set_property_string", String::from_utf8(property).unwrap(), String::from_utf8(value).unwrap()])).as_bytes()).await;
                                        writer.write(&[b'\n']).await;
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
                    .service(media)
                    .service(api)
            })
                .bind((addr, port))
                .or_else(|e| Err(format!("Failed to bind to address: {e}")))?;

            spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap(); //TODO: is this unwrap bad?
                //TODO: remove unwrap
                let mut socket = rt.block_on(start_mpv(&file)).unwrap();
                let (reader, writer) = &mut socket.split();

                //TODO: probably not necessary
                // writer.write(make_command(json!(["disable_event", "all"])).as_bytes());
                // writer.write(&[b'\n']);

                rt.block_on(writer.write(make_command(json!(["observe_property_string", 1, "pause"])).as_bytes()));
                rt.block_on(writer.write(&[b'\n']));

                rt.block_on(writer.write(make_command(json!(["observe_property_string", 2, "playback-time"])).as_bytes())); //TODO: this might get weird
                rt.block_on(writer.write(&[b'\n']));

                rt.block_on(writer.flush());

                let mut reader = BufReader::new(reader);
                loop {
                    let mut line = "".to_string();
                    rt.block_on(reader.read_line(&mut line));

                    match serde_json::from_str::<IpcEvent>(&line) {
                        Ok(event) => if event.event == String::from("property-change") {
                            //TODO: bad block_on?
                            rt.block_on(get_clients().update(event.name, event.data));
                        },
                        Err(_) => {}
                    }
                }
            });

            info!("Server configured, running...");
            server.run().await.or_else(|e| Err(format!("{e}")))
        }
    }
}

#[actix_web::get("/api")]
async fn api(req: HttpRequest, stream: Payload, args: Data<ServerArgs>) -> Result<HttpResponse, Error> {
    info!("Client {} attempting to connect...", req.peer_addr().map_or("<unknown>".to_string(), |addr| addr.ip().to_canonical().to_string()));
    let (res, session, stream) = actix_ws::handle(&req, stream)?;

    get_clients().push(session);
    info!("Client connected!");
    Ok(res)
}

#[actix_web::get("/media.mkv")]
async fn media(req: HttpRequest, args: Data<ServerArgs>) -> impl Responder {
    match NamedFile::open_async(shellexpand::tilde(&args.file).as_ref()).await {
        Ok(res) => res.respond_to(&req),
        Err(_) => HttpResponse::InternalServerError().finish()
    }
}
