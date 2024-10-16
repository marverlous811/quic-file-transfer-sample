use std::process;
use std::time::Instant;
use std::{net::SocketAddr, str::FromStr};

use clap::Parser;
use custom_utils::read_file_util;
use p2p::{
    P2pNetwork, P2pNetworkConfig, P2pQuicStream, P2pServiceEvent,
    P2pServiceRequester, PeerAddress, PeerId, SharedKeyHandshake,
};
use reedline_repl_rs::clap::{
    Arg as ReplArg, ArgMatches as ReplArgMatches, Command as ReplCommand,
};
use reedline_repl_rs::{Error, Repl};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::select;
use tracing_subscriber::{
    fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

const MTU_SIZE: usize = 1500;
pub const FORWARDER_SERVICE: u16 = 179;

pub enum ControllerCommand {
    Exit,
}

#[derive(Clone)]
pub struct ForwardCtx {
    pub requester: P2pServiceRequester,
    pub tx: tokio::sync::mpsc::Sender<ControllerCommand>,
    pub transfer_file: String,
}

fn send_to(
    args: ReplArgMatches,
    context: &mut ForwardCtx,
) -> Result<Option<String>, Error> {
    let node_id_str = args.get_one::<String>("node_id").unwrap();
    let node_id = node_id_str.parse::<u64>().unwrap();
    let msg = args.get_one::<String>("msg").unwrap();

    let peer_id = PeerId::from(node_id);
    let msg = msg.clone();
    let requester = context.requester.clone();

    tokio::spawn(async move {
        let _ = requester
            .send_unicast(peer_id, msg.as_bytes().to_vec())
            .await;
    });

    Ok(Some("ok".to_string()))
}

async fn exit(
    _: ReplArgMatches,
    context: &mut ForwardCtx,
) -> Result<Option<String>, Error> {
    let _ = context.tx.send(ControllerCommand::Exit).await;
    Ok(None)
}

fn transfer(
    args: ReplArgMatches,
    ctx: &mut ForwardCtx,
) -> Result<Option<String>, Error> {
    let node_id_str = args.get_one::<String>("node_id").unwrap();
    let node_id = node_id_str.parse::<u64>().unwrap();
    let requester = ctx.requester.clone();
    let file_to_transfer = ctx.transfer_file.clone();

    tokio::spawn(async move {
        let peer_id = PeerId::from(node_id);
        let stream =
            requester.open_stream(peer_id, "".as_bytes().to_vec()).await;

        if let Ok(mut stream) = stream {
            log::info!("opened stream to server");

            if let Ok(mut file) = File::open(&file_to_transfer).await {
                let file_len = file.metadata().await.unwrap().len();
                let start = Instant::now();
                let mut buf = vec![0u8; MTU_SIZE];
                let mut transported_len = 0;
                loop {
                    let len = file.read(&mut buf).await.unwrap();
                    if len == 0 {
                        break;
                    }

                    // let _ = requester
                    //     .send_unicast(peer_id, buf[..len].to_vec())
                    //     .await;
                    stream.write_all(&buf[..len]).await.unwrap();
                    transported_len += len;
                    log::info!(
                        "tranfered {transported_len}/{file_len} to server"
                    );
                }

                let end = start.elapsed();
                log::info!(
                    "Finished transfering in {:?}, average speed: {} MiB/s",
                    end,
                    (file_len as f64 / (1024.0 * 1024.0)) / end.as_secs_f64()
                );
            }
        }
    });
    Ok(Some("ok".to_string()))
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// UDP/TCP port for serving QUIC/TCP connection for SDN network
    #[arg(env, long)]
    sdn_peer_id: u64,

    /// UDP/TCP port for serving QUIC/TCP connection for SDN network
    #[arg(env, long, default_value = "0.0.0.0:11111")]
    sdn_listener: SocketAddr,

    /// Seeds
    #[arg(env, long, value_delimiter = ',')]
    sdn_seeds: Vec<String>,

    /// Allow it broadcast address to other peers
    /// This allows other peer can active connect to this node
    /// This option is useful with high performance relay node
    #[arg(env, long)]
    sdn_advertise_address: Option<SocketAddr>,

    /// Sdn secure code
    #[arg(env, long, default_value = "insecure")]
    sdn_secure_code: String,

    /// Server cert path
    #[arg(env, long, default_value = "../../dev-certs/cluster.cert")]
    server_cert: String,

    /// Server key path
    #[arg(env, long, default_value = "../../dev-certs/cluster.key")]
    server_key: String,

    #[arg(env, long)]
    transfer_file: String,
}

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("should install ring as default");

    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info");
    }
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }
    let args: Args = Args::parse();
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let server_cert: CertificateDer = read_file_util(&args.server_cert).await;
    let server_key: PrivatePkcs8KeyDer = read_file_util(&args.server_key).await;

    let mut p2p = P2pNetwork::new(P2pNetworkConfig {
        peer_id: args.sdn_peer_id.into(),
        listen_addr: args.sdn_listener,
        advertise: args.sdn_advertise_address.map(|a| a.into()),
        priv_key: server_key,
        cert: server_cert,
        tick_ms: 100,
        seeds: args
            .sdn_seeds
            .into_iter()
            .map(|s| {
                PeerAddress::from_str(s.as_str()).expect("should parse address")
            })
            .collect::<Vec<_>>(),
        secure: SharedKeyHandshake::from(args.sdn_secure_code.as_str()),
    })
    .await
    .expect("should create network");

    let (tx, mut rx) = tokio::sync::mpsc::channel::<ControllerCommand>(1);
    let mut forwarder_sv = p2p.create_service(FORWARDER_SERVICE.into());
    let forwarder_requester = forwarder_sv.requester();
    let ctx = ForwardCtx {
        transfer_file: args.transfer_file,
        requester: forwarder_requester,
        tx,
    };

    tokio::spawn(async move {
        loop {
            select! {
                _ = p2p.recv() => {
                    continue;
                },
                ev = forwarder_sv.recv() => match ev.expect("sdn crash") {
                    P2pServiceEvent::Unicast(from, data) => {
                        log::info!("[Forwarder] receive unicast from {} => {:?}", from, &data.len());
                    }
                    P2pServiceEvent::Broadcast(from, data) => {
                        log::info!("[Forwarder] receive boardcast from {} => {:?}", from, &data.len());
                    }
                    P2pServiceEvent::Stream(_from, _meta, stream) => {
                        tokio::spawn(receive_data(stream));
                    }
                },
                command = rx.recv() => match command {
                    Some(ControllerCommand::Exit) => {
                        break;
                    }
                    None => {
                        break;
                    }
                }
            }
        }

        process::exit(0);
    });

    let mut repl = Repl::new(ctx)
        .with_name("Sample forwarder")
        .with_command(
            ReplCommand::new("echo")
                .arg(ReplArg::new("node_id").required(true))
                .arg(ReplArg::new("msg").required(true))
                .about("send echo msg to other node"),
            send_to,
        )
        .with_command(
            ReplCommand::new("transfer")
                .arg(ReplArg::new("node_id").required(true))
                .about("send file to other node"),
            transfer,
        )
        .with_command_async(
            ReplCommand::new("exit").about("exit the program"),
            |args, context| Box::pin(exit(args, context)),
        );

    let _ = repl.run_async().await;
}

pub async fn receive_data(mut stream: P2pQuicStream) {
    let mut recv_len = 0;
    loop {
        let mut buf = vec![0u8; MTU_SIZE];
        let len = stream.read(&mut buf).await.unwrap();
        if len == 0 {
            break;
        }

        recv_len += len;
        log::info!("received chunk with size {}", len);
    }

    log::info!("received {recv_len} bytes");
}
