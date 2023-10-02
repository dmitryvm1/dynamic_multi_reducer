//! 
//!  src1  src2  src3  src4    src5 src6  src7    
//!    __   __   __   __        __   __   __ 
//!   |  | |  | |  | |  |      |  | |  | |  |   ....
//!    ^^   ^^   ^^   ^^        ^^   ^^   ^^
//!      \   |    |  _/          \    |    /
//!       \  |   / /              \   |   /
//!        \ |  / /                \  |  /      ....
//!         \| / /                  \ | /
//!          || /                    \|/ 
//!          __                      __
//!         |  |                    |  |        ....
//!          ^^                      ^^
//!         sink1                   sink2
//! 
//!  Streams data from multiple sources to sink1 and sink2 and backwards,
//!  Sources and sinks can be added on the fly

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    }, marker::PhantomData
};
use log::info;

use futures::Future;
use tokio::sync::mpsc::{Receiver, Sender};
use std::fmt::Debug;
use crate::{event::Event, stream::InitStreamFactory, connect::Connector};

#[derive(Default)]
pub struct InnerMap<M: Sized + Default + Clone + Send + Debug, T: PartialEq + Clone + EventSender<M>> {
    pub map: Mutex<HashMap<usize, T>>,
    p: PhantomData<M>
}

impl<M: Sized + Default + Clone + Send + Debug, T: PartialEq + Clone + EventSender<M>> InnerMap<M, T> {
    pub fn new() -> Self {
        InnerMap {
            map: Mutex::new(HashMap::new()),
            p: PhantomData
        }
    }
    pub fn add_source(&self, wss: T, connection_id: usize) {
        let mut locked = self.map.lock().unwrap();
        locked.insert(connection_id, wss);
    }

    pub fn remove_source(&self, connection_id: usize) {
        let mut locked = self.map.lock().unwrap();
        locked.remove(&connection_id);
    }

    pub fn add_sink(&self, _wss: T) {
        // TODO: This changes when working with websockets
        unimplemented!()
    }

    fn remove_sink<C>(&self, wss: T, cb: C) 
        where C: Fn(usize)
    {
        let mut locked = self.map.lock().unwrap();
        let mut remove = Vec::new();
        for (c, addr) in locked.iter() {
            if wss == *addr {
                remove.push(*c);
            }
        }
        for c in remove {
            cb(c);
            locked.remove(&c);
        }
    }

    pub fn by_connection_id(&self, connection_id: usize) -> Option<T> {
        let locked = self.map.lock().unwrap();
        let addr = locked.get(&connection_id);
        addr.cloned()
    }
}
pub struct DynamicMultiReducer<M: Sized + Default + Debug + Clone + Send, SourceSinkAddr: PartialEq + Clone + EventSender<M> > {
    pub map: Arc<InnerMap<M, SourceSinkAddr>>,
    event_rx: Mutex<Option<Receiver<Event<M>>>>,
    event_tx: Sender<Event<M>>,
    next_connection_id: AtomicUsize,
}

impl<M: Sized + Default + Debug + Clone + Send + 'static, SourceSinkAddr: PartialEq + Clone + EventSender<M>> DynamicMultiReducer<M, SourceSinkAddr> {
    pub fn future(&self, source_sink_factory: fn(usize, SourceSinkAddr) -> Box<dyn SourceSink<M>>,
    stream_init_factory: Box<dyn InitStreamFactory<M>>
) -> DynamicMultiReducerFuture<M, SourceSinkAddr> {
        DynamicMultiReducerFuture {
            streams: Default::default(),
            map: self.map.clone(),
            event_tx: self.event_tx.clone(),
            source_sink_factory,
            stream_init_factory,
            event_rx: self
                .event_rx
                .lock()
                .unwrap()
                .take()
                .expect("future was already called"),
        }
    }
    pub fn new() -> Self {
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(256);
        DynamicMultiReducer {
            event_tx,
            event_rx: Mutex::new(Some(event_rx)),
            next_connection_id: AtomicUsize::new(1),
            map: Arc::new(InnerMap::<M, SourceSinkAddr>::new()),
        }
    }

    pub fn remove_sink(&self, wss: SourceSinkAddr) {
        let tx = self.event_tx.clone();
        self.map.remove_sink(wss, move |c| {
            info!("Removing connection: {}", c);
            tx.try_send(Event::Disconnect { connection_id: c }).unwrap_or_default();
        })
    }

    pub fn shutdown_source(&self, connection_id: usize) {
        self.event_tx.try_send(Event::Disconnect { connection_id }).unwrap_or_default();
    }

    pub fn connect(&self, reserved: usize, server_address: String, proxy: Option<String>, addr: SourceSinkAddr) -> usize {
        let connection_id = self.next_connection_id.load(Ordering::SeqCst);
        self.map.add_source(addr, connection_id);
        Connector::connect(reserved, server_address, proxy, connection_id, self.event_tx.clone());
        self.next_connection_id.fetch_add(1, Ordering::SeqCst);
        
        connection_id
    }
}

pub struct DynamicMultiReducerFuture<M: Sized  + Debug + Default + Clone + Send, SourceSinkAddr: PartialEq + Clone + EventSender<M>> {
    map: Arc<InnerMap<M, SourceSinkAddr>>,
    streams: HashMap<usize, std::net::TcpStream>,
    event_rx: Receiver<Event<M>>,
    event_tx: Sender<Event<M>>,
    stream_init_factory: Box<dyn InitStreamFactory<M>>,
    source_sink_factory: fn(usize, SourceSinkAddr) -> Box<dyn SourceSink<M>>
}

impl<M: Sized + Default + Clone + Send + Debug + 'static, SourceSinkAddr: PartialEq + Clone + EventSender<M> + 'static> Future for DynamicMultiReducerFuture<M, SourceSinkAddr> {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match self.event_rx.poll_recv(cx) {
            std::task::Poll::Ready(Some(e)) => {
                match e {
                    Event::ConnectionFailed { connection_id } => {
                        self.map.remove_source(connection_id);
                    }
                    Event::Disconnect {connection_id} => {
                        if let Some(stream) = self.streams.get(&connection_id) {
                            match stream.shutdown(std::net::Shutdown::Both) {
                                Ok(_)=>{}
                                Err(err) => {
                                    info!("Error shutting down stream: {}", err);
                                }
                            }
                        }
                    }
                    Event::Connected {
                        reserved,
                        connection_id,
                        stream,
                    } => {
                        info!("Connected, {}", connection_id);
                        let (data_tx, data_rx) = tokio::sync::mpsc::channel(128);
                        let mut addr = self.map.by_connection_id(connection_id).expect("no such connection");
                        let mut source_sink = (self.source_sink_factory)(reserved, addr.clone());
                        source_sink.set_receiver(data_rx);
                        tokio::spawn(source_sink);
                        let (sender_tx, sender_rx) = tokio::sync::mpsc::channel(64);
                        addr.send(Event::Connected2 {
                            reserved,
                            sender: sender_tx,
                            connection_id,
                        });
                        self.streams.insert(connection_id, stream.try_clone().unwrap());
                        self.stream_init_factory.new_stream_initializer(data_tx, sender_rx, connection_id, reserved).init_stream
                        (
                            reserved,
                            connection_id,
                            tokio::net::TcpStream::from_std(stream).unwrap(),
                            self.event_tx.clone(),
                        )
                    }
                    Event::Disconnected { connection_id } => {
                        self.streams.remove(&connection_id);
                        info!("Disconnected: {}", connection_id);
                        self.map.remove_source(connection_id);
                    }
                    _ => {}
                }
                // Keep polling event_rx
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            std::task::Poll::Ready(None) => std::task::Poll::Ready(()),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

pub trait SourceSink<M: Sized + Default + Clone + Send>: Future<Output = ()> + Send + Unpin {
    fn set_receiver(&mut self, receiver: Receiver<(usize, M)>);
}


pub trait EventSender<M: Sized + Default + Clone + Send> {
    fn send(&mut self, event: Event<M>);
}

