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
    authentication_context::AuthenticationContext,
    authentication_errors::{AuthenticationError, SchemedAuthError},
    passwd_validator::UnixValidator,
};
use actix_web::{
    body::{EitherBody, MessageBody},
    dev::{Service, ServiceRequest, ServiceResponse},
    http::header::{self},
    Error, HttpRequest, HttpResponse,
};
use futures::future::LocalBoxFuture;
use futures::StreamExt;
use serde::Serialize;
use std::{rc::Rc, sync::Arc};
use tokio::sync::Mutex;

/// This authentication service is designed to prepare for implementing "Redfish
/// Session Login Authentication" as good as possible. Redfish is not yet
/// implemented in this product, until then this session based, token
/// authentication service is used to provide authentication.
#[derive(Clone)]
pub struct AuthenticationService<S> {
    service: Rc<S>,
    context: Arc<Mutex<AuthenticationContext<UnixValidator>>>,
    authentication_path: &'static str,
    realm: &'static str,
}

impl<S> AuthenticationService<S> {
    pub fn new(
        service: Rc<S>,
        context: Arc<Mutex<AuthenticationContext<UnixValidator>>>,
        authentication_path: &'static str,
        realm: &'static str,
    ) -> Self {
        AuthenticationService {
            service,
            context,
            authentication_path,
            realm,
        }
    }
}

impl<S, B> Service<ServiceRequest> for AuthenticationService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    actix_web::dev::forward_ready!(service);

    fn call(&self, mut request: ServiceRequest) -> Self::Future {
        let service = self.service.clone();

        // drop authentication for requests on loopback interface
        if request
            .head()
            .peer_addr
            .is_some_and(|addr| addr.ip().to_canonical().is_loopback())
        {
            return Box::pin(async move {
                service
                    .call(request)
                    .await
                    .map(ServiceResponse::map_into_left_body)
            });
        }

        let context = self.context.clone();
        let auth_path = self.authentication_path;
        let realm = self.realm;

        Box::pin(async move {
            let peer = request
                .connection_info()
                .peer_addr()
                .unwrap_or_default()
                .to_string();
            let mut context = context.lock().await;

            // handle authentication requests and return
            if request.request().uri().path() == auth_path {
                return authentication_request(&mut request, &peer, &mut context).await;
            }

            let auth = match parse_authorization_header(&request) {
                Ok(p) => p,
                Err(e) => {
                    return unauthorized_response(request.request(), e.into_basic_error(), realm)
                }
            };

            if let Err(e) = context.authorize_request(&peer, auth).await {
                unauthorized_response(request.request(), e, realm)
            } else {
                service
                    .call(request)
                    .await
                    .map(ServiceResponse::map_into_left_body)
            }
        })
    }
}

async fn authentication_request<B>(
    request: &mut ServiceRequest,
    peer: &str,
    context: &mut AuthenticationContext<UnixValidator>,
) -> Result<ServiceResponse<EitherBody<B>>, Error> {
    tracing::debug!("authentication request");
    let mut buffer = Vec::new();
    while let Some(Ok(bytes)) = request.parts_mut().1.next().await {
        buffer.extend_from_slice(&bytes);
    }

    let response = match context.authenticate_request(peer, &buffer).await {
        Ok(session) => authenticated_response(request.request(), session.id.clone(), session),
        Err(error) => forbidden_response(request.request(), error),
    };

    response
}

fn parse_authorization_header(request: &ServiceRequest) -> Result<&str, AuthenticationError> {
    tracing::debug!("authorize request");
    request
        .headers()
        .get(header::AUTHORIZATION)
        .ok_or(AuthenticationError::Empty)
        .and_then(|auth| {
            auth.to_str()
                .map_err(|e| AuthenticationError::HttpParseError(e.to_string()))
        })
}

fn forbidden_response<B, E: ToString>(
    request: &HttpRequest,
    response_text: E,
) -> Result<ServiceResponse<EitherBody<B>>, Error> {
    Ok(ServiceResponse::new(
        request.clone(),
        HttpResponse::Forbidden()
            .body(response_text.to_string())
            .map_into_right_body(),
    ))
}

fn authenticated_response<B>(
    request: &HttpRequest,
    token: String,
    body: impl Serialize,
) -> Result<ServiceResponse<EitherBody<B>>, Error> {
    let text = serde_json::to_string(&body)?;
    Ok(ServiceResponse::new(
        request.clone(),
        HttpResponse::Ok()
            .insert_header(("X-Auth-Token", token))
            .body(text)
            .map_into_right_body(),
    ))
}

fn unauthorized_response<B>(
    request: &HttpRequest,
    error: SchemedAuthError,
    realm: &str,
) -> Result<ServiceResponse<EitherBody<B>>, Error> {
    let response = HttpResponse::Unauthorized()
        .insert_header((header::WWW_AUTHENTICATE, error.challenge(realm)))
        .body(error.to_string());
    Ok(ServiceResponse::map_into_right_body(ServiceResponse::new(
        request.clone(),
        response,
    )))
}
