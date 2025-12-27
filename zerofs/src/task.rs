use std::future::Future;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

pub fn spawn_named<T, F>(_name: &str, future: F) -> JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    // Note: tokio::task::Builder requires tokio_unstable feature
    // Using regular spawn for now
    tokio::spawn(future)
}

pub fn spawn_named_on<T, F>(_name: &str, future: F, handle: &Handle) -> JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    // Note: tokio::task::Builder requires tokio_unstable feature
    // Using regular spawn_on for now
    handle.spawn(future)
}

pub fn spawn_blocking_named<T, F>(_name: &str, f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    // Note: tokio::task::Builder requires tokio_unstable feature
    // Using regular spawn_blocking for now
    tokio::task::spawn_blocking(f)
}
