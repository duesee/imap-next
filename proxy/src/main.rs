mod config;
mod proxy;
mod util;

use anyhow::{Context, Result};
use config::{Config, Service};
use proxy::{ClientAcceptedState, Proxy};
use tokio::task::JoinSet;
use tracing::{error, instrument, Instrument};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .without_time()
        .init();

    // Load config file
    let config = {
        let config_path = std::env::args().nth(1).unwrap_or("config.toml".to_string());

        Config::load(&config_path)
            .with_context(|| format!("Failed to load config from '{config_path}'"))?
    };

    // Start proxy services
    let mut set = JoinSet::new();
    for service in config.services {
        println!("# {}", service.name);
        println!("{} -> {}\n", service.bind, service.connect);

        set.spawn(handle_service(service));
    }

    // Terminate once all services has stopped
    while let Some(res) = set.join_next().await {
        if let Err(error) = res {
            error!(?error, "Failed to join with service task");
        }
    }
    Ok(())
}

#[instrument(name = "service", skip_all, fields(name = service.name))]
async fn handle_service(service: Service) {
    // Bind to port
    let proxy = match Proxy::bind(service.clone()).await {
        Ok(proxy) => proxy,
        Err(error) => {
            error!(?error, "Failed to start service");
            return ();
        }
    };

    loop {
        // Wait for client
        let proxy = match proxy.accept_client().await {
            Ok(result) => result,
            Err(error) => {
                error!(?error, "Failed to accept client");
                continue;
            }
        };

        // Handle client
        tokio::spawn(
            async {
                if let Err(error) = handle_client(proxy).await {
                    error!(?error, "Connection finished unexpectedly");
                }
            }
            .in_current_span(),
        );
    }
}

#[instrument(name = "client", skip_all, fields(addr = %proxy.client_addr()))]
async fn handle_client(proxy: Proxy<ClientAcceptedState>) -> Result<()> {
    let proxy = proxy.connect_to_server().await?;
    proxy.start_conversion().await;
    Ok(())
}
