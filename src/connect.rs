use std::fmt::Debug;
use tokio::{net::TcpStream, sync::mpsc::Sender};
use crate::event::Event;
use log::info;
use fast_socks5::client::Socks5Stream;
#[derive(Clone)]
pub struct Connector {}

impl Connector {
    pub fn new() -> Self {
        Connector {}
    }

    pub fn connect<M: Sized + Default + Clone + Send + Debug + 'static>(
        reserved: usize,
        server_address: String,
        proxy_address: Option<String>,
        connection_id: usize,
        tx: Sender<Event<M>>,
    ) {
        tokio::spawn(async move {
            let stream = connection_stream(&server_address, proxy_address.clone()).await;
            match stream {
                Ok(stream) => {
                    tx.try_send(Event::Connected {
                        reserved,
                        connection_id,
                        stream: stream.into_std().unwrap(),
                    })
                    .unwrap();
                }
                Err(_err) => {
                    tx.try_send(Event::ConnectionFailed { connection_id })
                        .unwrap();
                }
            }
        });
    }
}

async fn connection_stream(
    server_address: &str,
    proxy_address: Option<String>,
) -> Result<TcpStream, Box<dyn std::error::Error>> {
    info!(
        "Connecting to {:?}, using socks5 proxy: {:?}",
        server_address, proxy_address
    );
    let mut parts = server_address.split(":");
    let addr = parts.next()
        .ok_or(std::io::Error::new(std::io::ErrorKind::Other, "addr parse error"))?
        .to_string();
    let port: u16 = parts.next()
        .ok_or(std::io::Error::new(std::io::ErrorKind::Other, "addr parse error"))?
        .parse()?;
    if let Some(proxy_address) = proxy_address {
        let stream = 
            Socks5Stream::connect(proxy_address, addr,
             port, Default::default()).await?;
             Ok(stream.get_socket())
    } else {
        let stream = TcpStream::connect(&server_address).await;
        stream.map_err(|err| err.into())
    }
}
