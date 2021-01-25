// Note: `hyper::upgrade` docs link to this upgrade.
// use std::str};

use base64::encode;
use std::future::Future;

use hyper::{
    header::{self, HeaderValue},
    http,
    upgrade::Upgraded,
    Body, Response, StatusCode,
};
use sha1::Digest;
use tokio::task::JoinError;
use tokio_tungstenite::tungstenite::protocol::Role;

use anyhow::Result;
use log::*;

pub type WsStream = tokio_tungstenite::WebSocketStream<Upgraded>;

const WS_MAGIC_UUID: &'static str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

fn convert_client_key(key: &str) -> String {
    let to_hash = format!("{}{}", key, WS_MAGIC_UUID);

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
/// Based off of the [Mozilla docs](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API/Writing_WebSocket_servers#the_websocket_handshake) on WebSocket servers.
pub fn create_ws(
    req: hyper::Request<hyper::Body>,
) -> http::Result<(
    hyper::Response<hyper::Body>,
    Option<impl Future<Output = Result<hyper::Result<WsStream>, JoinError>>>,
)> {
    debug!("request headers: {:?}", req.headers());

    let mut res = Response::new(Body::empty());

    // The method must be a GET request:
    if req.method() != hyper::Method::GET {
        *res.status_mut() = StatusCode::BAD_REQUEST;
    }

    // Version must be at least HTTP 1.1
    if req.version() < hyper::Version::HTTP_11 {
        *res.status_mut() = StatusCode::BAD_REQUEST;
    }

    // `Connection: upgrade` header must valid and present
    if let Some(header_value) = req.headers().get(header::CONNECTION) {
        if let Ok(value) = header_value.to_str() {
            if !value.eq_ignore_ascii_case("upgrade") {
                *res.status_mut() = StatusCode::BAD_REQUEST;
            }
        }
    } else {
        *res.status_mut() = StatusCode::BAD_REQUEST;
    }

    // `Upgrade: websocket` header must valid and present
    if let Some(header_value) = req.headers().get(header::UPGRADE) {
        if let Ok(value) = header_value.to_str() {
            if !value.eq_ignore_ascii_case("websocket") {
                *res.status_mut() = StatusCode::BAD_REQUEST;
            }
        }
    } else {
        *res.status_mut() = StatusCode::BAD_REQUEST;
    }

    // Fail before we attempt to upgrade the connection.
    if res.status() == StatusCode::BAD_REQUEST {
        return Ok((res, None));
    }

    if let Some(socket_key) = req.headers().get(header::SEC_WEBSOCKET_KEY) {
        if let Ok(socket_value) = socket_key.to_str() {
            trace!("socket key: {:?}", socket_value);

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

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::SocketAddr;

    use futures::{SinkExt, StreamExt};
    use http::request::Builder;
    use hyper::{
        service::{make_service_fn, service_fn},
        Client, Method, Request, Version,
    };
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    /// Our server HTTP handler to initiate HTTP upgrades.
    async fn ws_listener(req: Request<Body>) -> http::Result<Response<Body>> {
        trace!("{:?}", req);

        let (res, ws_fut) = match create_ws(req) {
            Ok(t) => t,
            Err(e) => {
                error!("error creating ws stream: {:?}", e);

                let mut res = Response::new(Body::empty());
                *res.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                return Ok(res);
            }
        };

        if let Some(ws_fut) = ws_fut {
            tokio::task::spawn(async move {
                if let Ok(Ok(mut stream)) = ws_fut.await {
                    while let Some(Ok(message)) = stream.next().await {
                        debug!("server rx: {:?}", message);
                    }
                }
            });
        }

        Ok(res)
    }

    fn create_server() -> SocketAddr {
        let addr = ([127, 0, 0, 1], 0).into();
        let make_service =
            make_service_fn(|_| async { Ok::<_, hyper::Error>(service_fn(ws_listener)) });

        let server = hyper::Server::bind(&addr).serve(make_service);

        // We need the assigned address for the client to send it messages.
        let addr = server.local_addr();
        debug!("Listening on: {:?}", addr);

        tokio::task::spawn(async move {
            if let Err(e) = server.await {
                eprintln!("server error: {}", e);
            }
        });

        addr
    }

    #[tokio::test]
    async fn roundtrip_ping() {
        let server_addr = create_server();

        let (stream, res) = connect_async(format!("ws://{}", server_addr))
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::SWITCHING_PROTOCOLS);

        let (mut write, mut read) = stream.split();

        let data = vec![1, 2, 3, 4, 5];
        let data_c = data.clone();

        tokio::task::spawn(async move { write.send(Message::Ping(data_c)).await });
        let pong = read.next().await.unwrap().unwrap();

        assert_eq!(Message::Pong(data), pong);
    }

    fn valid_request(server_addr: SocketAddr) -> Builder {
        Request::builder()
            .method(Method::GET)
            .uri(format!("http://{}", server_addr))
            .version(Version::HTTP_11)
            .header(header::CONNECTION, HeaderValue::from_static("upgrade"))
            .header(header::UPGRADE, HeaderValue::from_static("websocket"))
            .header(
                header::SEC_WEBSOCKET_KEY,
                HeaderValue::from_static("123456"),
            )
            .header(
                header::SEC_WEBSOCKET_VERSION,
                HeaderValue::from_static("13"),
            )
    }

    #[tokio::test]
    async fn invalid_request() {
        let _ = env_logger::try_init();

        let server_addr = create_server();
        let client = Client::new();

        let invalid_method = valid_request(server_addr)
            .method(Method::PUT)
            .body(Body::empty())
            .unwrap();

        let resp = client.request(invalid_method).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let invalid_version = valid_request(server_addr)
            .version(Version::HTTP_10)
            .body(Body::empty())
            .unwrap();

        let resp = client.request(invalid_version).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let mut no_connection_header = valid_request(server_addr);
        no_connection_header
            .headers_mut()
            .unwrap()
            .remove(header::CONNECTION);
        let no_connection_header = no_connection_header.body(Body::empty()).unwrap();

        let resp = client.request(no_connection_header).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let mut no_upgrade_header = valid_request(server_addr);
        no_upgrade_header
            .headers_mut()
            .unwrap()
            .remove(header::UPGRADE);
        let no_upgrade_header = no_upgrade_header.body(Body::empty()).unwrap();

        let resp = client.request(no_upgrade_header).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let mut no_key_header = valid_request(server_addr);
        no_key_header
            .headers_mut()
            .unwrap()
            .remove(header::SEC_WEBSOCKET_KEY);
        let no_key_header = no_key_header.body(Body::empty()).unwrap();

        let resp = client.request(no_key_header).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // Request and Response key values take from Mozilla's article on
    // implementing a
    #[tokio::test]
    async fn valid_key_hash() {
        let server_addr = create_server();
        let client = Client::new();

        let mut request = valid_request(server_addr);

        request
            .headers_mut()
            .unwrap()
            .remove(header::SEC_WEBSOCKET_KEY);

        let request = request
            .header(
                header::SEC_WEBSOCKET_KEY,
                HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
            )
            .body(Body::empty())
            .unwrap();

        let resp = client.request(request).await.unwrap();

        let accept_key = &resp.headers()[header::SEC_WEBSOCKET_ACCEPT];

        assert_eq!(accept_key, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
    }
}
