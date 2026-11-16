//! A write the page makes is reported as made only once the daemon has it, as a command's is
//! (`fwd_core::daemon`, `Client::while_writing`): Ankurah relays a commit to the durable peers the
//! page's node has, and while the connection to the daemon is down the node has none, so the commit
//! goes to the page's own store alone and succeeds there. Ankurah's commit, at fwd's pin, offers no
//! way to require the daemon's acknowledgement, so the write watches the connection instead.

use super::client;
use ankurah::signals::{Peek, Subscribe};
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Why a write the page made is not reported as made ([`while_writing`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lost {
    /// The connection to the daemon was down when the write was to begin, so it began nothing.
    BeforeWriting,
    /// The connection dropped at some moment while the write ran: a commit made while it was down
    /// reached no daemon, and one whose acknowledgement the drop cut off may have reached it, so
    /// that sending it again may make it twice.
    Unconfirmed,
}

impl fmt::Display for Lost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Lost::BeforeWriting => "the connection to the daemon is lost, so nothing was sent",
            Lost::Unconfirmed => {
                "the connection to the daemon dropped while this was being sent, so it may or may not have reached the daemon: look before sending it again"
            }
        })
    }
}

impl std::error::Error for Lost {}

/// Runs `work`, a write the page makes, so that the page never reports as made a write the daemon
/// may not have got: `work` does not begin while the connection to the daemon is down
/// ([`Lost::BeforeWriting`]), and once it has begun, a connection that dropped at any moment while
/// it ran, however soon it came back, ends it with [`Lost::Unconfirmed`], whether it succeeded or
/// failed (its error then the cause). Unlike a command's, a page's write needs no deadline: the
/// requests of a connection that closes fail at once, so `work` ends by itself.
pub async fn while_writing<T>(work: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    let state = client().connection_state();
    watching(&state.map(|state| state.to_string() == "Connected"), work).await
}

/// Runs `work` as [`while_writing`] does, `connected` saying whether the connection is up. The
/// socket client takes the daemon off the node's durable peers in the step that tells `connected`'s
/// listeners the connection is down, and tells them it is up only once the daemon is a durable peer
/// again: so a write through which `connected` never turned false relayed its commit to the daemon
/// and waited for its answer.
async fn watching<T>(
    connected: &(impl Subscribe<bool> + Peek<bool>),
    work: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    if !connected.peek() {
        return Err(Lost::BeforeWriting.into());
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let _watch = connected.subscribe({
        let dropped = dropped.clone();
        move |up: bool| {
            if !up {
                dropped.store(true, Ordering::Relaxed);
            }
        }
    });
    let done = work.await;
    if !dropped.load(Ordering::Relaxed) {
        return done;
    }
    Err(match done {
        Ok(_) => Lost::Unconfirmed.into(),
        Err(error) => error.context(Lost::Unconfirmed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ankurah::signals::Mut;
    use anyhow::anyhow;
    use futures::channel::oneshot;
    use futures::executor::block_on;
    use futures::future::join;

    /// A write begins only while the connection is up, and while it stays up the write ends as it
    /// did: with what it made, or with its own error, which is no loss of the connection's.
    #[test]
    fn a_write_begins_only_on_a_connection_that_is_up_and_ends_as_it_did_while_it_stays_up() {
        let down = Mut::new(false);
        let mut began = false;
        let refused = block_on(watching(&down.read(), async {
            began = true;
            Ok(())
        }));
        assert!(!began, "the write began");
        assert_eq!(refused.unwrap_err().downcast_ref::<Lost>(), Some(&Lost::BeforeWriting));

        let up = Mut::new(true);
        assert_eq!(block_on(watching(&up.read(), async { Ok(7) })).unwrap(), 7);
        let failed = block_on(watching(&up.read(), async { Err::<(), _>(anyhow!("refused")) })).unwrap_err();
        assert_eq!(failed.downcast_ref::<Lost>(), None, "{failed:#}");
    }

    /// A write during which the connection drops and comes back is unconfirmed: when it succeeded,
    /// since its commit may have gone to the page's own store alone, and when it failed, since the
    /// daemon may have made what it wrote and only its acknowledgement was lost.
    #[test]
    fn a_write_during_which_the_connection_dropped_and_came_back_is_unconfirmed() {
        for outcome in [Ok(()), Err("the daemon's acknowledgement was lost")] {
            let connection = Mut::new(true);
            let connected = connection.read();
            let (began, when_begun) = oneshot::channel();
            let (release, when_released) = oneshot::channel();
            let write = watching(&connected, async {
                began.send(()).unwrap();
                when_released.await.unwrap();
                outcome.map_err(|cause| anyhow!(cause))
            });
            let drop_and_return = async {
                when_begun.await.unwrap();
                connection.set(false);
                connection.set(true);
                release.send(()).unwrap();
            };
            let (written, ()) = block_on(join(write, drop_and_return));
            let error = written.unwrap_err();
            assert_eq!(error.downcast_ref::<Lost>(), Some(&Lost::Unconfirmed), "{error:#}");
            if let Err(cause) = outcome {
                assert_eq!(error.root_cause().to_string(), cause, "the write's own error is lost");
            }
        }
    }
}
