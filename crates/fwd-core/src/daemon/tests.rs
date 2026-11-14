//! The launcher's reports when no daemon answers, `stop` on a daemon that stops answering after it
//! was found, and the probe's one deadline.

use super::holder::tests::paths;
use super::*;

fn a_launcher() -> Autostart {
    Autostart::Spawn { daemon_bin: "/nonexistent/fwd".into() }
}

/// A launcher that gives up waiting for the bootstrap lock, which another launcher holds while it
/// waits on the daemon that holds the lifetime lock, names that daemon when no command reaches it,
/// behind a probe of its own. This process holds both locks before the launcher starts.
#[tokio::test]
async fn a_launcher_that_gives_up_on_the_bootstrap_lock_names_the_daemon_no_command_reaches() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let paths = paths(dir.path());
    let lock = try_lock(&paths.lifetime_lock()).expect("the lock").expect("free");
    let holder =
        LockHolder { listening_on: Some(4242), ..LockHolder::this_process().expect("this process").expect("named") };
    holder.write(&lock).expect("written");
    let _bootstrap = try_lock(&paths.bootstrap_lock()).expect("the bootstrap lock").expect("free");

    let error = ensure_running(&paths, &a_launcher()).await.expect_err("no daemon answers");
    assert_eq!(
        error.downcast_ref::<NotAnswering>(),
        Some(&NotAnswering { holder, why: Unreached::PortFileMissing { port: 4242 } }),
        "{error:#}"
    );
}

/// A launcher whose start deadline passes while a process that names no daemon holds the lifetime
/// lock says so, with the log's path, rather than that no daemon answered.
#[tokio::test]
async fn a_launcher_at_its_deadline_names_a_lock_held_by_a_process_that_names_no_daemon() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let paths = paths(dir.path());
    let _lock = try_lock(&paths.lifetime_lock()).expect("the lock").expect("free");
    let error = ensure_running(&paths, &a_launcher()).await.expect_err("no daemon answers");
    assert!(matches!(error.downcast_ref::<NotStarted>(), Some(NotStarted::Unnamed { .. })), "{error:#}");
}

/// A daemon found answering that then stops answering, with the lock still held by it, is not
/// reported as none: here the daemon `stop` was given is this process, whose record holds the lock
/// and whose port file names a port nothing listens on any more.
#[tokio::test]
async fn stop_on_a_daemon_that_stopped_answering_after_it_was_found_stops_nothing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let paths = paths(dir.path());
    let port = std::net::TcpListener::bind("127.0.0.1:0").expect("a port").local_addr().expect("its address").port();
    let this = LockHolder::this_process().expect("this process").expect("named");
    let lock = try_lock(&paths.lifetime_lock()).expect("the lock").expect("free");
    LockHolder { listening_on: Some(port), ..this }.write(&lock).expect("written");
    let pf = PortFile { port, pid: this.pid, instance_id: "found".into(), started: 0, bind: Bind::Loopback };
    write_port_file(&paths.port_file(), &pf).expect("written");
    let found = Running { pf, projects: Vec::new(), build: None };

    let error = stop_running(&paths, &found).await.expect_err("nothing was stopped");
    match error.downcast_ref::<StoppedNothing>() {
        Some(StoppedNothing::Unreached(daemon)) => {
            assert!(matches!(daemon.why, Unreached::NoValidAnswer { port: p, .. } if p == port), "{error:#}")
        }
        _ => panic!("{error:#}"),
    }
}

/// A listener whose queue of connections is full and that accepts none: the probe ends at its
/// deadline as no answer, whether the connection waits in the queue or the reply never comes.
#[tokio::test]
async fn the_probe_ends_at_its_deadline_on_a_listener_that_accepts_nothing() {
    let socket = tokio::net::TcpSocket::new_v4().expect("a socket");
    socket.bind("127.0.0.1:0".parse().expect("an address")).expect("bound");
    let listener = socket.listen(1).expect("listening");
    let address = listener.local_addr().expect("its address");
    let _queued: Vec<_> = (0..16)
        .map_while(|_| std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).ok())
        .collect();
    let pf = PortFile { port: address.port(), pid: 0, instance_id: "full".into(), started: 0, bind: Bind::Loopback };
    let probe = http(&pf, "GET", "/health", None, Duration::from_millis(300));
    let answer = tokio::time::timeout(Duration::from_secs(10), probe).await.expect("the probe kept to its deadline");
    let error = answer.expect_err("nothing answers");
    assert!(error.is::<NoAnswer>(), "{error:#}");
}
