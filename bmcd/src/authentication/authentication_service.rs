use super::{
    authentication_context::AuthenticationContext, authentication_errors::AuthenticationError,
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
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    rc::Rc,
    sync::Arc,
};

const LOCALHOSTV4: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
const LOCALHOSTV6: IpAddr = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1));

/// This authentication service is designed to prepare for implementing "Redfish
/// Session Login Authentication" as good as possible. Redfish is not yet
/// implemented in this product, until then this session based, token
/// authentication service is used to provide authentication.
#[derive(Clone)]
pub struct AuthenticationService<S> {
    service: Rc<S>,
    context: Arc<AuthenticationContext<UnixValidator>>,
    authentication_path: &'static str,
}

impl<S> AuthenticationService<S> {
    pub fn new(
        service: Rc<S>,
        context: Arc<AuthenticationContext<UnixValidator>>,
        authentication_path: &'static str,
    ) -> Self {
        AuthenticationService {
            service,
            context,
            authentication_path,
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
            .is_some_and(|addr| addr.ip() == LOCALHOSTV6 || addr.ip() == LOCALHOSTV4)
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

        Box::pin(async move {
            if request.request().uri().path() == auth_path {
                log::debug!("authentication request");
                let mut buffer = Vec::new();
                while let Some(Ok(bytes)) = request.parts_mut().1.next().await {
                    buffer.extend_from_slice(&bytes);
                }

                let response = match context.authenticate_request(&buffer).await {
                    Ok(session) => {
                        authenticated_response(request.request(), session.id.clone(), session)
                    }
                    Err(error) => forbidden_response(request.request(), error),
                };

                return response;
            }

            log::debug!("authorize request");
            let parse_result = request
                .headers()
                .get(header::AUTHORIZATION)
                .ok_or(AuthenticationError::Empty)
                .and_then(|auth| {
                    auth.to_str()
                        .map_err(|e| AuthenticationError::HttpParseError(e.to_string()))
                });

            let auth = match parse_result {
                Ok(p) => p,
                Err(e) => return unauthorized_response(request.request(), e),
            };

            if let Err(e) = context.authorize_request(auth).await {
                unauthorized_response(request.request(), e)
            } else {
                service
                    .call(request)
                    .await
                    .map(ServiceResponse::map_into_left_body)
            }
        })
    }
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

fn unauthorized_response<B, E: ToString>(
    request: &HttpRequest,
    response_text: E,
) -> Result<ServiceResponse<EitherBody<B>>, Error> {
    let bearer_str = format!(
        "Bearer error=invalid_token, error_description={}",
        response_text.to_string()
    );
    let response = HttpResponse::Unauthorized()
        .insert_header((header::WWW_AUTHENTICATE, bearer_str))
        .body(response_text.to_string());
    Ok(ServiceResponse::new(request.clone(), response)).map(ServiceResponse::map_into_right_body)
}
