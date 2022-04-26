#![allow(clippy::blacklisted_name)]

use crate::{body::HttpBody, BoxError};

mod test_client;
pub(crate) use self::test_client::*;

pub(crate) fn assert_send<T: Send>() {}
pub(crate) fn assert_sync<T: Sync>() {}
pub(crate) fn assert_unpin<T: Unpin>() {}

pub(crate) struct NotSendSync(*const ());

#[cfg(feature = "integration_tests")]
pub mod integration_tests {
    use super::{BoxError, HttpBody};
    use crate::test_helpers::TestClient;
    use bytes::Bytes;
    use http::{
        header::{HeaderName, HeaderValue},
        Request, StatusCode,
    };
    use hyper::{Body, Server};
    use std::{
        convert::TryFrom,
        net::{SocketAddr, TcpListener},
    };
    use tower::make::Shared;
    use tower_service::Service;

    pub fn new<S, ResBody>(svc: S) -> TestClient
    where
        S: Service<Request<Body>, Response = http::Response<ResBody>> + Clone + Send + 'static,
        ResBody: HttpBody + Send + 'static,
        ResBody::Data: Send,
        ResBody::Error: Into<BoxError>,
        S::Future: Send,
        S::Error: Into<BoxError>,
    {
        TestClient::new(svc)
    }
}
