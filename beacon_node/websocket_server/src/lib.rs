use futures::future::TryFutureExt;
use slog::{debug, error, info, warn, Logger};
use std::marker::PhantomData;
use std::net::SocketAddr;
use tokio::runtime::Handle;
use types::EthSpec;
use ws::{Sender, WebSocket};

mod config;

pub use config::Config;

pub struct WebSocketSender<T: EthSpec> {
    sender: Option<Sender>,
    _phantom: PhantomData<T>,
}

impl<T: EthSpec> WebSocketSender<T> {
    /// Creates a dummy websocket server that never starts and where all future calls are no-ops.
    pub fn dummy() -> Self {
        Self {
            sender: None,
            _phantom: PhantomData,
        }
    }

    pub fn send_string(&self, string: String) -> Result<(), String> {
        if let Some(sender) = &self.sender {
            sender
                .send(string)
                .map_err(|e| format!("Unable to broadcast to websocket clients: {:?}", e))
        } else {
            Ok(())
        }
    }
}

pub fn start_server<T: EthSpec>(
    config: &Config,
    handle: &Handle,
    log: &Logger,
) -> Result<
    (
        WebSocketSender<T>,
        tokio::sync::oneshot::Sender<()>,
        SocketAddr,
    ),
    String,
> {
    let server_string = format!("{}:{}", config.listen_address, config.port);

    // Create a server that simply ignores any incoming messages.
    let server = WebSocket::new(|_| |_| Ok(()))
        .map_err(|e| format!("Failed to initialize websocket server: {:?}", e))?
        .bind(server_string.clone())
        .map_err(|e| {
            format!(
                "Failed to bind websocket server to {}: {:?}",
                server_string, e
            )
        })?;

    let actual_listen_addr = server.local_addr().map_err(|e| {
        format!(
            "Failed to read listening addr from websocket server: {:?}",
            e
        )
    })?;

    let broadcaster = server.broadcaster();

    // Produce a signal/channel that can gracefully shutdown the websocket server.
    let exit_channel = {
        let (exit_channel, exit) = tokio::sync::oneshot::channel();

        let log_inner = log.clone();
        let broadcaster_inner = server.broadcaster();
        let exit_future = exit
            .and_then(move |_| {
                if let Err(e) = broadcaster_inner.shutdown() {
                    warn!(
                        log_inner,
                        "Websocket server errored on shutdown";
                        "error" => format!("{:?}", e)
                    );
                } else {
                    info!(log_inner, "Websocket server shutdown");
                }
                futures::future::ok(())
            })
            .map_err(|_| ());

        // Place a future on the handle that will shutdown the websocket server when the
        // application exits.
        // TODO: check if we should spawn using a `Handle` or using `task::spawn`
        handle.spawn(exit_future);

        exit_channel
    };

    let log_inner = log.clone();
    // TODO: using tokio `spawn_blocking` instead of `thread::spawn`
    // Check which is more apt.
    let _handle = tokio::task::spawn_blocking(move || match server.run() {
        Ok(_) => {
            debug!(
                log_inner,
                "Websocket server thread stopped";
            );
        }
        Err(e) => {
            error!(
                log_inner,
                "Websocket server failed to start";
                "error" => format!("{:?}", e)
            );
        }
    });

    info!(
        log,
        "WebSocket server started";
        "address" => format!("{}", actual_listen_addr.ip()),
        "port" => actual_listen_addr.port(),
    );

    Ok((
        WebSocketSender {
            sender: Some(broadcaster),
            _phantom: PhantomData,
        },
        exit_channel,
        actual_listen_addr,
    ))
}
