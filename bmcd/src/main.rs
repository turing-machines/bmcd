use crate::{flash_service::FlashService, legacy::info_config};
use actix_files::Files;
use actix_web::{
    http::{self, KeepAlive},
    middleware, web,
    web::Data,
    App, HttpRequest, HttpResponse, HttpServer,
};
use anyhow::Context;
use clap::{command, value_parser, Arg};
use log::LevelFilter;
use openssl::ssl::SslAcceptorBuilder;
use std::path::{Path, PathBuf};
use tpi_rs::app::{bmc_application::BmcApplication, event_application::run_event_listener};
pub mod config;
mod flash_service;
mod into_legacy_response;
mod legacy;
use crate::config::Config;
use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod};

const HTTPS_PORT: u16 = 443;
const HTTP_PORT: u16 = 80;

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    init_logger();

    let config = Config::try_from(config_path()).context("Error parsing config file")?;
    let tls = load_tls_configuration(&config.tls.private_key, &config.tls.certificate)?;
    let tls6 = load_tls_configuration(&config.tls.private_key, &config.tls.certificate)?;

    let bmc = Data::new(BmcApplication::new().await?);
    run_event_listener(bmc.clone().into_inner())?;
    let flash_service = Data::new(FlashService::new());

    let run_server = HttpServer::new(move || {
        App::new()
            // Shared state: BmcApplication instance
            .app_data(bmc.clone())
            .app_data(flash_service.clone())
            // Legacy API
            .configure(legacy::config)
            // Enable logger
            .wrap(middleware::Logger::default())
            // Serve a static tree of files of the web UI. Must be the last item.
            .service(Files::new("/", "/mnt/var/www/").index_file("index.html"))
    })
    .bind_openssl(("0.0.0.0", HTTPS_PORT), tls)?
    .bind_openssl(("::1", HTTPS_PORT), tls6)?
    .keep_alive(KeepAlive::Os)
    .workers(2)
    .run();

    // redirect requests to 'HTTPS'
    let redirect_server = HttpServer::new(move || {
        App::new()
            .configure(info_config)
            .default_service(web::route().to(redirect))
    })
    .bind(("0.0.0.0", HTTP_PORT))?
    .bind(("::1", HTTP_PORT))?
    .run();

    tokio::try_join!(run_server, redirect_server)?;
    log::info!("exiting {}", env!("CARGO_PKG_NAME"));
    Ok(())
}

async fn redirect(request: HttpRequest) -> HttpResponse {
    let host = request.connection_info().host().to_string();
    let path = request.uri().to_string();
    let redirect_url = format!("https://{}{}", host, path);
    HttpResponse::PermanentRedirect()
        .append_header((http::header::LOCATION, redirect_url))
        .finish()
}

fn init_logger() {
    let level = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Warn
    };

    simple_logger::SimpleLogger::new()
        .with_level(level)
        .with_module_level("bmcd", LevelFilter::Debug)
        .with_module_level("actix_http", LevelFilter::Info)
        .with_module_level("h2::codec", LevelFilter::Info)
        .with_module_level("h2::proto", LevelFilter::Info)
        .with_colors(true)
        .env()
        .init()
        .expect("failed to initialize logger");

    log::info!("Turing Pi 2 BMC Daemon v{}", env!("CARGO_PKG_VERSION"));
}

fn config_path() -> PathBuf {
    command!()
        .arg(
            Arg::new("config")
                .long("config")
                .value_parser(value_parser!(PathBuf))
                .required(true),
        )
        .get_matches()
        .get_one::<PathBuf>("config")
        .expect("`config` argument required")
        .into()
}

fn load_tls_configuration<P: AsRef<Path>>(
    private_key: P,
    certificate: P,
) -> anyhow::Result<SslAcceptorBuilder> {
    let mut builder = SslAcceptor::mozilla_intermediate(SslMethod::tls())?;
    builder.set_private_key_file(private_key.as_ref(), SslFiletype::PEM)?;
    builder.set_certificate_chain_file(certificate.as_ref())?;
    Ok(builder)
}
