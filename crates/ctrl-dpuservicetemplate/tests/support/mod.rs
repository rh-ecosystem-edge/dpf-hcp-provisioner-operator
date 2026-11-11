//! In-process "rules" registry (axum on 127.0.0.1) mirroring the httptest server of the Go
//! golden generators: `GET /v2/` answers 200, every other request is matched against fixed
//! METHOD+path rules (an empty method matches GET and HEAD) taken from the fixture, and
//! anything else is a bare 404.

#![allow(dead_code)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Response, StatusCode, Uri};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Rule {
    pub method: String,
    pub path: String,
    pub status: u16,
    #[serde(default)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    /// base64
    #[serde(default)]
    pub body: String,
}

pub struct RulesRegistry {
    pub host: String,
}

async fn handle(State(rules): State<Arc<Vec<Rule>>>, method: Method, uri: Uri) -> Response<Body> {
    let path = uri.path();
    if path == "/v2/" {
        return Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap();
    }
    for rule in rules.iter() {
        if rule.path != path {
            continue;
        }
        if !rule.method.is_empty() && rule.method != method.as_str() {
            continue;
        }
        if rule.method.is_empty() && method != Method::GET && method != Method::HEAD {
            continue;
        }
        let body = gocompat::base64::decode_string(rule.body.as_bytes()).unwrap_or_default();
        let mut b = Response::builder().status(rule.status);
        for (k, v) in rule.headers.iter().flatten() {
            b = b.header(k, v);
        }
        b = b.header("Content-Length", body.len().to_string());
        let body = if method == Method::HEAD {
            Body::empty()
        } else {
            Body::from(body)
        };
        return b.body(body).unwrap();
    }
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::empty())
        .unwrap()
}

impl RulesRegistry {
    pub async fn start(rules: Vec<Rule>) -> RulesRegistry {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().fallback(handle).with_state(Arc::new(rules));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        RulesRegistry {
            host: addr.to_string(),
        }
    }

    /// Replace the registry address by "REGISTRY" (the fixtures' normalization).
    pub fn norm(&self, s: &str) -> String {
        s.replace(&self.host, "REGISTRY")
    }
}
