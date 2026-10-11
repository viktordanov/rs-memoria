//! The `git cat-file --batch` reader against stand-in processes: framing
//! across split writes, truncated and malformed responses, deadlines,
//! process-group cleanup, stderr floods, and cancellation.
//!
//! These tests start processes, so they run in their own test binary.

use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use memoria_infrastructure::bounded_process::Supervisor;
use memoria_infrastructure::cat_file::{BatchError, CatFileBatch, Header};

const OID: &str = "0123456789abcdef0123456789abcdef01234567";

/// A stand-in for `git cat-file --batch`: a shell script that prints a
/// fixed response stream, whatever it is asked.
fn fake(script: &str) -> CatFileBatch {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    CatFileBatch::spawn(command, None).unwrap()
}

fn soon() -> Option<Instant> {
    Some(Instant::now() + Duration::from_secs(5))
}

#[test]
fn frames_survive_split_writes_and_header_like_content() {
    // A blob whose content holds line feeds and header-like text, sent
    // in pieces that split the header and the content, then a missing
    // object in the same write as the end of the blob.
    let content = "fake blob 9\nnext x\n\nend";
    assert_eq!(content.len(), 23);
    let mut batch = fake(&format!(
        "printf '{}'; sleep 0.05; printf '{} blob 2'; sleep 0.05; printf '3\\nfake blob 9\\nne'; \
         sleep 0.05; printf 'xt x\\n\\nend\\nnext missing\\n'; cat >/dev/null",
        &OID[..10],
        &OID[10..]
    ));
    batch.send("first").unwrap();
    let Header::Object { size, .. } = batch.header("first", soon()).unwrap() else {
        panic!("expected an object");
    };
    assert_eq!(size, 23);
    assert_eq!(batch.content(size, soon()).unwrap(), content.as_bytes());
    assert_eq!(batch.header("next", soon()).unwrap(), Header::Missing);
}

#[test]
fn truncated_and_malformed_responses_are_protocol_errors() {
    // Content shorter than its size, then end of output.
    let mut batch = fake(&format!("printf '{OID} blob 10\\nabc'"));
    let header = batch.header("x", soon()).unwrap();
    assert!(matches!(header, Header::Object { size: 10, .. }));
    assert!(matches!(
        batch.content(10, soon()),
        Err(BatchError::Protocol(_))
    ));
    // Content not followed by a line feed.
    let mut batch = fake(&format!("printf '{OID} blob 3\\nabcX'; sleep 5"));
    batch.header("x", soon()).unwrap();
    assert!(matches!(
        batch.content(3, soon()),
        Err(BatchError::Protocol(_))
    ));
    // A header without a line feed that grows past the limit.
    let mut batch = fake("head -c 100000 /dev/zero | tr '\\0' 'a'; sleep 5");
    assert!(matches!(
        batch.header("x", soon()),
        Err(BatchError::Protocol(_))
    ));
    // A process that exits at once, with its stderr in the message.
    let mut batch = fake("echo 'fatal: broken' >&2; exit 3");
    let Err(BatchError::Protocol(message)) = batch.header("x", soon()) else {
        panic!("expected a protocol error");
    };
    assert!(message.contains("stopped"), "{message}");
}

#[test]
fn a_silent_process_meets_the_deadline_and_an_owned_group_is_terminated() {
    let marker = tempfile::tempdir().unwrap();
    let alive = marker.path().join("alive");
    let owner = Supervisor::new(Instant::now() + Duration::from_secs(30));
    let mut command = Command::new("sh");
    command.arg("-c").arg(format!(
        "( while true; do echo x >> '{}'; sleep 0.05; done ) & sleep 30",
        alive.display()
    ));
    let mut batch = CatFileBatch::spawn(command, Some(owner)).unwrap();
    let started = Instant::now();
    let result = batch.header("x", Some(Instant::now() + Duration::from_millis(200)));
    assert_eq!(result, Err(BatchError::Deadline));
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(batch);
    std::thread::sleep(Duration::from_millis(200));
    let before = std::fs::metadata(&alive).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(300));
    let after = std::fs::metadata(&alive).map(|m| m.len()).unwrap_or(0);
    assert_eq!(before, after, "a descendant outlived the owned reader");
}

#[test]
fn an_unowned_reader_stays_in_the_callers_process_group() {
    // A caller that ends its own process group, as the native hook does to
    // a status child on its deadline, must end the reader with it.
    let mut batch = fake("sleep 30");
    let pid = batch.id();
    let child = rustix::process::Pid::from_raw(pid as i32).unwrap();
    assert_eq!(
        rustix::process::getpgid(Some(child)).unwrap(),
        rustix::process::getpgid(None).unwrap(),
        "the reader left the caller's process group"
    );
    // Its own deadline still ends it.
    let result = batch.header("x", Some(Instant::now() + Duration::from_millis(100)));
    assert_eq!(result, Err(BatchError::Deadline));
    drop(batch);
    let gone = Instant::now() + Duration::from_secs(2);
    while rustix::process::test_kill_process(child).is_ok() && Instant::now() < gone {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        rustix::process::test_kill_process(child).is_err(),
        "the dropped reader is still running"
    );
}

#[test]
fn a_stderr_flood_never_blocks_the_responses() {
    let mut batch = fake(&format!(
        "head -c 1000000 /dev/zero >&2; printf '{OID} blob 0\\n\\n'; cat >/dev/null"
    ));
    let header = batch.header("x", soon()).unwrap();
    assert!(matches!(header, Header::Object { size: 0, .. }));
    assert_eq!(batch.content(0, soon()).unwrap(), b"");
    // A flood followed by an exit: the message keeps only the cap.
    let mut batch = fake("head -c 1000000 /dev/zero | tr '\\0' e >&2; exit 3");
    let Err(BatchError::Protocol(message)) = batch.header("x", soon()) else {
        panic!("expected a protocol error");
    };
    assert!(message.len() < 8 * 1024 + 200, "{} bytes", message.len());
}

#[test]
fn cancellation_stops_a_waiting_reader() {
    let owner = Supervisor::new(Instant::now() + Duration::from_secs(30));
    let mut command = Command::new("sh");
    command.arg("-c").arg("sleep 30");
    let mut batch = CatFileBatch::spawn(command, Some(Arc::clone(&owner))).unwrap();
    let canceller = Arc::clone(&owner);
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        canceller.cancel();
    });
    assert_eq!(batch.header("x", None), Err(BatchError::Cancelled));
    handle.join().unwrap();
    // A cancelled owner refuses a new reader.
    let mut command = Command::new("sh");
    command.arg("-c").arg("sleep 30");
    assert!(matches!(
        CatFileBatch::spawn(command, Some(owner)),
        Err(BatchError::Cancelled)
    ));
}

fn owned(script: &str) -> CatFileBatch {
    let owner = Supervisor::new(Instant::now() + Duration::from_secs(30));
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    CatFileBatch::spawn(command, Some(owner)).unwrap()
}

#[test]
fn an_owned_reader_lets_git_exit_normally_then_ends_its_group() {
    let marker = tempfile::tempdir().unwrap();
    let ended = marker.path().join("ended");
    let script = format!(
        "printf '{OID} blob 1\\nx\\n'; cat >/dev/null; printf done > '{}'",
        ended.display()
    );
    let mut batch = owned(&script);
    batch.send("first").unwrap();
    assert!(matches!(
        batch.header("first", soon()).unwrap(),
        Header::Object { size: 1, .. }
    ));
    assert_eq!(batch.content(1, soon()).unwrap(), b"x");
    drop(batch);
    // End of input reached the process before its group was killed.
    assert!(ended.exists(), "the process was killed instead of ending");

    // An owned reader with an unread response ends its group at once.
    let unread = marker.path().join("unread");
    let script = format!(
        "printf '{OID} blob 1\\nx\\n'; cat >/dev/null; sleep 0.3; printf done > '{}'",
        unread.display()
    );
    let mut batch = owned(&script);
    batch.send("first").unwrap();
    batch.header("first", soon()).unwrap();
    drop(batch);
    std::thread::sleep(Duration::from_millis(600));
    assert!(!unread.exists(), "an abandoned owned reader was not killed");
}

#[test]
fn an_unowned_reader_ends_like_command_output() {
    // A reader in the caller's group replaces `Command::output`: it is not
    // killed. Dropping it reads the remaining output to its end, which
    // waits for Git and for anything Git started that holds the pipe.
    let marker = tempfile::tempdir().unwrap();
    let ended = marker.path().join("ended");
    let descendant = marker.path().join("descendant");
    let stderr_only = marker.path().join("stderr-only");
    let script = format!(
        "( sleep 0.3; printf done > '{}' ) & ( sleep 0.4; printf done > '{}' ) >/dev/null & printf '{OID} blob 1\\nx\\n'; cat >/dev/null; printf done > '{}'",
        descendant.display(),
        stderr_only.display(),
        ended.display()
    );
    let mut batch = fake(&script);
    batch.send("first").unwrap();
    // The response stays unread: the walk stopped early.
    batch.header("first", soon()).unwrap();
    drop(batch);
    assert!(ended.exists(), "the process was killed instead of ending");
    assert!(
        descendant.exists(),
        "the drop returned before a descendant that holds the pipe ended"
    );
    assert!(
        stderr_only.exists(),
        "the drop returned before a descendant that holds only stderr ended"
    );
}

#[test]
fn queued_requests_never_wait_on_a_process_that_is_writing() {
    // The stand-in writes a 1 MB response before it reads any request, as
    // Git does when its output is full. The requests are far larger than
    // any pipe buffer, so writing them on the reading thread would wait on
    // a process that waits on us. The writer thread keeps both moving.
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut batch = fake(&format!(
            "printf '{OID} blob 1048576\\n'; head -c 1048576 /dev/zero; printf '\\n'; cat >/dev/null"
        ));
        let name = "x".repeat(1000);
        for _ in 0..256 {
            batch.queue(name.clone()).unwrap();
        }
        let header = batch.header(&name, soon()).unwrap();
        assert!(matches!(
            header,
            Header::Object {
                size: 1_048_576,
                ..
            }
        ));
        let content = batch.content(1_048_576, soon()).unwrap();
        let _ = done.send(content.len());
    });
    let read = finished
        .recv_timeout(Duration::from_secs(10))
        .expect("queueing requests waited on the reader");
    assert_eq!(read, 1_048_576);
}

#[test]
fn a_dropped_reader_stops_instead_of_reading_a_large_remainder() {
    let marker = tempfile::tempdir().unwrap();
    let ended = marker.path().join("ended");
    let mut batch = fake(&format!(
        "printf '{OID} blob 50000000\\n'; head -c 50000000 /dev/zero; printf done > '{}'",
        ended.display()
    ));
    batch.header("x", soon()).unwrap();
    // The walk stopped here, with 50 MB unread.
    drop(batch);
    std::thread::sleep(Duration::from_millis(300));
    assert!(!ended.exists(), "the reader read the whole remainder");
}
