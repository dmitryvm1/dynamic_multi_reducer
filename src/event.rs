use tokio::sync::mpsc::Sender;

#[derive(Debug)]
pub enum Event<M> {
    /// Send data
    Send { connection_id: usize, bytes: Vec<u8> },
    Disconnect {connection_id: usize},
    Connected { reserved: usize, connection_id: usize, stream: std::net::TcpStream},
    /// Duplicate of Event::Connected, but without stream
    Connected2 { reserved: usize, connection_id: usize, sender: Sender<Result<Vec<u8>, std::io::Error>> },
    ConnectionFailed { connection_id: usize },
    Raw { connection_id: usize, bytes: Vec<u8> },
    Message { reserved: usize, connection_id: usize, msg: M },
    Disconnected {connection_id: usize }
}