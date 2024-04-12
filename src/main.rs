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
#![deny(clippy::mod_module_files)]
mod api;
mod app;
mod authentication;
mod config;
mod hal;
mod persistency;
mod serial_service;
mod streaming_data_service;
mod usb_boot;
mod utils;

use crate::config::Config;
use crate::serial_service::serial::SerialConnections;
use crate::serial_service::serial_config;
use crate::{
    api::legacy, api::legacy::info_config, authentication::linux_authenticator::LinuxAuthenticator,
    streaming_data_service::StreamingDataService,
};
use actix_files::Files;
use actix_web::http::KeepAlive;
use actix_web::{
    http::{self},
    web,
    web::Data,
    App, HttpRequest, HttpResponse, HttpServer,
};
use anyhow::Context;
use app::{bmc_application::BmcApplication, event_application::run_event_listener};
use clap::{command, value_parser, Arg};
use futures::future::join_all;
use openssl::pkey::{PKey, Private};
use openssl::ssl::{SslAcceptor, SslAcceptorBuilder, SslMethod};
use openssl::x509::X509;
use std::fs::OpenOptions;
use std::io::Read;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::level_filters::LevelFilter;
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::Rotation;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

const HTTP_PORT: u16 = 80;

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    let _logger_lifetime = init_logger();

    let config = Config::try_from(config_path()).context("Error parsing config file")?;
    let tls = load_tls_config(&config)?;
    let bmc = Data::new(BmcApplication::new(config.store.write_timeout).await?);
    let serial_service = Data::new(SerialConnections::new());
    let streaming_data_service = Data::new(StreamingDataService::new());
    let authentication = Arc::new(
        LinuxAuthenticator::new(
            "/api/bmc/authenticate",
            "Access to Baseboard Management Controller",
            config.authentication.token_expires,
            config.authentication.authentication_attempts,
        )
        .await?,
    );

    run_event_listener(bmc.clone().into_inner())?;

    let run_server = HttpServer::new(move || {
        App::new()
            .wrap(authentication.clone())
            .service(
                web::scope("/api/bmc")
                    .app_data(bmc.clone())
                    .app_data(streaming_data_service.clone())
                    .app_data(serial_service.clone())
                    .configure(serial_config)
                    // Legacy API
                    .configure(legacy::config),
            )
            // Serve a static tree of files of the web UI. Must be the last item.
            .service(Files::new("/", &config.www).index_file("index.html"))
    })
    .bind_openssl((config.host.clone(), config.port), tls)?
    .keep_alive(KeepAlive::Os)
    .workers(2)
    .run();

    let mut futures = vec![run_server];
    if config.redirect_http {
        // redirect requests to 'HTTPS'
        futures.push(
            HttpServer::new(move || {
                App::new()
                    .app_data(Data::new(config.port))
                    .configure(info_config)
                    .default_service(web::route().to(redirect))
            })
            .bind((config.host, HTTP_PORT))?
            .run(),
        );
    }

    // run server(s)
    join_all(futures).await;
    tracing::info!("exiting {}", env!("CARGO_PKG_NAME"));
    Ok(())
}

async fn redirect(request: HttpRequest, port: web::Data<u16>) -> HttpResponse {
    let host = request.connection_info().host().to_string();
    let path = request.uri().to_string();
    let redirect_url = format!("https://{}:{}{}", host, port.get_ref(), path);
    HttpResponse::PermanentRedirect()
        .append_header((http::header::LOCATION, redirect_url))
        .finish()
}

fn init_logger() -> WorkerGuard {
    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(Rotation::HOURLY)
        .max_log_files(3)
        .filename_prefix("bmcd.log")
        .build(std::env::temp_dir())
        .expect("error setting up log rotation");

    let (bmcd_log, guard) = tracing_appender::non_blocking(file_appender);
    let full_layer = tracing_subscriber::fmt::layer().with_writer(bmcd_log);

    let targets = Targets::default()
        .with_target("actix_server", LevelFilter::OFF)
        .with_default(Level::INFO);

    let stdout_layer = tracing_subscriber::fmt::layer()
        .without_time()
        .with_writer(std::io::stdout)
        .compact();

    let layers = full_layer.and_then(stdout_layer).with_filter(targets);

    tracing_subscriber::registry().with(layers).init();

    tracing::info!("Turing Pi 2 BMC Daemon v{}", env!("CARGO_PKG_VERSION"));
    guard
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
        .open(private_key)
        .context("could not open private key file")?
        .read_to_end(&mut pkey)?;
    OpenOptions::new()
        .read(true)
        .open(certificate)
        .context("could not open cert file")?
        .read_to_end(&mut cert)?;

    let rsa_key = PKey::private_key_from_pem(&pkey)?;
    let x509 = X509::from_pem(&cert)?;
    Ok((rsa_key, x509))
}

fn load_tls_config(config: &Config) -> anyhow::Result<SslAcceptorBuilder> {
    let (private_key, cert) = load_keys_from_pem(&config.tls.private_key, &config.tls.certificate)?;
    let mut tls = SslAcceptor::mozilla_intermediate(SslMethod::tls())?;
    tls.set_private_key(&private_key)?;
    tls.set_certificate(&cert)?;
    Ok(tls)
}
