use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Ok;
use clap::Parser;
use custom_utils::read_file_util;
use quinn::{Endpoint, Incoming, RecvStream, ServerConfig, VarInt};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::select;
use tracing_subscriber::{
    fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

const MTU_SIZE: usize = 1500;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Server cert path
    #[arg(env, long, default_value = "../../dev-certs/local.cert")]
    server_cert: String,

    /// Server key path
    #[arg(env, long, default_value = "../../dev-certs/local.key")]
    server_key: String,

    /// Bind address
    #[arg(env, long)]
    bind_addr: SocketAddr,
}

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("should install ring as default");

    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info");
    }

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let server_cert: CertificateDer = read_file_util(&args.server_cert).await;
    let server_key: PrivatePkcs8KeyDer = read_file_util(&args.server_key).await;

    let endpoint = create_endpoint(&args.bind_addr, server_key, server_cert);
    match endpoint {
        Result::Ok(ep) => {
            run(ep).await;
        }
        Err(err) => {
            log::error!("error when create quic endpoint {:?}", err);
        }
    }
}

async fn run(endpoint: Endpoint) {
    loop {
        select! {
            connecting = endpoint.accept() => {
                if let Some(incoming) = connecting {
                    let remote = incoming.remote_address();
                    log::info!("[QuicServer] incoming connect from {remote} => accept");
                    tokio::spawn(handle_connection(incoming));
                }
            }
            else => {
                break;
            }
        }
    }

    endpoint.close(VarInt::from_u32(0), "Shutdown".as_bytes());
}

async fn handle_connection(incoming: Incoming) {
    let connection = incoming.await.expect("connection failed");
    let remote = connection.remote_address();
    log::info!("[QuicServer] connected to {remote}");

    let mut interval = tokio::time::interval(Duration::from_millis(10));
    loop {
        select! {
            _ = interval.tick() => {
                log::debug!("[QuicServer] connection {remote} is alive");
            },
            e = connection.accept_uni() => {
                match e {
                    Result::Err(err) => {
                        log::error!("[QuicServer] error when accept stream: {:?}", err);
                        break;
                    }
                    Result::Ok(stream) => {
                        log::info!("[QuicServer] incoming stream from {remote} => handle transfer");
                        let job = handle_transfer(stream, remote);
                        tokio::spawn(job);
                    }
                }
            },
            else => {
                break;
            }
        }
    }
}

async fn handle_transfer(mut recv: RecvStream, remote: SocketAddr) {
    let mut recv_len = 0;

    while let Result::Ok(chunk) = recv.read_chunk(MTU_SIZE, true).await {
        if chunk.is_none() {
            break;
        }

        let chunk = chunk.expect("failed to read chunk");
        recv_len += chunk.bytes.len();

        log::info!("received chunk with size {}", chunk.bytes.len());
    }

    log::info!("[QuicServer] received {recv_len} bytes from {remote}");
}

fn create_endpoint(
    bind_addr: &SocketAddr,
    priv_key: PrivatePkcs8KeyDer<'static>,
    cert: CertificateDer<'static>,
) -> anyhow::Result<Endpoint> {
    let server_config = configure_server(priv_key, cert.clone())?;
    let endpoint = Endpoint::server(server_config, bind_addr.clone())?;
    Ok(endpoint)
}

fn configure_server(
    priv_key: PrivatePkcs8KeyDer<'static>,
    cert: CertificateDer<'static>,
) -> anyhow::Result<ServerConfig> {
    let cert_chain = vec![cert];

    let mut server_config =
        ServerConfig::with_single_cert(cert_chain, priv_key.into())?;
    let transport_config =
        Arc::get_mut(&mut server_config.transport).expect("should get mut");
    transport_config.max_concurrent_uni_streams(10_u8.into());
    transport_config.max_idle_timeout(Some(
        Duration::from_secs(5)
            .try_into()
            .expect("Should config timeout"),
    ));

    Ok(server_config)
}
