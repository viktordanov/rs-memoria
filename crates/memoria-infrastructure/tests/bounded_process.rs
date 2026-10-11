//! The supervised process runner: fast and slow exits, both pipe drains,
//! output overflow, deadlines, and cancellation.
//!
//! These tests start processes, so they run in their own test binary: a
//! fork in the library's unit-test binary can briefly hold a sibling test's
//! file lock open.

use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use memoria_infrastructure::bounded_process::{SupervisedOutput, Supervisor};

fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    command
}

fn run(script: &str, budget_ms: u64, stdout_limit: u64) -> SupervisedOutput {
    let owner = Supervisor::new(Instant::now() + Duration::from_secs(60));
    owner
        .run(
            shell(script),
            Duration::from_millis(budget_ms),
            stdout_limit,
            true,
        )
        .unwrap()
}

#[test]
fn a_fast_child_and_a_slow_child_both_finish_normally() {
    let fast = run("printf done", 10_000, 1024);
    assert_eq!(fast.exit_code, Some(0));
    assert_eq!(fast.stdout, b"done");
    assert!(!fast.timed_out && !fast.output_truncated);
    // A child that outlives several backoff steps is still collected with
    // its exit code and output, not as a timeout.
    let slow = run("sleep 0.3; printf late; exit 7", 10_000, 1024);
    assert_eq!(slow.exit_code, Some(7));
    assert_eq!(slow.stdout, b"late");
    assert!(!slow.timed_out && !slow.output_truncated);
}

#[test]
fn both_pipes_are_drained_while_the_child_runs() {
    // Each stream is far larger than a pipe buffer. An undrained stderr
    // would block the child before it writes stdout.
    let output = run(
        "head -c 1000000 /dev/zero >&2; head -c 1000000 /dev/zero; printf x >&2",
        10_000,
        2_000_000,
    );
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(output.stdout.len(), 1_000_000);
    assert!(output.stderr.len() <= 8 * 1024, "stderr is capped");
    assert!(!output.timed_out && !output.output_truncated);
}

#[test]
fn an_output_overflow_is_truncation_not_a_timeout() {
    let output = run("head -c 1000000 /dev/zero; sleep 10", 10_000, 1_000);
    assert!(output.output_truncated, "the overflow was reported");
    assert!(!output.timed_out, "an overflow is not a timeout");
    assert!(output.stdout.len() <= 1_000);
}

#[test]
fn an_expired_deadline_is_a_timeout_not_truncation() {
    let output = run("sleep 10", 100, 1_000);
    assert!(output.timed_out);
    assert!(!output.output_truncated);
    assert_eq!(output.exit_code, None);
}

#[test]
fn a_cancelled_owner_stops_its_child_and_refuses_new_ones() {
    let owner = Supervisor::new(Instant::now() + Duration::from_secs(30));
    let canceller = Arc::clone(&owner);
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        canceller.cancel();
    });
    let output = owner
        .run(shell("sleep 10"), Duration::from_secs(30), 1024, false)
        .unwrap();
    handle.join().unwrap();
    // The group is killed, so the child never exits on its own. The loop
    // may see the kill before it sees the cancellation.
    assert_eq!(output.exit_code, None, "a cancelled child did not finish");
    let refused = owner
        .run(shell("true"), Duration::from_secs(1), 1024, false)
        .unwrap_err();
    assert_eq!(refused.operation, "cancelled");
}
