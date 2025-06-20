use crate::{error::Error, event::Event};
use fast_socks5::client::Socks5Stream;
use log::info;
use std::{fmt::Debug, net::SocketAddrV4};
use tokio::{net::TcpStream, sync::mpsc::Sender};
#[derive(Clone)]
pub struct Connector {}

impl Connector {
    pub fn new() -> Self {
        Connector {}
    }

    /// A synchronous method that spawns a future to obtain a tcp stream through socks5 proxy or directly
    /// The TcpStream is sent through `event_emitter` on success.
    /// Event::ConnectionFailed is sent on failure.
    pub fn connect<M: Sized + Default + Clone + Send + Debug + 'static>(
        reserved: usize,
        server_address: String,
        proxy_address: Option<String>,
        source_id: usize,
        event_emitter: Sender<Event<M>>,
    ) {
        tokio::spawn(async move {
            let stream = connect_via_proxy_or_directly(&server_address, proxy_address.clone()).await;
            match stream {
                Ok(stream) => {
                    info!("Socket connected to {}", server_address);
                    event_emitter
                        .blocking_send(Event::Connected {
                            reserved,
                            source_id,
                            stream: stream.into_std().unwrap(),
                        })
                        .unwrap();
                }
                Err(_err) => {
                    info!("Failed to connect to {}", server_address);
                    event_emitter
                        .blocking_send(Event::ConnectionFailed {
                            source_id,
                            reserved,
                        })
                        .unwrap();
                }
            }
        });
    }
}


/// Connects to the server through a socks5 proxy if specified or directly.
/// ### Returns
/// TCP stream
async fn connect_via_proxy_or_directly(
    server_address: &str,
    proxy_address: Option<String>,
) -> Result<TcpStream, Error> {
    info!(
        "Connecting to {:?}, using socks5 proxy: {:?}",
        server_address, proxy_address
    );
    let (addr, port) = parse_ipv4_address(server_address)?;
    // Obtain a tcp stream via connecting to proxy or directly to server
    if let Some(proxy_address) = proxy_address {
        let stream = Socks5Stream::connect(proxy_address, addr, port, Default::default()).await?;
        Ok(stream.get_socket())
    } else {
        let stream = TcpStream::connect(&server_address).await;
        Ok(stream?)
    }
}

fn parse_ipv4_address(addr: &str) -> Result<(String, u16), Error> {
    let addr: SocketAddrV4 = addr.parse()?;
    Ok((addr.ip().to_string(), addr.port()))
}
