use actix_files::Files;
use actix_web::{middleware, web::Data, App, HttpServer};
use log::LevelFilter;
use tokio::sync::Mutex;
use tpi_rs::app::bmc_application::BmcApplication;

mod legacy;

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    init_logger();

    let bmc = Mutex::new(BmcApplication::new().await?);
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
