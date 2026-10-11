//! The one owner of a session's lifecycle and current desktop size.
//!
//! The session thread creates the owner after the handshake. The state starts
//! at [`Lifecycle::Live`] and changes to [`Lifecycle::Ended`] once: the first
//! end cause wins. The state is published through a `tokio::sync::watch`, so a
//! reader never waits for the session loop and a slow reader only sees the
//! latest value. The frame `ended` flag is set from the same transition, after
//! the lifecycle, so it is never `true` while the lifecycle is still `Live`.

use std::sync::Arc;

use tokio::sync::watch;

use crate::framebuffer::SharedFrame;

/// Whether a session is live or has ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    /// The RDP session loop runs.
    Live,
    /// The session ended. It does not become live again.
    Ended(EndCause),
}

/// Why a session ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndCause {
    /// The RDP server ended the session; the text is the server's reason.
    ServerEnded(String),
    /// The session was closed or dropped by its owner.
    ClientClosed,
    /// The session loop failed; the text is the error.
    Error(String),
}

impl std::fmt::Display for EndCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServerEnded(reason) => write!(f, "the server ended the session: {reason}"),
            Self::ClientClosed => f.write_str("the session was closed"),
            Self::Error(text) => write!(f, "the session failed: {text}"),
        }
    }
}

/// The lifecycle of a session together with its current desktop size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionState {
    /// Live or ended, with the cause.
    pub lifecycle: Lifecycle,
    /// The desktop size of the current activation, in pixels.
    pub desktop_size: (u32, u32),
}

/// The writer of a session's [`SessionState`]. Cloning is cheap.
#[derive(Clone, Debug)]
pub(crate) struct LifecycleOwner {
    tx: Arc<watch::Sender<SessionState>>,
    frame: SharedFrame,
}

impl LifecycleOwner {
    /// A live lifecycle at `desktop_size` that also ends `frame`.
    pub(crate) fn new(frame: SharedFrame, desktop_size: (u32, u32)) -> Self {
        let (tx, _) = watch::channel(SessionState {
            lifecycle: Lifecycle::Live,
            desktop_size,
        });
        Self {
            tx: Arc::new(tx),
            frame,
        }
    }

    /// End the lifecycle with `cause` if it is still live, then mark the frame
    /// ended. A later call keeps the first cause.
    pub(crate) fn end(&self, cause: EndCause) {
        self.tx.send_if_modified(|state| {
            if state.lifecycle == Lifecycle::Live {
                state.lifecycle = Lifecycle::Ended(cause);
                true
            } else {
                false
            }
        });
        self.frame.mark_ended();
    }

    /// Publish the desktop size of the current activation.
    pub(crate) fn set_desktop_size(&self, size: (u32, u32)) {
        self.tx.send_if_modified(|state| {
            if state.desktop_size == size {
                false
            } else {
                state.desktop_size = size;
                true
            }
        });
    }

    /// The current desktop size.
    pub(crate) fn desktop_size(&self) -> (u32, u32) {
        self.tx.borrow().desktop_size
    }

    /// A new receiver of the state.
    pub(crate) fn subscribe(&self) -> watch::Receiver<SessionState> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cause(owner: &LifecycleOwner) -> Option<EndCause> {
        match owner.subscribe().borrow().lifecycle.clone() {
            Lifecycle::Live => None,
            Lifecycle::Ended(cause) => Some(cause),
        }
    }

    #[test]
    fn the_first_end_cause_wins() {
        let owner = LifecycleOwner::new(SharedFrame::new(), (10, 10));
        assert_eq!(cause(&owner), None);
        owner.end(EndCause::ServerEnded("bye".to_owned()));
        owner.end(EndCause::ClientClosed);
        assert_eq!(cause(&owner), Some(EndCause::ServerEnded("bye".to_owned())));

        let owner = LifecycleOwner::new(SharedFrame::new(), (10, 10));
        owner.end(EndCause::ClientClosed);
        owner.end(EndCause::ServerEnded("bye".to_owned()));
        assert_eq!(cause(&owner), Some(EndCause::ClientClosed));
    }

    #[tokio::test]
    async fn the_lifecycle_ends_before_the_frame_flag() {
        let frame = SharedFrame::new();
        let owner = LifecycleOwner::new(frame.clone(), (10, 10));
        let watch = frame.watch();
        let lifecycle = owner.subscribe();
        let observer = tokio::spawn(async move {
            let status = watch.changed(0).await;
            (status.ended, lifecycle.borrow().lifecycle.clone())
        });
        tokio::task::yield_now().await;
        owner.end(EndCause::Error("broken".to_owned()));
        let (ended, seen) = tokio::time::timeout(std::time::Duration::from_secs(2), observer)
            .await
            .expect("the frame flag wakes the observer")
            .expect("join");
        assert!(ended);
        assert_eq!(seen, Lifecycle::Ended(EndCause::Error("broken".to_owned())));
    }

    #[test]
    fn the_desktop_size_is_published_only_on_a_change() {
        let owner = LifecycleOwner::new(SharedFrame::new(), (10, 10));
        let mut rx = owner.subscribe();
        rx.mark_unchanged();
        owner.set_desktop_size((10, 10));
        assert!(!rx.has_changed().unwrap());
        owner.set_desktop_size((20, 5));
        assert!(rx.has_changed().unwrap());
        assert_eq!(owner.desktop_size(), (20, 5));
    }

    #[test]
    fn end_causes_name_the_end() {
        assert_eq!(
            EndCause::ServerEnded("logoff".to_owned()).to_string(),
            "the server ended the session: logoff"
        );
        assert_eq!(EndCause::ClientClosed.to_string(), "the session was closed");
        assert_eq!(
            EndCause::Error("read PDU failed".to_owned()).to_string(),
            "the session failed: read PDU failed"
        );
    }
}
