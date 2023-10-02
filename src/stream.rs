
use tokio::sync::mpsc::{Receiver, Sender};
use std::fmt::Debug;
use crate::event::Event;

pub trait InitStream<M: Send + Clone + Debug + Default> {
    fn init_stream(
        &mut self,
        reserved: usize,
        connection_id: usize,
        stream: tokio::net::TcpStream,
        tx: Sender<Event<M>>,
    );
}

pub trait InitStreamFactory<M: Send + Clone + Debug>: Send {
    fn new_stream_initializer(&self,
        buffer_sender: Sender<(usize, M)>,
        receiver: Receiver<Result<Vec<u8>, std::io::Error>>,
        connection_id: usize,
        reserved: usize
    ) -> Box<dyn InitStream<M> + Send>;
}