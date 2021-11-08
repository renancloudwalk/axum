use crate::error_handling::HandleErrorLayer;
use crate::test_helpers::*;
use crate::BoxError;
use crate::{
    extract::{self, Path},
    handler::Handler,
    response::IntoResponse,
    routing::{any, delete, get, on, patch, post, service_method_routing as service, MethodFilter},
    Json, Router,
};
use bytes::Bytes;
use http::{header::HeaderMap, Method, Request, Response, StatusCode, Uri};
use hyper::Body;
use serde::Deserialize;
use serde_json::{json, Value};
use std::future::Ready;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    convert::Infallible,
    future::ready,
    task::{Context, Poll},
    time::Duration,
};
use tower::{service_fn, timeout::TimeoutLayer, ServiceBuilder, ServiceExt};
use tower_http::auth::RequireAuthorizationLayer;
use tower_service::Service;

mod fallback;
mod get_to_head;
mod handle_error;
mod merge;
mod nest;

#[tokio::test]
async fn hello_world() {
    async fn root(_: Request<Body>) -> &'static str {
        "Hello, World!"
    }

    async fn foo(_: Request<Body>) -> &'static str {
        "foo"
    }

    async fn users_create(_: Request<Body>) -> &'static str {
        "users#create"
    }

    let app = Router::new()
        .route("/", get(root).post(foo))
        .route("/users", post(users_create));

    let client = TestClient::new(app);

    let res = client.get("/").send().await;
    let body = res.text().await;
    assert_eq!(body, "Hello, World!");

    let res = client.post("/").send().await;
    let body = res.text().await;
    assert_eq!(body, "foo");

    let res = client.post("/users").send().await;
    let body = res.text().await;
    assert_eq!(body, "users#create");
}

#[tokio::test]
async fn routing() {
    let app = Router::new()
        .route(
            "/users",
            get(|_: Request<Body>| async { "users#index" })
                .post(|_: Request<Body>| async { "users#create" }),
        )
        .route("/users/:id", get(|_: Request<Body>| async { "users#show" }))
        .route(
            "/users/:id/action",
            get(|_: Request<Body>| async { "users#action" }),
        );

    let client = TestClient::new(app);

    let res = client.get("/").send().await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let res = client.get("/users").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "users#index");

    let res = client.post("/users").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "users#create");

    let res = client.get("/users/1").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "users#show");

    let res = client.get("/users/1/action").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "users#action");
}

#[tokio::test]
async fn router_type_doesnt_change() {
    let app: Router = Router::new()
        .route(
            "/",
            on(MethodFilter::GET, |_: Request<Body>| async {
                "hi from GET"
            })
            .on(MethodFilter::POST, |_: Request<Body>| async {
                "hi from POST"
            }),
        )
        .layer(tower_http::compression::CompressionLayer::new());

    let client = TestClient::new(app);

    let res = client.get("/").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "hi from GET");

    let res = client.post("/").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "hi from POST");
}

#[tokio::test]
async fn routing_between_services() {
    use std::convert::Infallible;
    use tower::service_fn;

    async fn handle(_: Request<Body>) -> &'static str {
        "handler"
    }

    let app = Router::new()
        .route(
            "/one",
            service::get(service_fn(|_: Request<Body>| async {
                Ok::<_, Infallible>(Response::new(Body::from("one get")))
            }))
            .post(service_fn(|_: Request<Body>| async {
                Ok::<_, Infallible>(Response::new(Body::from("one post")))
            }))
            .on(
                MethodFilter::PUT,
                service_fn(|_: Request<Body>| async {
                    Ok::<_, Infallible>(Response::new(Body::from("one put")))
                }),
            ),
        )
        .route("/two", service::on(MethodFilter::GET, any(handle)));

    let client = TestClient::new(app);

    let res = client.get("/one").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "one get");

    let res = client.post("/one").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "one post");

    let res = client.put("/one").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "one put");

    let res = client.get("/two").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "handler");
}

#[tokio::test]
async fn middleware_on_single_route() {
    use tower::ServiceBuilder;
    use tower_http::{compression::CompressionLayer, trace::TraceLayer};

    async fn handle(_: Request<Body>) -> &'static str {
        "Hello, World!"
    }

    let app = Router::new().route(
        "/",
        get(handle.layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CompressionLayer::new())
                .into_inner(),
        )),
    );

    let client = TestClient::new(app);

    let res = client.get("/").send().await;
    let body = res.text().await;

    assert_eq!(body, "Hello, World!");
}

#[tokio::test]
async fn service_in_bottom() {
    async fn handler(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
        Ok(Response::new(hyper::Body::empty()))
    }

    let app = Router::new().route("/", service::get(service_fn(handler)));

    TestClient::new(app);
}

#[tokio::test]
async fn wrong_method_handler() {
    let app = Router::new()
        .route("/", get(|| async {}).post(|| async {}))
        .route("/foo", patch(|| async {}));

    let client = TestClient::new(app);

    let res = client.patch("/").send().await;
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);

    let res = client.patch("/foo").send().await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = client.post("/foo").send().await;
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);

    let res = client.get("/bar").send().await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn wrong_method_service() {
    #[derive(Clone)]
    struct Svc;

    impl<R> Service<R> for Svc {
        type Response = Response<http_body::Empty<Bytes>>;
        type Error = Infallible;
        type Future = Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: R) -> Self::Future {
            ready(Ok(Response::new(http_body::Empty::new())))
        }
    }

    let app = Router::new()
        .route("/", service::get(Svc).post(Svc))
        .route("/foo", service::patch(Svc));

    let client = TestClient::new(app);

    let res = client.patch("/").send().await;
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);

    let res = client.patch("/foo").send().await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = client.post("/foo").send().await;
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);

    let res = client.get("/bar").send().await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn multiple_methods_for_one_handler() {
    async fn root(_: Request<Body>) -> &'static str {
        "Hello, World!"
    }

    let app = Router::new().route("/", on(MethodFilter::GET | MethodFilter::POST, root));

    let client = TestClient::new(app);

    let res = client.get("/").send().await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = client.post("/").send().await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn wildcard_sees_whole_url() {
    let app = Router::new().route("/api/*rest", get(|uri: Uri| async move { uri.to_string() }));

    let client = TestClient::new(app);

    let res = client.get("/api/foo/bar").send().await;
    assert_eq!(res.text().await, "/api/foo/bar");
}

#[tokio::test]
async fn middleware_applies_to_routes_above() {
    let app = Router::new()
        .route("/one", get(std::future::pending::<()>))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(|_: BoxError| {
                    StatusCode::REQUEST_TIMEOUT
                }))
                .layer(TimeoutLayer::new(Duration::new(0, 0))),
        )
        .route("/two", get(|| async {}));

    let client = TestClient::new(app);

    let res = client.get("/one").send().await;
    assert_eq!(res.status(), StatusCode::REQUEST_TIMEOUT);

    let res = client.get("/two").send().await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn with_trailing_slash() {
    let app = Router::new().route("/foo", get(|| async {}));

    let client = TestClient::new(app);

    // `TestClient` automatically follows redirects
    let res = client.get("/foo/").send().await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn without_trailing_slash() {
    let app = Router::new().route("/foo/", get(|| async {}));

    let client = TestClient::new(app);

    // `TestClient` automatically follows redirects
    let res = client.get("/foo").send().await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn with_and_without_trailing_slash() {
    let app = Router::new()
        .route("/foo", get(|| async { "without tsr" }))
        .route("/foo/", get(|| async { "with tsr" }));

    let client = TestClient::new(app);

    let res = client.get("/foo/").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "with tsr");

    let res = client.get("/foo").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await, "without tsr");
}

// for https://github.com/tokio-rs/axum/issues/420
#[tokio::test]
async fn wildcard_with_trailing_slash() {
    #[derive(Deserialize, serde::Serialize)]
    struct Tree {
        user: String,
        repo: String,
        path: String,
    }

    let app: Router = Router::new().route(
        "/:user/:repo/tree/*path",
        get(|Path(tree): Path<Tree>| async move { Json(tree) }),
    );

    // low level check that the correct redirect happens
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/user1/repo1/tree")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(res.headers()["location"], "/user1/repo1/tree/");

    // check that the params are deserialized correctly
    let client = TestClient::new(app);
    let res = client.get("/user1/repo1/tree/").send().await;
    assert_eq!(
        res.json::<Value>().await,
        json!({
            "user": "user1",
            "repo": "repo1",
            "path": "/",
        })
    );
}

#[tokio::test]
async fn static_and_dynamic_paths() {
    let app = Router::new()
        .route(
            "/:key",
            get(|Path(key): Path<String>| async move { format!("dynamic: {}", key) }),
        )
        .route("/foo", get(|| async { "static" }));

    let client = TestClient::new(app);

    let res = client.get("/bar").send().await;
    assert_eq!(res.text().await, "dynamic: bar");

    let res = client.get("/foo").send().await;
    assert_eq!(res.text().await, "static");
}

#[tokio::test]
#[should_panic(expected = "Invalid route: empty path")]
async fn empty_route() {
    let app = Router::new().route("", get(|| async {}));
    TestClient::new(app);
}

#[tokio::test]
#[should_panic(expected = "Invalid route: empty path")]
async fn empty_route_nested() {
    let app = Router::new().nest("", get(|| async {}));
    TestClient::new(app);
}

#[tokio::test]
async fn middleware_still_run_for_unmatched_requests() {
    #[derive(Clone)]
    struct CountMiddleware<S>(S);

    static COUNT: AtomicUsize = AtomicUsize::new(0);

    impl<R, S> Service<R> for CountMiddleware<S>
    where
        S: Service<R>,
    {
        type Response = S::Response;
        type Error = S::Error;
        type Future = S::Future;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.0.poll_ready(cx)
        }

        fn call(&mut self, req: R) -> Self::Future {
            COUNT.fetch_add(1, Ordering::SeqCst);
            self.0.call(req)
        }
    }

    let app = Router::new()
        .route("/", get(|| async {}))
        .layer(tower::layer::layer_fn(CountMiddleware));

    let client = TestClient::new(app);

    assert_eq!(COUNT.load(Ordering::SeqCst), 0);

    client.get("/").send().await;
    assert_eq!(COUNT.load(Ordering::SeqCst), 1);

    client.get("/not-found").send().await;
    assert_eq!(COUNT.load(Ordering::SeqCst), 2);
}

#[tokio::test]
#[should_panic(
    expected = "Invalid route: `Router::route` cannot be used with `Router`s. Use `Router::nest` instead"
)]
async fn routing_to_router_panics() {
    TestClient::new(Router::new().route("/", Router::new()));
}

#[tokio::test]
async fn route_layer() {
    let app = Router::new()
        .route("/foo", get(|| async {}))
        .route_layer(RequireAuthorizationLayer::bearer("password"));

    let client = TestClient::new(app);

    let res = client
        .get("/foo")
        .header("authorization", "Bearer password")
        .send()
        .await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = client.get("/foo").send().await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    let res = client.get("/not-found").send().await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // it would be nice if this would return `405 Method Not Allowed`
    // but that requires knowing more about which method route we're calling, which we
    // don't know currently since its just a generic `Service`
    let res = client.post("/foo").send().await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}