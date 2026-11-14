use std::sync::Arc;

use async_trait::async_trait;
use tokio::time::Instant;

use crate::error::ServiceError;
use crate::output::Output;

/// A long-running part of the application, such as a server or a worker.
///
/// Register it as a bean that `provides` `dyn Service`. After the runners, boot
/// starts every service in ascending [`phase`](Service::phase), ties in DI order
/// (`order`, then registration order). With at least one service started, `run()`
/// waits for shutdown, then stops the services in reverse start order. The whole
/// stop sequence shares one deadline, `shutdown_timeout`: a `stop` still running
/// at the deadline is abandoned and named in the output. A start error stops the
/// services started so far and fails the run in the `Service` phase. Implement it
/// with [`async_trait`](crate::async_trait).
///
/// A panic in `start` is not caught: it propagates out of `run()` like any
/// panic in user code, and the services started before it are not stopped.
/// Whenever a service is registered, boot needs a Tokio runtime with the time
/// and IO drivers, such as `#[tokio::main]` or a builder with `enable_all()`:
/// it installs signal handlers just before the first `start`, spawns each
/// `stop` and bounds it with a timer.
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use oxbloom_boot::async_trait;
/// use oxbloom_boot::{OxbloomApplication, Service, ServiceError};
/// use oxbloom_config::Config;
///
/// struct Worker {
///     running: Arc<AtomicBool>,
/// }
///
/// #[async_trait]
/// impl Service for Worker {
///     async fn start(&self) -> Result<(), ServiceError> {
///         self.running.store(true, Ordering::SeqCst);
///         Ok(())
///     }
///
///     async fn stop(&self) {
///         self.running.store(false, Ordering::SeqCst);
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let running = Arc::new(AtomicBool::new(false));
/// let flag = running.clone();
/// let builder = OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .output(std::io::sink())
///     .configure(move |b| {
///         b.instance(Worker { running: flag })
///             .provides(|w: Arc<Worker>| -> Arc<dyn Service> { w })
///     });
/// // Ends the run without a signal. The trigger is remembered, so `run()`
/// // starts the worker and stops it again right away.
/// builder.shutdown_handle().trigger();
/// let status = builder.run().await?;
/// assert_eq!(status.code(), 0);
/// assert!(!running.load(Ordering::SeqCst));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[async_trait]
pub trait Service: Send + Sync + 'static {
    /// Starts the service. Return once it runs; spawn long-running work.
    ///
    /// ```
    /// use oxbloom_boot::async_trait;
    /// use oxbloom_boot::{Service, ServiceError};
    ///
    /// struct Port(u16);
    ///
    /// #[async_trait]
    /// impl Service for Port {
    ///     async fn start(&self) -> Result<(), ServiceError> {
    ///         if self.0 == 0 {
    ///             return Err(ServiceError::new("no port configured"));
    ///         }
    ///         Ok(())
    ///     }
    ///     async fn stop(&self) {}
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// assert!(Port(8080).start().await.is_ok());
    /// assert_eq!(Port(0).start().await.unwrap_err().to_string(), "no port configured");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    async fn start(&self) -> Result<(), ServiceError>;

    /// Stops the service. Boot runs it in its own task and abandons it at the
    /// shutdown deadline; a panic is reported and the next service still stops.
    ///
    /// ```
    /// use std::sync::atomic::{AtomicBool, Ordering};
    /// use oxbloom_boot::async_trait;
    /// use oxbloom_boot::{Service, ServiceError};
    ///
    /// #[derive(Default)]
    /// struct Flag(AtomicBool);
    ///
    /// #[async_trait]
    /// impl Service for Flag {
    ///     async fn start(&self) -> Result<(), ServiceError> {
    ///         Ok(())
    ///     }
    ///     async fn stop(&self) {
    ///         self.0.store(true, Ordering::SeqCst);
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let flag = Flag::default();
    /// flag.stop().await;
    /// assert!(flag.0.load(Ordering::SeqCst));
    /// # });
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    async fn stop(&self);

    /// The start position: lower phases start first and stop last. Default: 0.
    ///
    /// ```
    /// use oxbloom_boot::async_trait;
    /// use oxbloom_boot::{Service, ServiceError};
    ///
    /// struct Server;
    ///
    /// #[async_trait]
    /// impl Service for Server {
    ///     async fn start(&self) -> Result<(), ServiceError> {
    ///         Ok(())
    ///     }
    ///     async fn stop(&self) {}
    ///     // Start after everything else, stop first.
    ///     fn phase(&self) -> i32 {
    ///         i32::MAX
    ///     }
    /// }
    ///
    /// assert_eq!(Server.phase(), i32::MAX);
    /// ```
    fn phase(&self) -> i32 {
        0
    }

    /// The name used in diagnostics. Default: the type name.
    ///
    /// ```
    /// use oxbloom_boot::async_trait;
    /// use oxbloom_boot::{Service, ServiceError};
    ///
    /// struct Cache;
    ///
    /// #[async_trait]
    /// impl Service for Cache {
    ///     async fn start(&self) -> Result<(), ServiceError> {
    ///         Ok(())
    ///     }
    ///     async fn stop(&self) {}
    /// }
    ///
    /// assert!(Cache.name().ends_with("Cache"));
    /// ```
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// Starts `services` in ascending phase, ties in the given (DI) order.
///
/// Returns the started services in start order. On a start error, returns the
/// services started before the failing one together with the error.
pub(crate) async fn start_all(
    services: &[Arc<dyn Service>],
    out: &Output,
) -> Result<Vec<Arc<dyn Service>>, (Vec<Arc<dyn Service>>, ServiceError)> {
    let mut ordered = services.to_vec();
    // `sort_by_key` is stable: equal phases keep the DI order.
    ordered.sort_by_key(|s| s.phase());
    let mut started = Vec::with_capacity(ordered.len());
    for service in ordered {
        if let Err(error) = service.start().await {
            out.line(&format!("service {} failed to start", service.name()));
            return Err((started, error));
        }
        #[cfg(feature = "logging")]
        crate::events::service_started(service.name());
        started.push(service);
    }
    Ok(started)
}

/// Stops `started` in reverse order, all before one shared `deadline`.
///
/// Each `stop` runs in its own task. One still running at the deadline is
/// aborted, a panicking one is reported; either way the next service stops.
/// A service reached at or after the deadline is not stopped at all, only
/// reported. Dropping the returned future aborts the `stop` in progress.
pub(crate) async fn stop_all(started: Vec<Arc<dyn Service>>, deadline: Instant, out: &Output) {
    for service in started.into_iter().rev() {
        let name = service.name();
        if Instant::now() >= deadline {
            out.line(&format!("service {name} did not stop in time"));
            continue;
        }
        let mut task = AbortOnDrop(tokio::spawn(async move { service.stop().await }));
        match tokio::time::timeout_at(deadline, &mut task.0).await {
            Ok(Ok(())) => {
                #[cfg(feature = "logging")]
                crate::events::service_stopped(name);
            }
            Ok(Err(e)) if e.is_panic() => {
                out.line(&format!("service {name} panicked while stopping"));
            }
            // Cancelled from outside: nothing left to stop.
            Ok(Err(_)) => {}
            Err(_) => {
                task.0.abort();
                out.line(&format!("service {name} did not stop in time"));
            }
        }
    }
}

/// Aborts the task when dropped, so an abandoned `stop` does not keep running.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct Plain;

    #[async_trait]
    impl Service for Plain {
        async fn start(&self) -> Result<(), ServiceError> {
            Ok(())
        }
        async fn stop(&self) {}
    }

    #[test]
    fn defaults() {
        assert_eq!(Plain.phase(), 0);
        assert_eq!(Plain.name(), std::any::type_name::<Plain>());
    }

    #[tokio::test]
    async fn empty_lists_do_nothing() {
        let buf = Buf::default();
        let out = Output::new(buf.clone());
        tokio::time::timeout(Duration::from_secs(2), async {
            assert!(matches!(start_all(&[], &out).await, Ok(s) if s.is_empty()));
            stop_all(Vec::new(), Instant::now(), &out).await;
        })
        .await
        .expect("test timed out");
        assert!(buf.0.lock().unwrap().is_empty());
    }
}
