use actix_files::Files;
use actix_web::{middleware, web::Data, App, HttpServer};
use log::LevelFilter;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tpi_rs::app::bmc_application::BmcApplication;

mod legacy;

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    init_logger();

    let cancellation_token = CancellationToken::new();
    let bmc = Mutex::new(BmcApplication::new(cancellation_token).await?);
    let bmc_shared = Data::new(bmc);

    HttpServer::new(move || {
        App::new()
            // Shared state: mutex of BmcApplication instance
            .app_data(bmc_shared.clone())
            // Legacy API
            .configure(legacy::config)
            // Enable logger
            .wrap(middleware::Logger::default())
            // Serve a static tree of files of the web UI. Must be the last item.
            .service(Files::new("/", "/mnt/var/www/").index_file("index.html"))
    })
    .bind(("0.0.0.0", 8080))?
    .workers(2)
    .run()
    .await?;

    log::info!("shutting down {}", env!("CARGO_PKG_NAME"));
    Ok(())
}

fn init_logger() {
    let level = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Warn
    };

    simple_logger::SimpleLogger::new()
        .with_level(level)
        .with_module_level("bmcd", LevelFilter::Info)
        .with_colors(true)
        .env()
        .init()
        .expect("failed to initialize logger");

    log::info!("Turing Pi 2 BMC Daemon v{}", env!("CARGO_PKG_VERSION"));
}
