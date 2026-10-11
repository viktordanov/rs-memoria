//! One long-running `git cat-file --batch` process.
//!
//! A read through a separate `git cat-file blob` process costs one process
//! start for each object. This reader keeps one process for many requests
//! and reads its responses as frames, with the same Git command environment
//! as the per-object read it replaces.
//!
//! The protocol is the plain `--batch` form that every supported Git
//! version has: one request per line on stdin, and for each request either
//! `<name> missing`, `<name> ambiguous`, or `<oid> <type> <size>` followed by
//! the content and a line feed. A request is a full object ID or a
//! `<commit>:<path>` name. A name that contains a line feed or a carriage
//! return cannot be framed, so callers keep the per-object read for it.
//!
//! The reader is bounded in memory and in time. Stdout arrives through a
//! small bounded channel, stderr is kept only up to a cap, a header line has
//! a length limit, and content is read only up to the size the caller
//! accepts. Every wait has the caller's deadline. Any response that does not
//! match the protocol, and any early stop, discards the process: dropping the
//! reader terminates it and reaps it, so a partly read response can never be
//! mistaken for the next one. An owned reader leads its own process group,
//! which ends with it. Any other reader stays in the caller's process group,
//! as a per-object `git cat-file blob` did.

use std::io::{Read as _, Write as _};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::bounded_process::{
    STDERR_CAPTURE, Supervisor, spawn_in_session, terminate_child, terminate_group,
};

/// Stdout chunk size, and how many chunks may wait unread. The reader thread
/// blocks when the channel is full, so at most this much output is buffered
/// ahead of the caller.
const CHUNK: usize = 64 * 1024;
const QUEUED_CHUNKS: usize = 4;

/// Room for a header line beyond the request name: an object ID, a type,
/// a decimal size, or the `missing` and `ambiguous` words.
const HEADER_SLACK: usize = 160;

/// Unread output that dropping a reader in the caller's group reads before
/// it stops the process instead.
const DRAIN_LIMIT: usize = 8 * 1024 * 1024;

/// How long a reader with no unread response waits for Git to exit by
/// itself after its input closes, before the process group is killed.
/// Git exits at once on end of input, so this rarely waits at all.
const EXIT_GRACE: Duration = Duration::from_millis(50);

/// The first line of one response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Header {
    /// Git has no such object. `ambiguous` is reported the same way, as the
    /// per-object read reports both as a failed command.
    Missing,
    Object {
        oid: String,
        kind: String,
        size: u64,
    },
}

/// Why a request did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchError {
    /// The caller's deadline expired first.
    Deadline,
    /// The owning supervisor cancelled the process.
    Cancelled,
    /// The process stopped, or answered outside the protocol. The caller
    /// discards the reader and can use the per-object read instead.
    Protocol(String),
}

pub struct CatFileBatch {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Requests for the writer thread, once [`CatFileBatch::queue`] started it.
    queued: Option<mpsc::Sender<String>>,
    chunks: Receiver<Vec<u8>>,
    /// Output received. Bytes before `consumed` are already read, so a
    /// response is taken without shifting the rest of the buffer.
    pending: Vec<u8>,
    consumed: usize,
    /// Requests whose response is not read to its end.
    unanswered: usize,
    /// A request failed, so the stream position is unknown.
    broken: bool,
    stderr: Arc<Mutex<Vec<u8>>>,
    /// Closes when stderr reaches its end.
    stderr_done: Receiver<()>,
    owner: Option<Arc<Supervisor>>,
    /// Whether the process leads its own process group, which ends with
    /// the reader. A reader in the caller's group is never killed while it
    /// answers normally: dropping it reads to the end of its output and
    /// waits, as `Command::output` does, so whoever ends the caller's group,
    /// such as the native hook's deadline, ends the reader too.
    own_group: bool,
}

/// Whether `name` can be sent as one request line.
pub fn frameable(name: &str) -> bool {
    !name.is_empty() && !name.contains(['\n', '\r'])
}

impl CatFileBatch {
    /// Start `command`, which must already name `cat-file --batch` and its
    /// environment. With an owner, the process gets its own session and the
    /// owner adopts it, so the owner's cancellation terminates its group.
    /// Without one, it stays in the caller's process group, in place of a
    /// per-object `Command::output`.
    pub fn spawn(
        command: Command,
        owner: Option<Arc<Supervisor>>,
    ) -> Result<CatFileBatch, BatchError> {
        let own_group = owner.is_some();
        CatFileBatch::start(command, owner, own_group)
    }

    /// Start `command` in its own session without an owner, in place of a
    /// per-object read that ran in its own session. Its whole group ends
    /// with the reader, on a deadline and after its last response alike.
    pub fn spawn_in_own_group(command: Command) -> Result<CatFileBatch, BatchError> {
        CatFileBatch::start(command, None, true)
    }

    fn start(
        mut command: Command,
        owner: Option<Arc<Supervisor>>,
        own_group: bool,
    ) -> Result<CatFileBatch, BatchError> {
        if owner.as_ref().is_some_and(|owner| owner.cancelled()) {
            return Err(BatchError::Cancelled);
        }
        command.stdin(Stdio::piped());
        let spawned = if own_group {
            spawn_in_session(&mut command)
        } else {
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
        };
        let mut child = spawned
            .map_err(|err| BatchError::Protocol(format!("cannot start git cat-file: {err}")))?;
        if let Some(owner) = &owner
            && !owner.adopt(child.id())
        {
            terminate_group(&mut child);
            return Err(BatchError::Cancelled);
        }
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let (sender, chunks) = mpsc::sync_channel(QUEUED_CHUNKS);
        std::thread::spawn(move || {
            loop {
                let mut chunk = vec![0u8; CHUNK];
                match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        chunk.truncate(read);
                        // A dropped reader ends this thread.
                        if sender.send(chunk).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        // Stderr is read to the end so Git never blocks on it, and kept only
        // up to the cap for the error message.
        let kept = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&kept);
        let (done, stderr_done) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            // Dropping the sender at the end tells the reader that stderr closed.
            let _done = done;
            let mut chunk = [0u8; 4096];
            while let Ok(read) = stderr.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                if let Ok(mut kept) = sink.lock()
                    && kept.len() < STDERR_CAPTURE
                {
                    let room = STDERR_CAPTURE - kept.len();
                    kept.extend_from_slice(&chunk[..read.min(room)]);
                }
            }
        });
        Ok(CatFileBatch {
            child,
            stdin,
            queued: None,
            chunks,
            pending: Vec::new(),
            consumed: 0,
            unanswered: 0,
            broken: false,
            stderr: kept,
            stderr_done,
            owner,
            own_group,
        })
    }

    /// The process identifier.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Send one request. The name must be [`frameable`].
    pub fn send(&mut self, name: &str) -> Result<(), BatchError> {
        debug_assert!(frameable(name));
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(BatchError::Protocol("git cat-file input is closed".into()));
        };
        let mut line = Vec::with_capacity(name.len() + 1);
        line.extend_from_slice(name.as_bytes());
        line.push(b'\n');
        let written = stdin.write_all(&line).and_then(|()| stdin.flush());
        match written {
            Ok(()) => {
                self.unanswered += 1;
                Ok(())
            }
            Err(err) => {
                self.broken = true;
                Err(self.stopped(&format!("cannot write a request: {err}")))
            }
        }
    }

    /// Queue one request for a writer thread, which owns the input from the
    /// first call on.
    ///
    /// The caller keeps reading responses while the thread writes, so
    /// neither side waits on the other, whatever the pipe capacity: on Linux
    /// a pipe can hold as little as 8 KiB once a user exceeds the pipe soft
    /// limit. The caller bounds how many requests are queued and unanswered.
    /// Dropping the reader ends the thread after the requests already queued.
    pub fn queue(&mut self, name: String) -> Result<(), BatchError> {
        debug_assert!(frameable(&name));
        if self.queued.is_none() {
            let Some(mut stdin) = self.stdin.take() else {
                return Err(BatchError::Protocol("git cat-file input is closed".into()));
            };
            let (sender, requests) = mpsc::channel::<String>();
            std::thread::spawn(move || {
                for name in requests {
                    let mut line = name.into_bytes();
                    line.push(b'\n');
                    if stdin.write_all(&line).is_err() {
                        return;
                    }
                }
                // The input closes here, after the last queued request.
            });
            self.queued = Some(sender);
        }
        let sent = self.queued.as_ref().is_some_and(|q| q.send(name).is_ok());
        if !sent {
            self.broken = true;
            return Err(self.stopped("git cat-file stopped reading requests"));
        }
        self.unanswered += 1;
        Ok(())
    }

    /// Read the header of the response to `name`.
    pub fn header(&mut self, name: &str, deadline: Option<Instant>) -> Result<Header, BatchError> {
        let header = self.read_header(name, deadline);
        match &header {
            // A missing object has no content, so its response is complete.
            Ok(Header::Missing) => self.unanswered = self.unanswered.saturating_sub(1),
            Ok(Header::Object { .. }) => {}
            Err(_) => self.broken = true,
        }
        header
    }

    fn read_header(&mut self, name: &str, deadline: Option<Instant>) -> Result<Header, BatchError> {
        let limit = name.len() + HEADER_SLACK;
        loop {
            let unread = &self.pending[self.consumed..];
            if let Some(end) = unread.iter().position(|b| *b == b'\n') {
                if end > limit {
                    return Err(BatchError::Protocol("a response header is too long".into()));
                }
                let header = parse_header(name, &unread[..end]);
                self.consumed += end + 1;
                return header;
            }
            if unread.len() > limit {
                return Err(BatchError::Protocol("a response header is too long".into()));
            }
            self.receive(deadline)?;
        }
    }

    /// Read exactly `size` content bytes and the line feed that ends them.
    ///
    /// The caller has already decided that `size` is within its byte limit.
    pub fn content(&mut self, size: u64, deadline: Option<Instant>) -> Result<Vec<u8>, BatchError> {
        let content = self.read_content(size, deadline);
        match &content {
            Ok(_) => self.unanswered = self.unanswered.saturating_sub(1),
            Err(_) => self.broken = true,
        }
        content
    }

    fn read_content(
        &mut self,
        size: u64,
        deadline: Option<Instant>,
    ) -> Result<Vec<u8>, BatchError> {
        let size = usize::try_from(size)
            .map_err(|_| BatchError::Protocol("an object size does not fit in memory".into()))?;
        let mut content = Vec::with_capacity(size.min(CHUNK));
        loop {
            let wanted = size - content.len();
            let unread = &self.pending[self.consumed..];
            if unread.len() > wanted {
                content.extend_from_slice(&unread[..wanted]);
                let terminator = unread[wanted];
                self.consumed += wanted + 1;
                if terminator != b'\n' {
                    return Err(BatchError::Protocol(
                        "an object is not followed by a line feed".into(),
                    ));
                }
                return Ok(content);
            }
            content.extend_from_slice(unread);
            self.consumed = self.pending.len();
            self.receive(deadline)?;
        }
    }

    /// Move one more stdout chunk into `pending`.
    fn receive(&mut self, deadline: Option<Instant>) -> Result<(), BatchError> {
        let deadline = match (deadline, &self.owner) {
            (Some(own), Some(owner)) => Some(own.min(Instant::now() + owner.remaining())),
            (None, Some(owner)) => Some(Instant::now() + owner.remaining()),
            (deadline, None) => deadline,
        };
        let chunk = match deadline {
            Some(deadline) => {
                let wait = deadline.saturating_duration_since(Instant::now());
                if wait.is_zero() {
                    return Err(BatchError::Deadline);
                }
                match self.chunks.recv_timeout(wait) {
                    Ok(chunk) => chunk,
                    Err(RecvTimeoutError::Timeout) => return Err(BatchError::Deadline),
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(self.stopped("git cat-file stopped before it answered"));
                    }
                }
            }
            None => self
                .chunks
                .recv()
                .map_err(|_| self.stopped("git cat-file stopped before it answered"))?,
        };
        // Drop what was read before the buffer grows, so it holds at most
        // one unread response prefix and one chunk.
        self.pending.drain(..self.consumed);
        self.consumed = 0;
        self.pending.extend_from_slice(&chunk);
        Ok(())
    }

    /// The error for a process that stopped answering.
    fn stopped(&self, what: &str) -> BatchError {
        if self.owner.as_ref().is_some_and(|owner| owner.cancelled()) {
            return BatchError::Cancelled;
        }
        let stderr = self
            .stderr
            .lock()
            .map(|kept| String::from_utf8_lossy(&kept).trim().to_string())
            .unwrap_or_default();
        if stderr.is_empty() {
            BatchError::Protocol(what.to_string())
        } else {
            BatchError::Protocol(format!("{what}: {stderr}"))
        }
    }
}

impl Drop for CatFileBatch {
    fn drop(&mut self) {
        // Close the input first, so Git sees the end of its requests. With a
        // writer thread, the input closes after the requests already queued.
        self.queued.take();
        self.stdin.take();
        if self.own_group {
            // A reader that read every response lets Git exit by itself for
            // a moment, so it ends normally. Then end the group and reap the
            // leader: nothing this reader started outlives it.
            if !self.broken && self.unanswered == 0 {
                let until = Instant::now() + EXIT_GRACE;
                let mut pause = Duration::from_micros(50);
                while Instant::now() < until && matches!(self.child.try_wait(), Ok(None)) {
                    std::thread::sleep(pause);
                    pause = (pause * 2).min(Duration::from_millis(5));
                }
            }
            terminate_group(&mut self.child);
        } else if self.broken {
            // The stream is not trustworthy, so stop the process.
            terminate_child(&mut self.child);
        } else {
            // As `Command::output` does: read the remaining responses to
            // the end of the output, which also waits for anything Git
            // started that holds the pipe, then reap Git. More unread output
            // than [`DRAIN_LIMIT`] stops the process instead, so a reader
            // that ended early never reads a large remainder.
            let mut drained = 0;
            let mut ended = true;
            while let Ok(chunk) = self.chunks.recv() {
                drained += chunk.len();
                if drained > DRAIN_LIMIT {
                    ended = false;
                    break;
                }
            }
            if ended {
                // Stderr too, as `Command::output` waits for both pipes, so
                // the drop also waits for anything Git started that holds
                // only stderr.
                let _ = self.stderr_done.recv();
                let _ = self.child.wait();
            } else {
                terminate_child(&mut self.child);
            }
        }
        if let Some(owner) = &self.owner {
            owner.release(self.child.id());
        }
    }
}

fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Parse one header line. A missing or ambiguous answer must name the
/// request; an object answer must have a valid ID, type, and size.
fn parse_header(name: &str, line: &[u8]) -> Result<Header, BatchError> {
    if let Some(word) = line
        .strip_prefix(name.as_bytes())
        .and_then(|rest| rest.strip_prefix(b" "))
        && (word == b"missing" || word == b"ambiguous")
    {
        return Ok(Header::Missing);
    }
    let malformed = || {
        BatchError::Protocol(format!(
            "unexpected git cat-file response {:?}",
            String::from_utf8_lossy(&line[..line.len().min(200)])
        ))
    };
    let text = std::str::from_utf8(line).map_err(|_| malformed())?;
    let mut fields = text.split(' ');
    let (Some(oid), Some(kind), Some(size), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(malformed());
    };
    if !is_object_id(oid) || !matches!(kind, "blob" | "tree" | "commit" | "tag") {
        return Err(malformed());
    }
    if size.is_empty() || !size.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed());
    }
    let size = size.parse::<u64>().map_err(|_| malformed())?;
    Ok(Header::Object {
        oid: oid.to_string(),
        kind: kind.to_string(),
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests that start processes live in `tests/cat_file.rs`, a separate
    // test binary: a fork in this binary can briefly hold a sibling test's
    // file lock open.

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn headers_name_the_request_or_carry_a_valid_object() {
        assert_eq!(
            parse_header("abc:a b", b"abc:a b missing").unwrap(),
            Header::Missing
        );
        assert_eq!(
            parse_header("abc:p", b"abc:p ambiguous").unwrap(),
            Header::Missing
        );
        assert_eq!(
            parse_header("abc:p", format!("{OID} blob 12").as_bytes()).unwrap(),
            Header::Object {
                oid: OID.into(),
                kind: "blob".into(),
                size: 12
            }
        );
        for bad in [
            "other missing".to_string(),
            format!("{OID} blob"),
            format!("{OID} blob -1"),
            format!("{OID} blob +1"),
            format!("{OID} blob 1 extra"),
            format!("{OID} blobby 1"),
            format!("{} blob 1", &OID[..39]),
            format!("{} blob 1", OID.to_uppercase()),
            format!("{OID} blob 99999999999999999999999"),
            String::new(),
        ] {
            assert!(
                matches!(
                    parse_header("abc:p", bad.as_bytes()),
                    Err(BatchError::Protocol(_))
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn names_with_line_breaks_are_not_frameable() {
        assert!(frameable("abc:dir/a file.md"));
        assert!(frameable(OID));
        assert!(!frameable(""));
        assert!(!frameable("abc:a\nb"));
        assert!(!frameable("abc:a\r"));
    }
}
