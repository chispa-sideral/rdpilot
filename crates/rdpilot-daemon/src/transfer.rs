//! The transfer worker: one FIFO queue per live session generation that runs
//! its file transfers (Put, Get) and the Connect bootstrap one at a time.
//!
//! - Every live entry owns one [`TransferQueue`]; its worker task serves that
//!   generation only and ends when the queue is cancelled or dropped.
//! - A caller enqueues a job with [`TransferHandle::run`]. At most
//!   [`TRANSFER_QUEUE_BOUND`] jobs wait; a caller then waits for capacity.
//! - A blocking job (Put, Get) runs on a blocking-pool thread, so its
//!   synchronous local copy and hash never hold a runtime worker. That thread
//!   drives the job on its own current-thread runtime, so the job's timers
//!   do not depend on the daemon runtime, which can shut down while the job
//!   is in a synchronous step. The worker task waits until that thread has
//!   left the job before it starts the next one.
//! - [`TransferQueue::cancel`] (the close of the session) answers every
//!   caller at once with [`DaemonError::SessionClosed`]: the queued job, the
//!   running job and a caller waiting for capacity. A running job stops at
//!   its next await. Effects already made are not rolled back.
//! - A caller that goes away abandons its job: a queued job is skipped and a
//!   running one stops at its next await.
//! - The daemon runtime waits for a blocking job's current synchronous step
//!   (for Get, the whole local tail after the bridge reply) when it shuts
//!   down.

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::seams::{BoxFuture, DaemonError, ManagedSession};

/// Jobs that wait in one generation's queue; the running job is not counted.
pub(crate) const TRANSFER_QUEUE_BOUND: usize = 8;

/// The work of one job. It sends its own typed result to its caller.
type JobRun = Box<dyn FnOnce(Arc<dyn ManagedSession>) -> BoxFuture<'static, ()> + Send>;

/// One queued transfer.
struct Job {
    run: JobRun,
    /// Run on a blocking-pool thread.
    blocking: bool,
    /// Ends (with an error) when the caller goes away.
    abandoned: oneshot::Receiver<()>,
}

/// The queue of one live session generation. Dropping it ends the worker.
pub struct TransferQueue {
    tx: mpsc::Sender<Job>,
    cancel: watch::Sender<bool>,
}

/// What a caller needs to enqueue a job, cloned under the registry lock.
#[derive(Clone)]
pub(crate) struct TransferHandle {
    tx: mpsc::Sender<Job>,
    cancel: watch::Receiver<bool>,
}

impl TransferQueue {
    /// Start the worker of `session` on the current runtime.
    pub(crate) fn start(session: Arc<dyn ManagedSession>) -> Self {
        let (tx, rx) = mpsc::channel(TRANSFER_QUEUE_BOUND);
        let (cancel, cancel_rx) = watch::channel(false);
        tokio::spawn(work(session, rx, cancel_rx));
        TransferQueue { tx, cancel }
    }

    pub(crate) fn handle(&self) -> TransferHandle {
        TransferHandle {
            tx: self.tx.clone(),
            cancel: self.cancel.subscribe(),
        }
    }

    /// Cancel every queued and running job, and every caller waiting for
    /// capacity: each gets [`DaemonError::SessionClosed`] at once. The worker
    /// takes no further job.
    pub(crate) fn cancel(&self) {
        self.cancel.send_replace(true);
    }
}

impl TransferHandle {
    /// Run `op` as the next job of this generation's queue and return its
    /// result.
    ///
    /// # Errors
    ///
    /// [`DaemonError::SessionClosed`] when the queue is cancelled before the
    /// result is delivered (also for an error `op` returns after that), or
    /// no longer takes jobs; [`DaemonError::SessionNotFound`] when the worker
    /// drops an enqueued job without a cancel (the daemon is stopping);
    /// otherwise whatever `op` returns.
    pub(crate) async fn run<T: Send + 'static>(
        mut self,
        session: &str,
        op: impl FnOnce(Arc<dyn ManagedSession>) -> BoxFuture<'static, Result<T, DaemonError>>
            + Send
            + 'static,
        blocking: bool,
    ) -> Result<T, DaemonError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        // Dropped with this future: the worker then skips or stops the job.
        let (_present, abandoned) = oneshot::channel::<()>();
        let job = Job {
            run: Box::new(move |session| {
                Box::pin(async move {
                    let _ = reply_tx.send(op(session).await);
                })
            }),
            blocking,
            abandoned,
        };
        tokio::select! {
            biased;
            () = cancelled(&mut self.cancel) => return Err(DaemonError::SessionClosed),
            sent = self.tx.send(job) => {
                if sent.is_err() {
                    return Err(DaemonError::SessionClosed);
                }
            }
        }
        tokio::select! {
            biased;
            () = cancelled(&mut self.cancel) => Err(DaemonError::SessionClosed),
            reply = reply_rx => match reply {
                Ok(result) => result,
                Err(_) if *self.cancel.borrow() => Err(DaemonError::SessionClosed),
                // The worker ended without a cancel: the entry was dropped
                // without a close, which only the daemon's exit does.
                Err(_) => Err(DaemonError::SessionNotFound(session.to_owned())),
            },
        }
    }
}

/// Resolves once the queue is cancelled; never when its sender is dropped
/// without a cancel.
async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    if cancel.wait_for(|c| *c).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Resolves once the queue is cancelled or its sender is dropped.
async fn stopped(cancel: &mut watch::Receiver<bool>) {
    let _ = cancel.wait_for(|c| *c).await;
}

/// The worker loop of one generation.
async fn work(
    session: Arc<dyn ManagedSession>,
    mut rx: mpsc::Receiver<Job>,
    mut cancel: watch::Receiver<bool>,
) {
    loop {
        let job = tokio::select! {
            biased;
            () = stopped(&mut cancel) => break,
            job = rx.recv() => match job {
                Some(job) => job,
                None => break,
            },
        };
        let Job {
            run,
            blocking,
            mut abandoned,
        } = job;
        if matches!(
            abandoned.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ) {
            continue;
        }
        let session = Arc::clone(&session);
        if blocking {
            // Dropped with this task (cancel or runtime shutdown): the
            // blocking thread then stops the job at its next await.
            let (_alive, worker_gone) = oneshot::channel::<()>();
            let mut job_cancel = cancel.clone();
            let thread = tokio::task::spawn_blocking(move || {
                run_blocking(async move {
                    tokio::select! {
                        biased;
                        () = stopped(&mut job_cancel) => {}
                        _ = worker_gone => {}
                        _ = &mut abandoned => {}
                        () = run(session) => {}
                    }
                });
            });
            // Wait for the thread to leave the job before the next one starts.
            tokio::select! {
                biased;
                () = stopped(&mut cancel) => break,
                _ = thread => {}
            }
        } else {
            tokio::select! {
                biased;
                () = stopped(&mut cancel) => break,
                _ = &mut abandoned => {}
                () = run(session) => {}
            }
        }
    }
    // Queued jobs are dropped with their reply senders; their callers have
    // been answered by the cancel, or see the worker gone.
    rx.close();
    while rx.try_recv().is_ok() {}
}

/// Drive `job` to its end on a current-thread runtime of this blocking
/// thread. Without a runtime the job is dropped, which its caller sees as
/// the worker gone.
fn run_blocking(job: impl std::future::Future<Output = ()>) {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(job),
        Err(error) => eprintln!("rdpilot-daemon: could not start a transfer runtime: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use super::*;
    use crate::op_class_tests::{until, OpSession, Ops, BOUND};

    fn queue() -> (TransferQueue, Arc<Ops>) {
        let ops = Arc::new(Ops::new(0));
        let queue = TransferQueue::start(Arc::new(OpSession(Arc::clone(&ops))));
        (queue, ops)
    }

    /// A job that runs the transfer `label` of the fake session.
    fn job(
        label: &str,
    ) -> impl FnOnce(Arc<dyn ManagedSession>) -> BoxFuture<'static, Result<(), DaemonError>> + Send
    {
        let label = label.to_owned();
        move |s| Box::pin(async move { s.upload_file("unused".into(), label).await.map(drop) })
    }

    fn spawn_run(
        queue: &TransferQueue,
        label: &str,
        blocking: bool,
    ) -> tokio::task::JoinHandle<Result<(), DaemonError>> {
        let handle = queue.handle();
        let job = job(label);
        tokio::spawn(async move { handle.run("web", job, blocking).await })
    }

    #[tokio::test]
    async fn a_full_queue_makes_the_next_caller_wait_for_capacity() {
        let (queue, ops) = queue();
        let mut callers = vec![spawn_run(&queue, "hold-0", false)];
        until("the first job runs", || ops.started("hold-0")).await;
        for n in 1..=TRANSFER_QUEUE_BOUND {
            callers.push(spawn_run(&queue, &format!("hold-{n}"), false));
        }
        until("the queue is full", || queue.tx.capacity() == 0).await;
        let waiting = spawn_run(&queue, "hold-waiting", false);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished());
        // One job ends: the waiting caller gets the free slot.
        ops.gate.add_permits(1);
        until("the waiting caller is queued", || queue.tx.capacity() == 0).await;
        ops.gate.add_permits(TRANSFER_QUEUE_BOUND + 1);
        for caller in callers {
            caller.await.unwrap().unwrap();
        }
        waiting.await.unwrap().unwrap();
        assert_eq!(ops.max_running(), 1);
    }

    #[tokio::test]
    async fn an_abandoned_queued_job_is_skipped_and_a_running_one_stops() {
        let (queue, ops) = queue();
        let running = spawn_run(&queue, "hold-running", false);
        until("the job runs", || ops.started("hold-running")).await;
        let queued = spawn_run(&queue, "queued", false);
        tokio::time::sleep(Duration::from_millis(20)).await;
        queued.abort();
        let _ = queued.await;
        running.abort();
        let _ = running.await;
        until("the running job stopped", || ops.left("hold-running")).await;
        spawn_run(&queue, "next", false).await.unwrap().unwrap();
        assert!(!ops.started("queued"), "{:?}", ops.log());
        assert_eq!(
            ops.log(),
            [
                "start hold-running",
                "left hold-running",
                "start next",
                "left next"
            ]
        );
    }

    /// A blocking job whose caller left keeps its thread until its
    /// synchronous step ends; the next job starts only after that.
    #[tokio::test]
    async fn the_next_job_waits_until_an_abandoned_blocking_job_left_its_thread() {
        let (queue, ops) = queue();
        let held = spawn_run(&queue, "sync-held", true);
        until("the blocking job runs", || ops.started("sync-held")).await;
        held.abort();
        let _ = held.await;
        let next = spawn_run(&queue, "next", true);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!ops.started("next"), "{:?}", ops.log());
        ops.release_sync();
        next.await.unwrap().unwrap();
        assert_eq!(
            ops.log(),
            [
                "start sync-held",
                "left sync-held",
                "start next",
                "left next"
            ]
        );
        assert_eq!(ops.max_running(), 1);
    }

    #[tokio::test]
    async fn a_job_after_the_cancel_is_refused_at_once() {
        let (queue, _ops) = queue();
        queue.cancel();
        let refused = tokio::time::timeout(BOUND, queue.handle().run("web", job("x"), false))
            .await
            .unwrap();
        assert!(
            matches!(refused, Err(DaemonError::SessionClosed)),
            "{refused:?}"
        );
    }

    /// Cancels racing enqueues and completions: every caller gets its result
    /// or the close, and none hangs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancel_racing_enqueue_and_completion_never_hangs() {
        for round in 0..50 {
            let (queue, _ops) = queue();
            let callers: Vec<_> = (0..4)
                .map(|n| spawn_run(&queue, &format!("job-{n}"), n % 2 == 0))
                .collect();
            for _ in 0..round % 5 {
                tokio::task::yield_now().await;
            }
            queue.cancel();
            let late = spawn_run(&queue, "late", false);
            for caller in callers.into_iter().chain([late]) {
                let result = tokio::time::timeout(BOUND, caller).await.unwrap().unwrap();
                assert!(
                    matches!(result, Ok(()) | Err(DaemonError::SessionClosed)),
                    "{result:?}"
                );
            }
        }
    }

    /// The daemon runtime shuts down while a blocking job is in its
    /// synchronous step. The job's timer after that step belongs to the
    /// job's own runtime: no panic, and the staged file is removed.
    #[test]
    fn a_runtime_shutdown_during_a_blocking_job_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged.tmp");
        let label = format!("stage:{}", staged.display());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let (queue, ops) = runtime.block_on(async { queue() });
        let caller = {
            let handle = queue.handle();
            let job = job(&label);
            runtime.spawn(async move { handle.run("web", job, true).await })
        };
        runtime.block_on(until("the job is in its synchronous step", || {
            ops.started(&label)
        }));
        assert!(staged.exists());
        // Released only after the runtime has started to shut down.
        let release = {
            let ops = Arc::clone(&ops);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                ops.release_sync();
            })
        };
        drop(caller);
        drop(runtime);
        release.join().unwrap();
        assert!(ops.left(&label), "{:?}", ops.log());
        assert!(!ops.dropped_panicking.load(Ordering::SeqCst));
        assert!(!staged.exists());
        drop(queue);
    }
}
