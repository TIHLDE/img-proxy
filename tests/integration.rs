#![allow(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] fns are not exempted by clippy.toml"
)]

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{StatusCode, header},
    response::IntoResponse,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use img_proxy::{app, config::Config, sign};
use tower::ServiceExt;

const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

/// Fake imagor: keys containing `renamed-pdf` are answered as a PDF, like
/// imagor would after sniffing the bytes; everything else is WebP.
async fn fake_imagor() -> String {
    let router = Router::new().fallback(|req: Request| async move {
        let path = req.uri().path().to_string();
        let b64 = path.rsplit_once("b64:").unwrap().1;
        let key = String::from_utf8(URL_SAFE_NO_PAD.decode(b64).unwrap()).unwrap();
        let content_type = if key.contains("renamed-pdf") {
            "application/pdf"
        } else {
            "image/webp"
        };
        (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "public, max-age=604800"),
                (
                    header::HeaderName::from_static("x-imagor-path"),
                    path.as_str(),
                ),
                (header::HeaderName::from_static("x-internal"), "secret"),
            ],
            "image-bytes",
        )
            .into_response()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

async fn proxy() -> Router {
    app(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        imagor_url: fake_imagor().await,
        secret: SECRET.to_vec(),
        public_bucket: "public".into(),
        private_bucket: "photon-files".into(),
        sizes: vec![320, 640],
        private_max_age: 3600,
    })
    .unwrap()
}

async fn get(app: Router, uri: &str) -> axum::response::Response {
    app.oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test]
async fn public_image() {
    let resp = get(proxy().await, "/public/a.jpg?w=640&fmt=webp").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let h = resp.headers();
    assert_eq!(h["content-type"], "image/webp");
    assert_eq!(h["cache-control"], "public, max-age=604800");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert!(h.contains_key("content-security-policy"));
    assert!(
        !h.contains_key("x-internal"),
        "unlisted headers must not leak"
    );
    assert!(!h.contains_key("x-imagor-path"));
    assert_eq!(
        resp.into_body().collect().await.unwrap().to_bytes(),
        "image-bytes"
    );
}

#[tokio::test]
async fn private_image_gets_private_cache_control() {
    let exp = now() + 60;
    let sig = sign::sign(SECRET, "a.jpg", exp).unwrap();
    let resp = get(
        proxy().await,
        &format!("/private/a.jpg?w=320&exp={exp}&sig={sig}"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cc = resp.headers()["cache-control"].to_str().unwrap();
    let max_age: u64 = cc
        .strip_prefix("private, max-age=")
        .unwrap()
        .parse()
        .unwrap();
    assert!(max_age <= 60, "{cc}");
}

#[tokio::test]
async fn rejects_non_image_content_type() {
    // A PDF renamed to .jpg: the extension passes, imagor's sniffed type doesn't.
    let resp = get(proxy().await, "/public/renamed-pdf.jpg?original").await;
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn rejection_statuses() {
    let app = proxy().await;
    let expired = now() - 1;
    let sig = sign::sign(SECRET, "a.jpg", expired).unwrap();
    for (uri, status) in [
        ("/public/a.pdf", StatusCode::NOT_FOUND),
        ("/public/a.jpg?w=123", StatusCode::BAD_REQUEST),
        ("/private/a.jpg", StatusCode::FORBIDDEN),
        (
            &format!("/private/a.jpg?exp={expired}&sig={sig}"),
            StatusCode::GONE,
        ),
    ] {
        assert_eq!(get(app.clone(), uri).await.status(), status, "{uri}");
    }
    let resp = app
        .oneshot(Request::post("/public/a.jpg").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}
