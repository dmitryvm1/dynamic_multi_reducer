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
use crate::{event::Event, stream::StreamProcessorFactory, connect::Connector};

#[derive(Default)]
pub struct InnerMap<M: Sized + Default + Clone + Send + Debug, T: PartialEq + Clone + EventEmitter<M>> {
    pub map: Mutex<HashMap<usize, T>>,
    pub next_source_id: AtomicUsize,
    p: PhantomData<M>
}

impl<M: Sized + Default + Clone + Send + Debug, T: PartialEq + Clone + EventEmitter<M>> InnerMap<M, T> {
    pub fn new() -> Self {
        InnerMap {
            map: Mutex::new(HashMap::new()),
            p: PhantomData,
            next_source_id: AtomicUsize::new(1)
        }
    }
    pub fn map_source_to_sink(&self, sink_addr: T, source_id: usize) {
        let mut locked = self.map.lock().unwrap();
        locked.insert(source_id, sink_addr);
    }

    pub fn unmap_source_from_sink(&self, source_id: usize) {
        let mut locked = self.map.lock().unwrap();
        locked.remove(&source_id);
    }

    fn remove_sink<C>(&self, sink_addr: T, cb: C) 
        where C: Fn(usize)
    {
        let mut locked = self.map.lock().unwrap();
        let mut remove = Vec::new();
        for (c, addr) in locked.iter() {
            if sink_addr == *addr {
                remove.push(*c);
            }
        }
        for c in remove {
            cb(c);
            locked.remove(&c);
        }
    }

    /// Returns a sink address associated with the given source id.
    pub fn by_source_id(&self, source_id: usize) -> Option<T> {
        let locked = self.map.lock().unwrap();
        let addr = locked.get(&source_id);
        addr.cloned()
    }
}
pub struct DynamicMultiReducer<M: Sized + Default + Debug + Clone + Send, SinkAddr: PartialEq + Clone + EventEmitter<M> > {
    pub map: Arc<InnerMap<M, SinkAddr>>,
    event_rx: Mutex<Option<Receiver<Event<M>>>>,
    event_tx: Sender<Event<M>>,
    next_source_id: AtomicUsize,
}

impl<M: Sized + Default + Debug + Clone + Send + 'static, SinkAddr: PartialEq + Clone + EventEmitter<M>> DynamicMultiReducer<M, SinkAddr> {
    pub fn future(&self, source_sink_factory: fn(usize, SinkAddr) -> Box<dyn Sink<M>>,
    stream_init_factory: Box<dyn StreamProcessorFactory<M>>
) -> DynamicMultiReducerFuture<M, SinkAddr> {
        DynamicMultiReducerFuture {
            streams: Default::default(),
            source_id_to_sink_addr: self.map.clone(),
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
            next_source_id: AtomicUsize::new(1),
            map: Arc::new(InnerMap::<M, SinkAddr>::new()),
        }
    }

    pub fn remove_sink(&self, wss: SinkAddr) {
        let tx = self.event_tx.clone();
        self.map.remove_sink(wss, move |c| {
            info!("Removing connection: {}", c);
            tx.try_send(Event::Disconnect { source_id: c }).unwrap_or_default();
        })
    }

    pub fn shutdown_source(&self, source_id: usize) {
        self.event_tx.try_send(Event::Disconnect { source_id }).unwrap_or_default();
    }

    /// Connects a new source to the sink
    pub fn connect(&self, reserved: usize, server_address: String, proxy: Option<String>, addr: SinkAddr) -> usize {
        let source_id = self.next_source_id.fetch_add(1, Ordering::SeqCst);
        self.map.map_source_to_sink(addr, source_id);
        Connector::connect(reserved, server_address, proxy, source_id, self.event_tx.clone());
        source_id
    }
}

pub struct DynamicMultiReducerFuture<M: Sized  + Debug + Default + Clone + Send, SinkAddr: PartialEq + Clone + EventEmitter<M>> {
    /// Source ID to SinkAddr map.
    source_id_to_sink_addr: Arc<InnerMap<M, SinkAddr>>,
    /// Source ID to TcpStream map.
    streams: HashMap<usize, std::net::TcpStream>,
    /// Inner receiver of network events.
    event_rx: Receiver<Event<M>>,
    /// Inner sender of network events.
    event_tx: Sender<Event<M>>,
    stream_init_factory: Box<dyn StreamProcessorFactory<M>>,
    source_sink_factory: fn(usize, SinkAddr) -> Box<dyn Sink<M>>
}

impl<M: Sized + Default + Clone + Send + Debug + 'static, SinkAddr: PartialEq + Clone + EventEmitter<M> + 'static> Future for DynamicMultiReducerFuture<M, SinkAddr> {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match self.event_rx.poll_recv(cx) {
            std::task::Poll::Ready(Some(e)) => {
                match e {
                    Event::ConnectionFailed { source_id, reserved: _ } => {
                        self.source_id_to_sink_addr.unmap_source_from_sink(source_id);
                    }
                    Event::Disconnect {source_id} => {
                        if let Some(stream) = self.streams.get(&source_id) {
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
                        source_id,
                        stream,
                    } => {
                        info!("Connected, {}", source_id);
                        let (data_tx, data_rx) = tokio::sync::mpsc::channel(512);
                        let mut addr = self.source_id_to_sink_addr.by_source_id(source_id).expect("no such connection");
                        let mut source_sink = (self.source_sink_factory)(reserved, addr.clone());
                        source_sink.set_receiver(data_rx);
                        tokio::spawn(source_sink);
                        let (sink_to_source_tx, sink_to_source_rx) = tokio::sync::mpsc::channel(128);
                        addr.emit(Event::Connected2 {
                            reserved,
                            sink_to_source: sink_to_source_tx,
                            source_id: source_id,
                        });
                        self.streams.insert(source_id, stream.try_clone().unwrap());
                        let event_emitter = self.event_tx.clone();
                        self.stream_init_factory.build(
                            data_tx,
                            sink_to_source_rx,
                            source_id,
                            reserved
                        ).setup(
                            reserved,
                            source_id,
                            tokio::net::TcpStream::from_std(stream).unwrap(),
                            event_emitter,
                        )
                    }
                    Event::Disconnected { source_id } => {
                        self.streams.remove(&source_id);
                        info!("Disconnected: {}", source_id);
                        let mut addr = self.source_id_to_sink_addr.by_source_id(source_id).expect("no such connection");
                        addr.emit(Event::Disconnected { source_id });
                        self.source_id_to_sink_addr.unmap_source_from_sink(source_id);
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

pub trait Sink<M: Sized + Default + Clone + Send>: Future<Output = ()> + Send + Unpin {
    fn set_receiver(&mut self, receiver: Receiver<(usize, M)>);
}

pub trait EventEmitter<M: Sized + Default + Clone + Send> {
    fn emit(&mut self, event: Event<M>);
}

