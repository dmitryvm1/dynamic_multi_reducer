use dynamic_multi_reducer::{
    event::Event,
    reducer::{DynamicMultiReducer, EventEmitter, Sink},
    stream::{StreamProcessor, StreamProcessorFactory},
};
use futures::{Future, SinkExt, StreamExt, TryFutureExt};
use std::fmt::Debug;
use std::sync::Arc;
use tokio::{sync::mpsc::{Receiver, Sender}, net::tcp::{OwnedWriteHalf, OwnedReadHalf}};
use tokio_stream::wrappers::ReceiverStream;

use tokio_util::{
    codec::{Decoder, FramedRead, FramedWrite},
    sync::PollSender,
};

#[derive(Clone, Debug, PartialEq, Default)]
pub enum MyMessage {
    #[default]
    Data1,
    Data2,
}

#[tokio::main]
async fn main() {
    let dynamic_multi_reducer = Arc::new(DynamicMultiReducer::new());
    // dynamic_multi_reducer.connect(reserved, server_address, proxy, addr);
    tokio::spawn(dynamic_multi_reducer.future(
        |reserved, _addr: AddrWrap| {
            // Addr is what we specify in dynamic_multi_reducer.connect
            Box::new(ReceiverToUi { receiver: None })
        }, Box::new(StreamFactory {}),
    ));
}

#[derive(Clone)]
pub struct AddrWrap {
    id: usize,
    sender: Sender<Event<MyMessage>>,
}

impl PartialEq for AddrWrap {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl EventEmitter<MyMessage> for AddrWrap {
    fn emit(&mut self, event: Event<MyMessage>) {
        self.sender.try_send(event).unwrap();
    }
}

pub struct ReceiverToUi {
    receiver: Option<Receiver<(usize, MyMessage)>>,
}
impl Sink<MyMessage> for ReceiverToUi {
    fn set_receiver(&mut self, receiver: Receiver<(usize, MyMessage)>) {
        self.receiver = Some(receiver);
    }
}

impl Future for ReceiverToUi {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match self.receiver.as_mut().unwrap().poll_recv(cx) {
            std::task::Poll::Ready(item) => {
                match item {
                    Some(inner) => {
                        match inner.1 {
                            MyMessage::Data1 => {}
                            MyMessage::Data2 => {}
                        }
                        // Keep polling the receiver
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    }
                    None => std::task::Poll::Ready(()),
                }
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}
pub struct StreamFactory {}
impl StreamProcessorFactory<MyMessage> for StreamFactory {
    fn build(
        &self,
        buffer_sender: Sender<(usize, MyMessage)>,
        receiver: Receiver<Result<Vec<u8>, std::io::Error>>,
        _connection_id: usize,
        _reserved: usize
    ) -> Box<dyn StreamProcessor<MyMessage> + Send> {
        // TODO: get encoding associated with reserved
        Box::new(StreamInitiazer::new(buffer_sender, String::new(), receiver))
    }
}

/// Wires reader half and writer half of TcpStream with receiver and sender
/// and spawns futures for both of them
#[derive(Debug)]
pub struct StreamInitiazer<MyMessage> {
    /// Sending part of a source to sink channel
    source_to_sink: Sender<(usize, MyMessage)>,
    encoding: String,
    receiver: Option<Receiver<Result<Vec<u8>, std::io::Error>>>,
}

impl StreamInitiazer<MyMessage> {
    pub fn new(
        // (connection_id, message)
        // This is a sender to whatever receives the message
        // It can be script or console log, or ui thing
        buffer_sender: Sender<(usize, MyMessage)>,
        encoding: String,
        receiver: Receiver<Result<Vec<u8>, std::io::Error>>,
    ) -> Self {
        StreamInitiazer {
            source_to_sink: buffer_sender,
            encoding,
            receiver: Some(receiver),
        }
    }

    fn spawn_writer(&mut self, writer: OwnedWriteHalf) {
        let writer = FramedWrite::new(writer, CommandEncoder::default());
        let stream_receiver = ReceiverStream::new(self.receiver.take().unwrap());
        let writer_fut = stream_receiver.forward(writer);
        tokio::spawn(writer_fut);
    }
    
    // Set's up a chain of futures to read from a socket stream and send corresponsing events
    // using reducer_event_sender
    fn spawn_reader1(&mut self, reader: OwnedReadHalf, source_id: usize,
        reducer_event_sender: Sender<Event<MyMessage>>) {
            let receiver_fut = PollSender::new(self.source_to_sink.clone());
            let framed_reader = FramedRead::new(reader, MessageCodec::new(self.encoding.clone()));
            let cm_tx = reducer_event_sender.clone();
            let fut =
                framed_reader
                    .map(move |buf| match buf {
                        Ok(b) => Ok((source_id, b)),
                        Err(err) => {
                            cm_tx
                                .try_send(Event::Disconnected { source_id })
                                .unwrap();
                            Err(err)
                        }
                    })
                    .forward(receiver_fut.sink_map_err(|_| {
                        std::io::Error::new(std::io::ErrorKind::Other, "error sending")
                    }));
            tokio::spawn(fut.and_then(move |_| {
                reducer_event_sender.try_send(Event::Disconnected { source_id }).unwrap();
                futures::future::ok(())
            }));
    }

    // Same but with another codec that returns same message type
    fn spawn_reader2(&mut self, reader: OwnedReadHalf, source_id: usize,
        reducer_event_sender: Sender<Event<MyMessage>>) {
            let receiver_fut = PollSender::new(self.source_to_sink.clone());
            let framed_reader = FramedRead::new(reader, MessageCodec2::new(self.encoding.clone()));
            let cm_tx = reducer_event_sender.clone();
            let fut =
                framed_reader
                    .map(move |buf| match buf {
                        Ok(b) => Ok((source_id, b)),
                        Err(err) => {
                            cm_tx
                                .try_send(Event::Disconnected { source_id })
                                .unwrap();
                            Err(err)
                        }
                    })
                    .forward(receiver_fut.sink_map_err(|_| {
                        std::io::Error::new(std::io::ErrorKind::Other, "error sending")
                    }));
            tokio::spawn(fut.and_then(move |_| {
                reducer_event_sender.try_send(Event::Disconnected { source_id }).unwrap();
                futures::future::ok(())
            }));
    }
}

impl StreamProcessor<MyMessage> for StreamInitiazer<MyMessage> {
    fn setup(
        &mut self,
        reserved: usize,
        source_id: usize,
        stream: tokio::net::TcpStream,
        // This is a one unique channel for events from reducer
        reducer_event_emitter: Sender<Event<MyMessage>>,
    ) {
        let (reader, writer) = stream.into_split();
        self.spawn_writer(writer);
        if reserved == 1 {
            self.spawn_reader1(reader, source_id, reducer_event_emitter);
        } else {
            self.spawn_reader2(reader, source_id, reducer_event_emitter);
        }
    }
}

use tokio_util::codec::Encoder;

#[derive(Default)]
pub struct CommandEncoder {
    encoding: String,
}

impl Encoder<Vec<u8>> for CommandEncoder {
    type Error = std::io::Error;

    fn encode(&mut self, item: Vec<u8>, dst: &mut bytes::BytesMut) -> Result<(), Self::Error> {
        dst.extend_from_slice(item.as_slice());
        // TODO: add encoding
        Ok(())
    }
}

pub struct MessageCodec {
    encoding: String,
}

impl MessageCodec {
    pub fn new(encoding: String) -> Self {
        MessageCodec { encoding }
    }
}

impl<'a> Decoder for MessageCodec {
    type Item = MyMessage;

    type Error = std::io::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        Ok(None)
    }
}

pub struct MessageCodec2 {
    encoding: String,
}

impl MessageCodec2 {
    pub fn new(encoding: String) -> Self {
        MessageCodec2 { encoding }
    }
}

impl<'a> Decoder for MessageCodec2 {
    type Item = MyMessage;

    type Error = std::io::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        Ok(None)
    }
}
