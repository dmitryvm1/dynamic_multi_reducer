use tokio::sync::mpsc::Sender;

#[derive(Debug)]
pub enum Event<M> {
    /// Data was sent from the source to the sink.
    SourceToSink { source_id: usize, bytes: Vec<u8> },
    /// The source was disconnected.
    Disconnect {source_id: usize},
    /// The source was connected to the target server.
    Connected { reserved: usize, source_id: usize, stream: std::net::TcpStream},
    /// Duplicate of Event::Connected, but without stream.
    Connected2 { reserved: usize, source_id: usize, sink_to_source: Sender<Result<Vec<u8>, std::io::Error>> },
    /// Source failed to connect to the target server.
    ConnectionFailed { source_id: usize, reserved: usize },
    Raw { source_id: usize, bytes: Vec<u8> },
    Message { reserved: usize, source_id: usize, msg: M },
    Disconnected {source_id: usize }
}