//! `lb-editor --game <valve dir> [--install DIR] [--listen 127.0.0.1:8090] [--token T] [--secret S]`: the map
//! editor next to a game server.
//! For a server on another machine run it there and open the page through `ssh -L 8090:127.0.0.1:8090 <server>`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use lb_editor::commands::Commands;
use lb_editor::maps::{Game, Maps};
use lb_editor::nav::NavMaps;
use lb_editor::{AppState, app, guard};

const USAGE: &str = "usage: lb-editor --game <valve dir> [options]
  --game <dir>          the mod directory of the game or server: maps/ and the WADs are read from it
                        (and from <dir>_addon and <dir>_downloads next to it)
  --install <dir>       the bots' directory: overlays, the server's graphs, config (default <game>/addons/lambdabots)
  --listen <addr>       a loopback address and port (default 127.0.0.1:8090)
  --token <token>       the access token (default: a random one; also LB_EDITOR_TOKEN)
  --secret <secret>     the server's command channel secret, when lambdabots.yaml does not have it (as when it is
                        set with lb_telemetry_secret in server.cfg; also LB_TELEMETRY_SECRET)
  --telemetry-port <n>  the server's telemetry port, when not the one in lambdabots.yaml (commands go to it + 1)";

struct Args {
    game: PathBuf,
    install: PathBuf,
    listen: SocketAddr,
    token: Option<String>,
    secret: Option<String>,
    telemetry_port: Option<u16>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut game = None;
    let mut listen: SocketAddr = "127.0.0.1:8090".parse().expect("an address");
    let mut token = std::env::var("LB_EDITOR_TOKEN").ok().filter(|t| !t.is_empty());
    let mut install = None;
    let mut secret = std::env::var("LB_TELEMETRY_SECRET").ok().filter(|s| !s.is_empty());
    let mut telemetry_port = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--game" => game = Some(PathBuf::from(value()?)),
            "--listen" => listen = value()?.parse().map_err(|e| format!("--listen: {e}"))?,
            "--token" => token = Some(value()?),
            "--install" => install = Some(PathBuf::from(value()?)),
            "--secret" => secret = Some(value()?),
            "--telemetry-port" => {
                telemetry_port = Some(value()?.parse().map_err(|e| format!("--telemetry-port: {e}"))?)
            }
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let game = game.ok_or("--game is required")?;
    if !game.join("maps").is_dir() {
        return Err(format!("{} has no maps/ directory", game.display()));
    }
    if !listen.ip().is_loopback() {
        return Err("the editor listens on loopback only: reach a remote one through an SSH tunnel".into());
    }
    let install = install.unwrap_or_else(|| game.join("addons/lambdabots"));
    Ok(Args {
        game,
        install,
        listen,
        token,
        secret,
        telemetry_port,
    })
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&args) {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}\n");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let token = args.token.unwrap_or_else(guard::new_token);
    let commands = Commands::new(&args.install, args.secret.clone(), args.telemetry_port);
    let state = Arc::new(AppState {
        maps: Maps::new(Game::new(&args.game)),
        token: token.clone(),
        install: args.install.clone(),
        nav: NavMaps::default(),
        commands,
    });
    let dirs: Vec<String> = state.maps.game.dirs().iter().map(|d| d.display().to_string()).collect();
    let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
    runtime.block_on(async move {
        let listener = match tokio::net::TcpListener::bind(args.listen).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("cannot listen on {}: {e}", args.listen);
                return ExitCode::FAILURE;
            }
        };
        let addr = listener.local_addr().unwrap_or(args.listen);
        tracing::info!("maps from {}", dirs.join(", "));
        tracing::info!(
            "overlays and the server's graphs in {}; commands: {}",
            state.install.display(),
            state.commands.status().detail
        );
        println!("lb-editor: open http://{addr}/?token={token}");
        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        match axum::serve(listener, app(state)).with_graceful_shutdown(shutdown).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        }
    })
}
