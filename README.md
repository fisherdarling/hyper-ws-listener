## hyper-ws-listener

Hyper WebSocket Listener is a library for creating a [tokio_tungstenite](https://docs.rs/tokio-tungstenite/0.13.0/tokio_tungstenite/struct.WebSocketStream.html) websocket stream from a hyper `Request<Body>`.

Since the server upgrade response must be sent before the stream is upgraded, a tuple of the formatted response and an `Option<Future<...>>` is returned. This future will resolve to the WebSocket stream or an error if an error occurred while upgrading the connection.


## Example Usage

This example shows a roundtrip ping-pong over the created websocket stream.

```rust
use futures::{SinkExt, StreamExt};
use http::StatusCode;
use hyper::{
    service::{make_service_fn, service_fn},
    Body, Request, Response,
};
use log::*;
use tokio_tungstenite::tungstenite::Message;

/// Hyper handler that initiates HTTP upgrades.
async fn ws_listener(req: Request<Body>) -> http::Result<Response<Body>> {
    trace!("{:?}", req);

    // Attempt to create a websocket stream using the crate
    let (res, ws_fut) = match hyper_ws_listener::create_ws(req).await {
        Ok(t) => t,
        Err(e) => {
            error!("error creating ws stream: {:?}", e);

            let mut res = Response::new(Body::empty());
            *res.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            return Ok(res);
        }
    };

    // If the request was valid, this will be `Some(_)`
    // This is a future since the stream might still be
    // in the process of being created. We do not want to
    // block returning `res` since that response is
    // necessary for converting to a WS stream.
    if let Some(ws_fut) = ws_fut {
        tokio::task::spawn(async move {
            if let Ok(Ok(mut stream)) = ws_fut.await {
                while let Some(Ok(message)) = stream.next().await {
                    println!("{:?}", message);
                }
            }
        });
    }

    // Return the response that will notify the client that
    // the protocol is changing `StatusCode 101`.
    Ok(res)
}

#[tokio::main]
async fn main() {
    env_logger::try_init().unwrap();

    // Create a hyper service that will try to upgrade a request
    // to a WebSocket stream.
    let make_service =
        make_service_fn(|_| async { Ok::<_, hyper::Error>(service_fn(ws_listener)) });

    let server_addr = ([127, 0, 0, 1], 0).into();
    let server = hyper::Server::bind(&server_addr).serve(make_service);

    // We need the address for the client to send messages.
    let server_addr = server.local_addr();
    debug!("listening on: {:?}", server_addr);

    tokio::task::spawn(async move {
        if let Err(e) = server.await {
            eprintln!("server error: {}", e);
        }
    });

    // Using tokio_tungstenite, start the WebSocket handshake with the server.
    let (stream, _res) = tokio_tungstenite::connect_async(format!("ws://{}", server_addr))
        .await
        .unwrap();

    let (mut write, mut read) = stream.split();

    let data = vec![1, 2, 3, 4, 5];
    let data_c = data.clone();

    // Write some data and verify that the server sent back the proper data.
    tokio::task::spawn(async move { write.send(Message::Ping(data_c)).await });
    let pong = read.next().await.unwrap().unwrap();

    assert_eq!(Message::Pong(data), pong);
}
```