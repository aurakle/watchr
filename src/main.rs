use std::{fmt::Binary, fs, io::{BufRead, BufReader, BufWriter, Write}, os::unix::net::UnixStream, process, thread::{self, sleep, spawn}, time::Duration};

use actix_web::{rt::time::timeout, web::{self, Data, Payload}, App, Error, HttpRequest, HttpResponse, HttpServer, Responder};
use awc::Client;
use clap::{command, Parser, Subcommand};
use env_logger::Target;
use log::{error, info, warn, LevelFilter};
use serde::Serialize;

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

//TODO: what
// #[derive(Debug, Serialize)]
// struct IpcCommand<T: Serialize> {
//     command: T,
// }

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

                        loop {
                            match timeout(Duration::from_secs(20), ws.next()).await {
                                Ok(Some(msg)) => {
                                    //TODO: what (make this actually behave properly)
                                    // if let Ok(Binary(msg)) = msg {
                                    //     if msg[0] == 0xff {
                                    //         info!("Ping!");
                                    //     } else {
                                    //         info!("Ba-bump");
                                    //     }
                                    // }
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

            spawn(move || {
                let mut stream = UnixStream::connect(shellexpand::tilde("~/.config/watchr/sock").as_ref())
                    .or_else(|e| Err(format!("Failed to open UNIX socket: {e}")))?;
                let mut writer = BufWriter::new(stream);
                let mut reader = BufReader::new(stream);

                writer.write(serde_json::to_vec(/*TODO: IpcCommand?*/));
                writer.write(&['\n']);

                loop {
                    for line in reader.lines() {
                        //TODO: handle events given by mpv
                    }
                }
            });

            info!("Server configured, running...");
            server.run().await.or_else(|e| Err(format!("{e}")))
        }
    }
}

#[get("/api")]
pub async fn api(req: HttpRequest, stream: Payload, args: Data<ServerArgs>) -> Result<HttpResponse, Error> {
    info!("Client {} attempting to connect...", req.peer_addr().map_or("<unknown>".to_string(), |addr| addr.ip().to_canonical().to_string()));
    let (res, session, stream) = actix_ws::handle(&req, stream)?;

    //TODO: add client to the connected clients in a fashion similar to connectr

    info!("Client connected!");
    Ok(res)
}

#[get("/media")]
pub async fn media(req: HttpRequest, args: Data<ServerArgs>) -> impl Responder {
    //TODO: deliver the current media
}
