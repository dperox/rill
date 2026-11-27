//! Run each plaintext worker's socket readiness and local tasks on the same thread.
use super::{Transport, spawn_socket};
use axum::Router;
use futures_util::{StreamExt, stream::FuturesUnordered};
use http_kit_io::{FromStd, Listener, tokio::Tokio};
use std::{io, net::TcpListener};
use tokio::{
    runtime::{Builder, LocalRuntime},
    sync::oneshot,
};

#[derive(Clone, Copy)]
pub struct WorkerReactors {
    workers: usize,
    blocking_threads: usize,
}

impl WorkerReactors {
    pub fn new(workers: usize, blocking_threads: usize) -> Self {
        Self {
            workers,
            blocking_threads,
        }
    }

    pub fn runtime(self, index: usize) -> io::Result<LocalRuntime> {
        // Split the process-wide filesystem/DNS budget, including the remainder.
        let limit = self.blocking_threads / self.workers
            + usize::from(index < self.blocking_threads % self.workers);
        Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(limit)
            .build_local(Default::default())
    }

    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        app: Router,
        transport: Transport,
    ) -> io::Result<()> {
        let listener = listener.into_std()?;
        let mut finished = FuturesUnordered::new();
        let mut shutdown = Vec::new();
        for index in 1..self.workers {
            let listener = listener.try_clone()?;
            let app = app.clone();
            let (stop, stopped) = oneshot::channel::<()>();
            let (result, done) = oneshot::channel();
            std::thread::Builder::new()
                .name(format!("rill-h1-{index}"))
                .spawn(move || {
                    let outcome = self.runtime(index).and_then(|runtime| {
                        runtime.block_on(async move {
                            tokio::select! {
                                result = accept(listener, app, transport) => result,
                                _ = stopped => Ok(()),
                            }
                        })
                    });
                    let _ = result.send(outcome);
                })?;
            shutdown.push(stop);
            finished.push(done);
        }
        // Returning (including cancellation) drops the stop senders. A failed
        // worker propagates its error even if no further connections arrive.
        tokio::select! {
            result = accept(listener, app, transport) => result,
            Some(result) = finished.next() => result.map_err(io::Error::other)?,
        }
    }
}

async fn accept(listener: TcpListener, app: Router, transport: Transport) -> io::Result<()> {
    let listener = Tokio.tcp_listener(listener)?;
    loop {
        let (stream, peer) = listener.accept().await?;
        spawn_socket(stream, peer, app.clone(), transport);
    }
}
