// Note: `hyper::upgrade` docs link to this upgrade.
// use std::str};

use base64::encode;
use futures::StreamExt;
use std::future::Future;

use hyper::{
    header::{self, HeaderValue},
    http::{self, method},
    service::{make_service_fn, service_fn},
    upgrade::Upgraded,
    Body, Request, Response, Server, StatusCode,
};
use sha1::{Digest, Sha1};
use tokio::{sync::oneshot, task::JoinError};
use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

use anyhow::Result;
use log::*;

pub type WsStream = tokio_tungstenite::WebSocketStream<Upgraded>;

const WS_HASH_UUID: &'static str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

fn convert_client_key(key: &str) -> String {
    let to_hash = format!("{}{}", key, WS_HASH_UUID);

    let hash = sha1::Sha1::digest(to_hash.as_bytes());
    encode(&hash)
}

async fn convert_to_ws_stream(upgraded: Upgraded) -> WsStream {
    tokio_tungstenite::WebSocketStream::from_partially_read(
        upgraded,
        Vec::new(),
        Role::Server,
        None,
    )
    .await
}

fn upgrade_connection(
    mut req: hyper::Request<hyper::Body>,
) -> impl Future<Output = Result<hyper::Result<WsStream>, JoinError>> {
    tokio::task::spawn(async move {
        match hyper::upgrade::on(&mut req).await {
            Ok(upgraded) => Ok(convert_to_ws_stream(upgraded).await),
            Err(e) => Err(e),
        }
    })
}

/// Handle a WS handshake and create a tokio_tungstenite stream
/// Based off of the [Mozilla docs](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API/Writing_WebSocket_servers#the_websocket_handshake) on WS servers.
pub async fn create_ws(
    req: hyper::Request<hyper::Body>,
) -> http::Result<(
    hyper::Response<hyper::Body>,
    Option<impl Future<Output = Result<hyper::Result<WsStream>, JoinError>>>,
)> {
    println!("Recieved: {:?}", req);

    let mut res = Response::new(Body::empty());

    // The method must be a GET request:
    if req.method() != hyper::Method::GET {
        *res.status_mut() = StatusCode::BAD_REQUEST;
    }

    // Version must be at least HTTP 1.1
    if req.version() < hyper::Version::HTTP_11 {
        *res.status_mut() = StatusCode::BAD_REQUEST;
    }

    // `Connection: Upgrade` header must be present
    if let Some(header_value) = req.headers().get(header::CONNECTION) {
        if let Ok(value) = header_value.to_str() {
            if !value.eq_ignore_ascii_case("upgrade") {
                *res.status_mut() = StatusCode::BAD_REQUEST;
            }
        }
    }

    // `Upgrade: websocket` header must be present
    if let Some(header_value) = req.headers().get(header::UPGRADE) {
        if let Ok(value) = header_value.to_str() {
            if !value.eq_ignore_ascii_case("websocket") {
                *res.status_mut() = StatusCode::BAD_REQUEST;
            }
        }
    }

    // Fail before we attempt to upgrade the connection.
    if res.status() == StatusCode::BAD_REQUEST {
        return Ok((res, None));
    }

    if let Some(socket_key) = req.headers().get(header::SEC_WEBSOCKET_KEY) {
        if let Ok(socket_value) = socket_key.to_str() {
            println!("Socket Value: {:?}", socket_value);

            *res.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
            res.headers_mut()
                .insert(header::UPGRADE, HeaderValue::from_static("websocket"));
            res.headers_mut()
                .insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
            res.headers_mut().insert(
                header::SEC_WEBSOCKET_ACCEPT,
                HeaderValue::from_str(&convert_client_key(&socket_value))?,
            );

            return Ok((res, Some(upgrade_connection(req))));
        }
    }

    *res.status_mut() = StatusCode::BAD_REQUEST;
    Ok((res, None))
}

async fn idle_stream(mut stream: WebSocketStream<Upgraded>) {
    println!("idling stream");

    while let Some(Ok(message)) = stream.next().await {
        println!("Message: {:?}", message);
    }
}

/// Our server HTTP handler to initiate HTTP upgrades.
async fn ws_listener(req: Request<Body>) -> http::Result<Response<Body>> {
    println!("{:?}", req);

    let (res, ws_fut) = match create_ws(req).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error creating WS stream: {:?}", e);

            let mut res = Response::new(Body::empty());
            *res.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            return Ok(res);
        }
    };

    if let Some(ws_fut) = ws_fut {
        tokio::task::spawn(async move {
            if let Ok(Ok(stream)) = ws_fut.await {
                idle_stream(stream).await;
            }
        });
    }

    Ok(res)
}

#[tokio::main]
async fn main() {
    env_logger::try_init().unwrap();

    let addr = ([127, 0, 0, 1], 0).into();
    let make_service =
        make_service_fn(|_| async { Ok::<_, hyper::Error>(service_fn(ws_listener)) });

    let server = Server::bind(&addr).serve(make_service);

    println!("Bound server");

    // We need the assigned address for the client to send it messages.
    let addr = server.local_addr();
    println!("Addr: {:?}", addr);

    println!("Running server");
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}
