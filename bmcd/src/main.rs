// Copyright 2023 Turing Machines
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use crate::config::Config;
use crate::{
    authentication::linux_authenticator::LinuxAuthenticator, flash_service::FlashService,
    legacy::info_config,
};
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
use openssl::pkey::{PKey, Private};
use openssl::ssl::{SslAcceptor, SslAcceptorBuilder, SslMethod};
use openssl::x509::X509;
use std::fs::OpenOptions;
use std::io::Read;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tpi_rs::app::{bmc_application::BmcApplication, event_application::run_event_listener};
pub mod authentication;
pub mod config;
mod flash_service;
mod into_legacy_response;
mod legacy;

const HTTPS_PORT: u16 = 443;
const HTTP_PORT: u16 = 80;

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    init_logger();

    let config = Config::try_from(config_path()).context("Error parsing config file")?;
    let (tls, tls6) = load_tls_config(&config)?;
    let bmc = Data::new(BmcApplication::new(config.write_timeout).await?);
    bmc.start_serial_workers().await?;

    run_event_listener(bmc.clone().into_inner())?;
    let flash_service = Data::new(FlashService::new());
    let authentication = Arc::new(LinuxAuthenticator::new("/api/bmc/authenticate").await?);

    let run_server = HttpServer::new(move || {
        App::new()
            .app_data(bmc.clone())
            .app_data(flash_service.clone())
            // Enable logger
            .wrap(middleware::Logger::default())
            .wrap(authentication.clone())
            // Legacy API
            .configure(legacy::config)
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
        .with_module_level("h2", LevelFilter::Info)
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

fn load_keys_from_pem<P: AsRef<Path>>(
    private_key: P,
    certificate: P,
) -> anyhow::Result<(PKey<Private>, X509)> {
    let mut pkey = Vec::new();
    let mut cert = Vec::new();
    OpenOptions::new()
        .read(true)
        .open(private_key)?
        .read_to_end(&mut pkey)?;
    OpenOptions::new()
        .read(true)
        .open(certificate)?
        .read_to_end(&mut cert)?;

    let rsa_key = PKey::private_key_from_pem(&pkey)?;
    let x509 = X509::from_pem(&cert)?;
    Ok((rsa_key, x509))
}

fn load_tls_config(config: &Config) -> anyhow::Result<(SslAcceptorBuilder, SslAcceptorBuilder)> {
    let (private_key, cert) = load_keys_from_pem(&config.tls.private_key, &config.tls.certificate)?;
    let mut tls = SslAcceptor::mozilla_intermediate(SslMethod::tls())?;
    tls.set_private_key(&private_key)?;
    tls.set_certificate(&cert)?;
    let mut tls6 = SslAcceptor::mozilla_intermediate(SslMethod::tls())?;
    tls6.set_private_key(&private_key)?;
    tls6.set_certificate(&cert)?;
    Ok((tls, tls6))
}
