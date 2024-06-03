use std::{future::IntoFuture, path::PathBuf};

use axum::routing::get;
use axum_prometheus::PrometheusMetricLayerBuilder;
use base64::{engine::general_purpose, Engine as _};
use certifier::certifier::RouterLimiter;
use clap::{arg, Parser, Subcommand};
use ed25519_dalek::SigningKey;
use tokio::net::TcpListener;
use tracing::info;
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, FmtSubscriber};

#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[arg(
        short,
        long,
        default_value = "config.yml",
        env("CERTIFIER_CONFIG_PATH")
    )]
    config: PathBuf,

    #[command(subcommand)]
    cmd: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// generate keypair and write it to standard out.
    /// the keypair is encoded as json
    GenerateKeys,
}

fn generate_keys() -> Result<(), Box<dyn std::error::Error>> {
    let signing_key: SigningKey = SigningKey::generate(&mut rand::rngs::OsRng);

    #[serde_with::serde_as]
    #[derive(serde::Serialize)]
    struct KeyPair {
        #[serde_as(as = "serde_with::base64::Base64")]
        public_key: [u8; ed25519_dalek::PUBLIC_KEY_LENGTH],
        #[serde_as(as = "serde_with::base64::Base64")]
        secret_key: [u8; ed25519_dalek::SECRET_KEY_LENGTH],
    }

    let keypair = KeyPair {
        public_key: signing_key.verifying_key().to_bytes(),
        secret_key: signing_key.to_bytes(),
    };

    serde_json::to_writer_pretty(std::io::stdout(), &keypair)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();

    if let Some(Commands::GenerateKeys) = args.cmd {
        return generate_keys();
    }

    LogTracer::init()?;
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("INFO"));
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(env_filter)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let config = certifier::configuration::get_configuration(&args.config)?;
    let signer = SigningKey::from_bytes(&config.signing_key);
    let pubkey_b64 = general_purpose::STANDARD.encode(signer.verifying_key().as_bytes());

    info!("listening on: {:?}, pubkey: {}", config.listen, pubkey_b64,);
    info!("POST proof configuration: {:?}", config.post_cfg);
    info!("POST init configuration: {:?}", config.init_cfg);
    info!("RandomX mode: {:?}", config.randomx_mode);
    info!("{:?}", config.limits);
    if let Some(expiry) = config.certificate_expiration {
        info!("generated certificates will expire after {expiry:?}");
    } else {
        info!("generated certificates won't expire");
    }

    let mut app = certifier::certifier::new(
        config.post_cfg,
        config.init_cfg,
        signer,
        config.randomx_mode,
        config.certificate_expiration,
    )
    .apply_limits(config.limits);

    if let Some(addr) = config.metrics {
        info!("metrics enabled on: http://{addr:?}/metrics");
        let (layer, handle) = PrometheusMetricLayerBuilder::new()
            .with_prefix("certifier")
            .with_default_metrics()
            .build_pair();

        app = app.layer(layer);
        let metrics = axum::Router::new().route("/metrics", get(|| async move { handle.render() }));
        let listener = TcpListener::bind(addr).await?;
        tokio::spawn(axum::serve(listener, metrics.into_make_service()).into_future());
    }

    let listener = TcpListener::bind(config.listen).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}
