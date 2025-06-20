
use tokio::sync::mpsc::{Receiver, Sender};
use std::fmt::Debug;
use crate::event::Event;

pub trait StreamProcessor<M: Send + Clone + Debug + Default> {
    fn setup(
        &mut self,
        reserved: usize,
        connection_id: usize,
        stream: tokio::net::TcpStream,
        tx: Sender<Event<M>>,
    );
}

pub trait StreamProcessorFactory<M: Send + Clone + Debug>: Send {
    fn build(&self,
        source_to_sink: Sender<(usize, M)>,
        receiver: Receiver<Result<Vec<u8>, std::io::Error>>,
        connection_id: usize,
        reserved: usize
    ) -> Box<dyn StreamProcessor<M> + Send>;
}