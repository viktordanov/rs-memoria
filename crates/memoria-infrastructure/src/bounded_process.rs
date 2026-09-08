//! Bounded child processes with one owner.
//!
//! The native hook endpoint must answer within a fixed deadline and must not
//! leave a process behind when it does. Every subprocess it starts —
//! repository discovery as well as the status inspection — runs through a
//! [`Supervisor`]. The supervisor gives each child its own session, so the
//! child's process group holds every descendant it spawns, bounds the child
//! by the deadline that remains, drains its pipes without accumulating
//! stderr, and terminates and reaps the whole group on a deadline, on an
//! output overflow, or when the owner cancels.

use std::io::Read as _;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use memoria_application::ports::{AdapterError, BoundedOutput, BoundedStatusProcess};

/// How long cleanup waits for a terminated child and its drain threads.
const CLEANUP_BUDGET: Duration = Duration::from_millis(250);

/// Captured stderr for a supervised command, in bytes. Everything above this
/// is read and discarded, so a flood cannot exhaust memory.
const STDERR_CAPTURE: usize = 8 * 1024;

/// One owner for every subprocess started under a single deadline.
///
/// The owner is a thread that may hand its work to another thread. When the
/// owner gives up, [`Supervisor::cancel`] terminates every process group that
/// is still running and refuses later spawns, so nothing outlives the
/// endpoint that started it.
#[derive(Debug)]
pub struct Supervisor {
    deadline: Instant,
    owned: Mutex<Owned>,
}

#[derive(Debug, Default)]
struct Owned {
    cancelled: bool,
    /// Process identifiers of live children. Each child leads its own
    /// process group, so its identifier names the whole group.
    groups: Vec<u32>,
}

/// One finished supervised command.
#[derive(Debug)]
pub struct SupervisedOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    /// Captured stderr, up to [`STDERR_CAPTURE`] bytes.
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub output_truncated: bool,
}

impl Supervisor {
    /// A supervisor whose children share one deadline.
    pub fn new(deadline: Instant) -> Arc<Supervisor> {
        Arc::new(Supervisor {
            deadline,
            owned: Mutex::new(Owned::default()),
        })
    }

    /// The time left before the shared deadline.
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Whether the owner has given up.
    pub fn cancelled(&self) -> bool {
        self.owned.lock().map(|o| o.cancelled).unwrap_or(true)
    }

    /// Terminate every owned process group and refuse later spawns.
    ///
    /// The endpoint calls this before it returns on a timeout. Signalling
    /// each group stops the child and every descendant it started.
    pub fn cancel(&self) {
        let groups = match self.owned.lock() {
            Ok(mut owned) => {
                owned.cancelled = true;
                std::mem::take(&mut owned.groups)
            }
            Err(_) => return,
        };
        for pid in groups {
            kill_group(pid);
        }
    }

    /// Take ownership of one live child. Returns `false` when the owner has
    /// already cancelled, in which case the caller must terminate it.
    fn adopt(&self, pid: u32) -> bool {
        match self.owned.lock() {
            Ok(mut owned) if !owned.cancelled => {
                owned.groups.push(pid);
                true
            }
            _ => false,
        }
    }

    fn release(&self, pid: u32) {
        if let Ok(mut owned) = self.owned.lock() {
            owned.groups.retain(|live| *live != pid);
        }
    }

    /// Run one command under the smaller of `budget` and the time that
    /// remains before the shared deadline.
    pub fn run(
        &self,
        command: Command,
        budget: Duration,
        stdout_limit: u64,
        capture_stderr: bool,
    ) -> Result<SupervisedOutput, AdapterError> {
        run_bounded(
            command,
            budget.min(self.remaining()),
            stdout_limit,
            capture_stderr,
            Some(self),
        )
    }
}

/// Terminate one process group by the identifier of its leader.
fn kill_group(pid: u32) {
    #[cfg(unix)]
    if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}

/// Terminate the child and every descendant in its process group, then reap.
///
/// The child was started in its own session, so its process group holds
/// every descendant it spawned. Signalling the group is what stops a Git
/// child that still holds the inherited pipes open.
fn terminate_group(child: &mut Child) {
    kill_group(child.id());
    let _ = child.kill();
    // Reap within a bound. A process stuck in uninterruptible sleep must not
    // hold the native endpoint past its own deadline.
    let until = Instant::now() + CLEANUP_BUDGET;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => {
                if Instant::now() >= until {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// Run one command in its own session, bounded in time and output.
fn run_bounded(
    mut command: Command,
    budget: Duration,
    stdout_limit: u64,
    capture_stderr: bool,
    supervisor: Option<&Supervisor>,
) -> Result<SupervisedOutput, AdapterError> {
    let program = command.get_program().to_string_lossy().to_string();
    let cancelled = || {
        AdapterError::new(
            "cancelled",
            Some(program.clone()),
            "the endpoint stopped before this command finished".to_string(),
        )
    };
    if supervisor.is_some_and(|owner| owner.cancelled()) {
        return Err(cancelled());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A new session, so the deadline can terminate descendants as a group.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt as _;
        // A failed setsid is not fatal: the deadline still applies.
        command.pre_exec(|| {
            let _ = rustix::process::setsid();
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|err| AdapterError::new("spawn", Some(program.clone()), err.to_string()))?;
    let pid = child.id();
    if let Some(owner) = supervisor
        && !owner.adopt(pid)
    {
        terminate_group(&mut child);
        return Err(cancelled());
    }

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    // Drain both pipes concurrently: a full stderr pipe would otherwise
    // block the child before it finishes writing stdout.
    let overflowed = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::channel();
    let limit = stdout_limit as usize;
    let overflow_flag = Arc::clone(&overflowed);
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        let mut truncated = false;
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if buffer.len() + read > limit {
                        let remaining = limit.saturating_sub(buffer.len());
                        buffer.extend_from_slice(&chunk[..remaining]);
                        truncated = true;
                        overflow_flag.store(true, Ordering::SeqCst);
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                }
            }
        }
        let _ = sender.send((buffer, truncated));
    });
    // Stderr is read to the end but kept only up to a small cap, so a flood
    // cannot exhaust memory while its diagnostic text stays available.
    let (error_sender, error_receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(read) = stderr.read(&mut chunk) {
            if read == 0 {
                break;
            }
            if capture_stderr && kept.len() < STDERR_CAPTURE {
                let room = STDERR_CAPTURE - kept.len();
                kept.extend_from_slice(&chunk[..read.min(room)]);
            }
        }
        let _ = error_sender.send(kept);
    });

    let deadline = Instant::now() + budget;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                let expired = Instant::now() >= deadline;
                let overflow = overflowed.load(Ordering::SeqCst);
                let stopped = supervisor.is_some_and(|owner| owner.cancelled());
                if expired || overflow || stopped {
                    timed_out = true;
                    terminate_group(&mut child);
                    break None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => {
                // Ownership is released only after cleanup, on this path too.
                terminate_group(&mut child);
                if let Some(owner) = supervisor {
                    owner.release(pid);
                }
                return Err(AdapterError::new("wait", None, err.to_string()));
            }
        }
    };
    // The leader has exited, but a descendant can still hold the pipes. The
    // group stays owned through cleanup, so a cancellation during this drain
    // still finds it and can end it.
    //
    // Each drain wait is bounded by the cleanup budget and by the deadline
    // that remains, so a descendant that holds the write end cannot extend
    // this call.
    let drain_budget = || match supervisor {
        Some(owner) => CLEANUP_BUDGET.min(owner.remaining()),
        None => CLEANUP_BUDGET,
    };
    let (buffer, output_truncated) = match receiver.recv_timeout(drain_budget()) {
        Ok(result) => result,
        Err(_) => {
            timed_out = true;
            terminate_group(&mut child);
            receiver
                .recv_timeout(CLEANUP_BUDGET)
                .unwrap_or((Vec::new(), false))
        }
    };
    let captured = if capture_stderr {
        match error_receiver.recv_timeout(drain_budget()) {
            Ok(text) => text,
            Err(_) => {
                timed_out = true;
                terminate_group(&mut child);
                error_receiver
                    .recv_timeout(CLEANUP_BUDGET)
                    .unwrap_or_default()
            }
        }
    } else {
        Vec::new()
    };
    // Nothing this command started may outlive it. The leader is gone, so
    // this signal reaches only descendants that are still running, and it
    // runs before ownership is released.
    terminate_group(&mut child);
    if let Some(owner) = supervisor {
        owner.release(pid);
    }
    // The drain threads are detached on purpose. Joining them could wait
    // on a descendant that inherited the pipe.
    Ok(SupervisedOutput {
        exit_code: status.and_then(|s| s.code()),
        stdout: buffer,
        stderr: captured,
        timed_out,
        output_truncated,
    })
}

pub struct SelfStatusProcess {
    executable: std::path::PathBuf,
    supervisor: Option<Arc<Supervisor>>,
}

impl SelfStatusProcess {
    pub fn new(executable: std::path::PathBuf) -> SelfStatusProcess {
        SelfStatusProcess {
            executable,
            supervisor: None,
        }
    }

    /// A status process owned by `supervisor`, so a late start still shares
    /// the endpoint's remaining deadline and its cancellation.
    pub fn supervised(
        executable: std::path::PathBuf,
        supervisor: Arc<Supervisor>,
    ) -> SelfStatusProcess {
        SelfStatusProcess {
            executable,
            supervisor: Some(supervisor),
        }
    }
}

impl BoundedStatusProcess for SelfStatusProcess {
    fn run_summary(
        &self,
        root: &str,
        deadline_ms: u64,
        stdout_limit: u64,
    ) -> Result<BoundedOutput, AdapterError> {
        let mut command = Command::new(&self.executable);
        command
            .arg("--root")
            .arg(root)
            .arg("status")
            .arg("--summary")
            .arg("--format")
            .arg("json");
        let budget = Duration::from_millis(deadline_ms);
        let output = match &self.supervisor {
            Some(owner) => owner.run(command, budget, stdout_limit, false)?,
            None => run_bounded(command, budget, stdout_limit, false, None)?,
        };
        Ok(BoundedOutput {
            exit_code: output.exit_code,
            stdout: output.stdout,
            timed_out: output.timed_out,
            output_truncated: output.output_truncated,
        })
    }
}
