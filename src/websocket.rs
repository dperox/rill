//! HTTP/1.1 WebSocket handshake headers and bidirectional transport.
use super::{Lease, error_page, strip_hop_headers};
use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use hyper::{body::Incoming, upgrade::OnUpgrade};
use hyper_util::rt::TokioIo;
use ring::digest::{Context, SHA1_FOR_LEGACY_USE_ONLY};

fn contains_token(headers: &HeaderMap, name: header::HeaderName, token: &str) -> bool {
    headers.get_all(name).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|value| value.trim().eq_ignore_ascii_case(token))
        })
    })
}

pub fn requests_upgrade(headers: &HeaderMap) -> bool {
    contains_token(headers, header::UPGRADE, "websocket")
}

pub fn connection_upgrade(headers: &HeaderMap) -> bool {
    contains_token(headers, header::CONNECTION, "upgrade")
}

pub fn set_upgrade_headers(headers: &mut HeaderMap) {
    headers.insert(
        header::CONNECTION,
        header::HeaderValue::from_static("upgrade"),
    );
    headers.insert(
        header::UPGRADE,
        header::HeaderValue::from_static("websocket"),
    );
}

pub enum Downstream {
    Hyper(OnUpgrade),
    Kit,
}

pub fn downstream(req: &mut axum::extract::Request) -> Option<Downstream> {
    if req.extensions().get::<crate::plain::Transport>().is_some() {
        Some(Downstream::Kit)
    } else {
        req.extensions_mut()
            .remove::<OnUpgrade>()
            .map(Downstream::Hyper)
    }
}

type TunnelParts = (OnUpgrade, Option<Lease>);

#[derive(Clone)]
pub struct Tunnel(std::sync::Arc<std::sync::Mutex<Option<TunnelParts>>>);

impl Tunnel {
    pub fn take(self) -> Option<TunnelParts> {
        self.0.lock().unwrap().take()
    }
}

pub struct Handshake {
    downstream: Downstream,
    accept: String,
    protocols: Vec<header::HeaderValue>,
}

impl Handshake {
    pub fn new(downstream: Downstream, headers: &HeaderMap) -> Option<Self> {
        let key = single_value(headers, header::SEC_WEBSOCKET_KEY)?
            .to_str()
            .ok()?;
        let key = key.trim();
        if key.len() != 24 || STANDARD.decode(key).ok()?.len() != 16 {
            return None;
        }

        // RFC 6455 defines this digest for the handshake.
        let mut digest = Context::new(&SHA1_FOR_LEGACY_USE_ONLY);
        digest.update(key.as_bytes());
        digest.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");

        Some(Self {
            downstream,
            accept: STANDARD.encode(digest.finish()),
            protocols: headers
                .get_all(header::SEC_WEBSOCKET_PROTOCOL)
                .iter()
                .cloned()
                .collect(),
        })
    }

    fn accepts_protocol(&self, headers: &HeaderMap) -> bool {
        if !headers.contains_key(header::SEC_WEBSOCKET_PROTOCOL) {
            return true;
        }
        let Some(selected) = single_value(headers, header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
        else {
            return false;
        };

        self.protocols.iter().any(|value| {
            value.to_str().is_ok_and(|value| {
                value
                    .split(',')
                    .any(|offered| !offered.trim().is_empty() && offered.trim() == selected.trim())
            })
        })
    }
}

fn single_value(headers: &HeaderMap, name: header::HeaderName) -> Option<&header::HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

pub fn switch_protocols(
    mut response: hyper::Response<Incoming>,
    handshake: Option<Handshake>,
    lease: Option<Lease>,
) -> Response {
    let Some(handshake) = handshake else {
        return error_page(StatusCode::BAD_GATEWAY);
    };
    let headers = response.headers_mut();
    let valid_upgrade = single_value(headers, header::UPGRADE)
        .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"websocket"));
    if !connection_upgrade(headers) || !valid_upgrade || !handshake.accepts_protocol(headers) {
        return error_page(StatusCode::BAD_GATEWAY);
    }
    let selected_protocol = headers.contains_key(header::SEC_WEBSOCKET_PROTOCOL);
    strip_hop_headers(headers);
    if single_value(headers, header::SEC_WEBSOCKET_ACCEPT)
        .is_none_or(|value| value.as_bytes() != handshake.accept.as_bytes())
        || selected_protocol != headers.contains_key(header::SEC_WEBSOCKET_PROTOCOL)
    {
        return error_page(StatusCode::BAD_GATEWAY);
    }
    let Some(upstream) = response.extensions_mut().remove::<OnUpgrade>() else {
        return error_page(StatusCode::BAD_GATEWAY);
    };
    set_upgrade_headers(response.headers_mut());
    response.headers_mut().remove(header::CONTENT_LENGTH);

    let Downstream::Hyper(downstream) = handshake.downstream else {
        response
            .extensions_mut()
            .insert(Tunnel(std::sync::Arc::new(std::sync::Mutex::new(Some((
                upstream, lease,
            ))))));
        return response.map(|_| Body::empty());
    };
    tokio::spawn(async move {
        let _lease = lease;
        if let Ok((downstream, upstream)) = tokio::try_join!(downstream, upstream) {
            let _ = tokio::io::copy_bidirectional(
                &mut TokioIo::new(downstream),
                &mut TokioIo::new(upstream),
            )
            .await;
        }
    });
    response.map(|_| Body::empty())
}
