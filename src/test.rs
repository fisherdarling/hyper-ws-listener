use std::time::Duration;

use futures::{SinkExt, StreamExt};
use hyper::{
    http,
    service::{make_service_fn, service_fn},
    Body, Request, Response,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use hyper_ws_listener::create_ws;
use log::*;

// #[tokio::main]
// async fn main() {
//     env_logger::try_init().unwrap();

//     let (stream, _res) = connect_async("ws://localhost:43567").await.unwrap();
//     println!("created stream: {:?}", stream);

//     let (mut write, mut read) = stream.split();

//     tokio::task::spawn(async move {
//         let mut interval = tokio::time::interval(Duration::from_millis(500));

//         loop {
//             interval.tick().await;

//             let message = Message::Ping(vec![1, 2, 3, 4, 5]);
//             write.send(message).await.unwrap();
//         }
//     });

//     while let Some(Ok(message)) = read.next().await {
//         println!("recieved: {:?}", message);
//     }
// }

async fn idle_stream(mut stream: WebSocketStream<Upgraded>) {
    println!("idling stream");

    while let Some(Ok(message)) = stream.next().await {
        println!("Message: {:?}", message);
    }
}

/// Our server HTTP handler that initiates HTTP upgrades.
async fn ws_listener(req: Request<Body>) -> http::Result<Response<Body>> {
    println!("{:?}", req);

    let (res, fut_opt) = match create_ws(req).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error creating WS stream: {:?}", e);

            let mut res = Response::new(Body::empty());
            *res.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
            return Ok(res);
        }
    };

    if let Some(create_stream_fut) = fut_opt {
        tokio::task::spawn(async move {
            if let Ok(Ok(stream)) = create_stream_fut.await {
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

    let server = hyper::Server::bind(&addr).serve(make_service);

    // We need the assigned address for the client to send it messages.
    let addr = server.local_addr();
    debug!("Listening on: {:?}", addr);

    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}

// #[tokio::main]
// async fn main() {
//     env_logger::try_init().unwrap();

//     let addr = ([127, 0, 0, 1], 0).into();
//     let make_service =
//         make_service_fn(|_| async { Ok::<_, hyper::Error>(service_fn(ws_listener)) });

//     let server = Server::bind(&addr).serve(make_service);

//     println!("Bound server");

//     // We need the assigned address for the client to send it messages.
//     let addr = server.local_addr();
//     println!("Addr: {:?}", addr);

//     println!("Running server");
//     if let Err(e) = server.await {
//         eprintln!("server error: {}", e);
//     }
// }
