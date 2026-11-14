//! Shutdown causes: the [`ShutdownHandle`] and the OS signals.
//!
//! One [`Signals`] set is installed just before the services start (step 5),
//! used by the step-7 wait and moved into step 8, so a signal at any point
//! from the first `start` to the end of the stop sequence is seen. It is only
//! installed when there is a service. Note that Tokio installs its
//! process-wide OS handler the first time a listener is created and never
//! removes it: after that, a Ctrl-C or SIGTERM that arrives while no listener
//! exists (before step 5 or after step 8) no longer terminates the process and
//! is not replayed to a listener created later; it is ignored.

use crate::shutdown::ShutdownHandle;

/// What ended a wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cause {
    /// [`ShutdownHandle::trigger`].
    Handle,
    /// Ctrl-C (SIGINT on Unix).
    CtrlC,
    /// SIGTERM, Unix only.
    #[cfg_attr(not(unix), allow(dead_code))]
    Terminate,
}

impl Cause {
    /// The exit code after a forced second signal of this kind.
    pub(crate) fn forced_exit_code(self) -> i32 {
        match self {
            // 128 + SIGINT, 128 + SIGTERM, as a shell reports them.
            Cause::CtrlC => 130,
            Cause::Terminate => 143,
            // `second_signal` never yields `Handle`; listed for exhaustiveness.
            Cause::Handle => 0,
        }
    }
}

/// The OS signal listeners. A listener that cannot be installed is `None`
/// and never fires; the other sources keep working. `recv` can be called
/// repeatedly on the same set: each call waits for the next signal.
pub(crate) struct Signals {
    #[cfg(unix)]
    interrupt: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
    #[cfg(not(unix))]
    ctrl_c: Option<tokio::signal::windows::CtrlC>,
}

impl Signals {
    /// Installs the listeners now, so a signal from this point on is seen.
    pub(crate) fn install() -> Self {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            // `tokio::signal::ctrl_c` listens for SIGINT the same way.
            Self {
                interrupt: signal(SignalKind::interrupt()).ok(),
                terminate: signal(SignalKind::terminate()).ok(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                ctrl_c: tokio::signal::windows::ctrl_c().ok(),
            }
        }
    }

    /// Waits for the next signal; pending forever when no listener exists.
    async fn recv(&mut self) -> Cause {
        #[cfg(unix)]
        {
            async fn next(signal: Option<&mut tokio::signal::unix::Signal>) {
                match signal {
                    Some(signal) => {
                        signal.recv().await;
                    }
                    None => std::future::pending().await,
                }
            }
            tokio::select! {
                () = next(self.interrupt.as_mut()) => Cause::CtrlC,
                () = next(self.terminate.as_mut()) => Cause::Terminate,
            }
        }
        #[cfg(not(unix))]
        {
            match self.ctrl_c.as_mut() {
                Some(ctrl_c) => {
                    ctrl_c.recv().await;
                    Cause::CtrlC
                }
                None => std::future::pending().await,
            }
        }
    }
}

/// Step 7: waits for the first of [`ShutdownHandle::trigger`], Ctrl-C and, on
/// Unix, SIGTERM, listening on `signals`. The handle wins when both are ready.
pub(crate) async fn wait_for_shutdown(handle: &ShutdownHandle, signals: &mut Signals) -> Cause {
    tokio::select! {
        biased;
        () = handle.wait() => Cause::Handle,
        cause = signals.recv() => cause,
    }
}

/// Step 8: waits for the next Ctrl-C or (Unix) SIGTERM on `signals`, the set
/// used in step 7, so a signal after the step-7 wake is not missed. Pending
/// forever when no listener could be installed.
pub(crate) async fn second_signal(mut signals: Signals) -> Cause {
    signals.recv().await
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    fn kill(flag: &str) {
        let status = std::process::Command::new("kill")
            .args([flag, &std::process::id().to_string()])
            .status()
            .expect("run kill");
        assert!(status.success());
    }

    /// A signal that arrives after the step-7 wake but before step 8 polls its
    /// future is still delivered to the second wait. The set is installed
    /// before the `kill`, so Tokio's handler catches it and the test process
    /// survives; no other unit test in this binary listens for signals.
    #[tokio::test]
    async fn signal_between_steps_is_not_lost() {
        let handle = ShutdownHandle::new();
        let mut signals = Signals::install();
        handle.trigger();
        assert_eq!(
            wait_for_shutdown(&handle, &mut signals).await,
            Cause::Handle
        );
        // Between the steps: the signal arrives and Tokio's driver runs before
        // the second wait is polled.
        kill("-TERM");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let cause = tokio::time::timeout(Duration::from_secs(2), second_signal(signals))
            .await
            .expect("signal between steps was lost");
        assert_eq!(cause, Cause::Terminate);
    }
}
