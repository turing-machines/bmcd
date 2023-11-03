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
use super::{
    authentication_context::AuthenticationContext, authentication_service::AuthenticationService,
    passwd_validator::UnixValidator,
};
use actix_web::{
    body::{EitherBody, MessageBody},
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use futures::StreamExt;
use inotify::WatchMask;
use inotify::{EventMask, Inotify};
use std::{
    future::{ready, Ready},
    io,
    time::Duration,
};
use std::{rc::Rc, sync::Arc};
use tokio::{
    fs::OpenOptions,
    io::{AsyncBufReadExt, BufReader},
    sync::Mutex,
};

const SHADOW_FILE: &str = "/etc/shadow";

type LinuxContext = AuthenticationContext<UnixValidator>;

pub struct LinuxAuthenticator {
    context: Arc<Mutex<LinuxContext>>,
    authentication_path: &'static str,
    realm: &'static str,
}

impl LinuxAuthenticator {
    pub async fn new(
        authentication_path: &'static str,
        realm: &'static str,
        authentication_token_duration: Duration,
        authentication_attemps: usize,
    ) -> io::Result<Self> {
        let password_entries = Self::parse_shadow_file().await?;

        let instance = Self {
            context: Arc::new(Mutex::new(LinuxContext::with_unix_validator(
                password_entries,
                authentication_token_duration,
                authentication_attemps,
            ))),
            authentication_path,
            realm,
        };

        if let Err(e) = instance.auto_reload().await {
            log::warn!("auto reloading of password-cache disabled: {}", e);
        }

        Ok(instance)
    }
}

impl LinuxAuthenticator {
    /// Watches for any changes in the shadow file and reloads the password
    /// cache when a change is detected.
    async fn auto_reload(&self) -> std::io::Result<()> {
        let inotify = Inotify::init()?;
        let mask = WatchMask::DELETE_SELF | WatchMask::CLOSE_WRITE;

        inotify.watches().add(SHADOW_FILE, mask)?;
        let buffer = [0; 256];
        let mut event_stream = inotify.into_event_stream(buffer)?;

        let context = self.context.clone();
        tokio::spawn(async move {
            while let Some(Ok(event)) = event_stream.next().await {
                if EventMask::DELETE_SELF == event.mask {
                    event_stream
                        .watches()
                        .add(SHADOW_FILE, mask)
                        .expect("error rebinding shadow file watcher");
                    continue;
                }

                let mut lock = context.lock().await;
                Self::parse_shadow_file().await.map_or_else(
                    |e| log::error!("error parsing {}:{}", SHADOW_FILE, e),
                    |entries| {
                        lock.reload_password_cache(entries);
                        log::info!("reloaded user cache");
                    },
                );
            }
            log::warn!("exited /etc/shadow watcher");
        });

        Ok(())
    }

    async fn parse_shadow_file() -> io::Result<impl Iterator<Item = (String, String)>> {
        let file = OpenOptions::new().read(true).open(SHADOW_FILE).await?;

        let mut password_hashes: Vec<(String, String)> = Vec::new();
        let mut read_buffer = BufReader::new(file);

        loop {
            let mut line = String::new();
            let bytes_read = read_buffer.read_line(&mut line).await?;
            if bytes_read == 0 {
                break;
            }

            let mut items = line.splitn(3, ':');
            let username = items.next();
            let password = items.next();
            let (Some(user), Some(pass)) = (username, password) else {
                break;
            };

            if !pass.starts_with('*') {
                password_hashes.push((user.to_string(), pass.to_string()));
                log::debug!("loaded user {user}");
            }
        }
        Ok(password_hashes.into_iter())
    }
}

impl<S, B> Transform<S, ServiceRequest> for LinuxAuthenticator
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type InitError = ();
    type Transform = AuthenticationService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(AuthenticationService::new(
            Rc::new(service),
            self.context.clone(),
            self.authentication_path,
            self.realm,
        )))
    }
}
