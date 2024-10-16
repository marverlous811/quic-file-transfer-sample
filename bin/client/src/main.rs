use std::{
    net::SocketAddr,
    sync::Arc,
    thread::sleep,
    time::{Duration, Instant},
};

use clap::Parser;
use custom_utils::read_file_util;
use quinn::{ClientConfig, Endpoint, TransportConfig};
use rustls::pki_types::CertificateDer;
use tokio::{fs::File, io::AsyncReadExt};
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

    #[arg(env, long, default_value = "quic.localhost")]
    server_name: String,

    /// Bind address
    #[arg(env, long)]
    bind_addr: SocketAddr,

    /// Server Address
    #[arg(env, long, default_value = "127.0.0.1:8080")]
    server_addr: SocketAddr,

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

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let server_cert: CertificateDer = read_file_util(&args.server_cert).await;

    let endpoint = create_endpoint(&args.bind_addr, server_cert);
    match endpoint {
        Result::Ok(ep) => {
            let _ =
                run(ep, args.server_addr, args.server_name, args.transfer_file)
                    .await;
        }
        Err(err) => {
            log::error!("error when create quic endpoint {:?}", err);
        }
    }
}

async fn run(
    endpoint: Endpoint,
    server_addr: SocketAddr,
    server_name: String,
    file_to_transfer: String,
) -> anyhow::Result<()> {
    let connecting = endpoint.connect(server_addr, &server_name);
    if let Ok(connecting) = connecting {
        let connection = connecting.await.expect("connection failed");
        log::info!("connected to {:?}", connection.remote_address());

        log::info!("[QuicServer] wait for accept...");
        let mut send =
            connection.open_uni().await.expect("failed to open stream");
        log::info!("opened stream to server");

        if let Ok(mut file) = File::open(&file_to_transfer).await {
            let file_len = file.metadata().await.unwrap().len();
            log::info!("start transfering file of size {}", file_len);

            let start = Instant::now();
            let mut buf = vec![0u8; MTU_SIZE];
            let mut transported_len = 0;
            loop {
                let len = file.read(&mut buf).await.unwrap();
                if len == 0 {
                    break;
                }

                send.write_all(&buf[..len]).await.unwrap();
                transported_len += len;
                log::info!("tranfered {transported_len}/{file_len} to server");
                sleep(Duration::from_micros(100));
            }

            let end = start.elapsed();
            let _ = send.finish();
            log::info!(
                "Finished transfering in {:?}, average speed: {} MiB/s",
                end,
                (file_len as f64 / (1024.0 * 1024.0)) / end.as_secs_f64()
            );
        }
    }
    Ok(())
}

fn create_endpoint(
    bind_addr: &SocketAddr,
    cert: CertificateDer<'static>,
) -> anyhow::Result<quinn::Endpoint> {
    let client_config = configure_client(&[cert])?;
    let mut endpoint = Endpoint::client(bind_addr.clone())?;
    endpoint.set_default_client_config(client_config);
    Ok(endpoint)
}

fn configure_client(
    server_certs: &[CertificateDer],
) -> anyhow::Result<ClientConfig> {
    let mut certs = rustls::RootCertStore::empty();
    for cert in server_certs {
        certs.add(cert.clone())?;
    }
    let mut config = ClientConfig::with_root_certificates(Arc::new(certs))?;

    let mut transport = TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(1)));
    transport.max_idle_timeout(Some(
        Duration::from_secs(5)
            .try_into()
            .expect("Should config timeout"),
    ));
    config.transport_config(Arc::new(transport));

    Ok(config)
}
