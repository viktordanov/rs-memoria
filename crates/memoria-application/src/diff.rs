//! Line-oriented text diff for review packets and conflict explanations.

/// Upper bound on lines per side before the diff is omitted.
pub const MAX_DIFF_LINES: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffText {
    /// A unified-style hunk listing without file headers.
    Unified(String),
    /// Both inputs are identical.
    Identical,
    /// One input is not valid UTF-8.
    Binary,
    /// Too many lines to diff within bounds.
    TooLarge,
}

/// Diff two texts by line.
pub fn diff_bytes(old: &[u8], new: &[u8]) -> DiffText {
    if old == new {
        return DiffText::Identical;
    }
    let (Ok(old_text), Ok(new_text)) = (std::str::from_utf8(old), std::str::from_utf8(new)) else {
        return DiffText::Binary;
    };
    let old_lines: Vec<&str> = split_lines(old_text);
    let new_lines: Vec<&str> = split_lines(new_text);
    if old_lines.len() > MAX_DIFF_LINES || new_lines.len() > MAX_DIFF_LINES {
        return DiffText::TooLarge;
    }
    let ops = myers(&old_lines, &new_lines);
    DiffText::Unified(render_unified(&old_lines, &new_lines, &ops))
}

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split_inclusive('\n').collect();
    if let Some(last) = lines.last()
        && last.is_empty()
    {
        lines.pop();
    }
    lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// Myers O(ND) diff returning edit operations.
fn myers(a: &[&str], b: &[&str]) -> Vec<Op> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    let max = n + m;
    let offset = max;
    let width = (2 * max + 1) as usize;
    let mut v = vec![0isize; width.max(1)];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    'outer: for d in 0..=max {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let index = (k + offset) as usize;
            let mut x = if k == -d || (k != d && v[index - 1] < v[index + 1]) {
                v[index + 1]
            } else {
                v[index - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[index] = x;
            if x >= n && y >= m {
                break 'outer;
            }
            k += 2;
        }
    }
    // Backtrack.
    let mut ops = Vec::new();
    let mut x = n;
    let mut y = m;
    for d in (0..trace.len() as isize).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let index = (k + offset) as usize;
        let prev_k = if k == -d || (k != d && v[index - 1] < v[index + 1]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v[(prev_k + offset) as usize];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Equal((x - 1) as usize, (y - 1) as usize));
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                ops.push(Op::Insert((y - 1) as usize));
            } else {
                ops.push(Op::Delete((x - 1) as usize));
            }
        }
        x = prev_x;
        y = prev_y;
    }
    ops.reverse();
    ops
}

fn render_unified(a: &[&str], b: &[&str], ops: &[Op]) -> String {
    const CONTEXT: usize = 3;
    let mut out = String::new();
    let mut i = 0;
    while i < ops.len() {
        if matches!(ops[i], Op::Equal(..)) {
            i += 1;
            continue;
        }
        let start = i.saturating_sub(CONTEXT);
        let mut end = i;
        let mut equal_run = 0;
        while end < ops.len() {
            match ops[end] {
                Op::Equal(..) => {
                    equal_run += 1;
                    if equal_run > CONTEXT * 2 {
                        break;
                    }
                }
                _ => equal_run = 0,
            }
            end += 1;
        }
        let end = if equal_run > CONTEXT {
            end - (equal_run - CONTEXT)
        } else {
            end
        };
        let hunk = &ops[start..end];
        let (old_start, old_count, new_start, new_count) = hunk_bounds(hunk);
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start + 1,
            old_count,
            new_start + 1,
            new_count
        ));
        for op in hunk {
            match op {
                Op::Equal(x, _) => push_line(&mut out, ' ', a[*x]),
                Op::Delete(x) => push_line(&mut out, '-', a[*x]),
                Op::Insert(y) => push_line(&mut out, '+', b[*y]),
            }
        }
        i = end;
    }
    out
}

fn hunk_bounds(hunk: &[Op]) -> (usize, usize, usize, usize) {
    let mut old_start = usize::MAX;
    let mut new_start = usize::MAX;
    let mut old_count = 0;
    let mut new_count = 0;
    for op in hunk {
        match op {
            Op::Equal(x, y) => {
                old_start = old_start.min(*x);
                new_start = new_start.min(*y);
                old_count += 1;
                new_count += 1;
            }
            Op::Delete(x) => {
                old_start = old_start.min(*x);
                old_count += 1;
            }
            Op::Insert(y) => {
                new_start = new_start.min(*y);
                new_count += 1;
            }
        }
    }
    (
        if old_start == usize::MAX {
            0
        } else {
            old_start
        },
        old_count,
        if new_start == usize::MAX {
            0
        } else {
            new_start
        },
        new_count,
    )
}

fn push_line(out: &mut String, prefix: char, line: &str) {
    out.push(prefix);
    out.push_str(line);
    if !line.ends_with('\n') {
        out.push_str("\n\\ No newline at end of file\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_and_binary() {
        assert_eq!(diff_bytes(b"a\n", b"a\n"), DiffText::Identical);
        assert_eq!(diff_bytes(b"\xff", b"a"), DiffText::Binary);
    }

    #[test]
    fn unified_output_lists_changes() {
        let old = "one\ntwo\nthree\nfour\n";
        let new = "one\n2\nthree\nfour\nfive\n";
        match diff_bytes(old.as_bytes(), new.as_bytes()) {
            DiffText::Unified(text) => {
                assert!(text.contains("-two\n"));
                assert!(text.contains("+2\n"));
                assert!(text.contains("+five\n"));
                assert!(text.starts_with("@@ -1,4 +1,5 @@\n"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn handles_missing_trailing_newline() {
        match diff_bytes(b"a\nb", b"a\nc") {
            DiffText::Unified(text) => assert!(text.contains("\\ No newline at end of file")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn myers_edit_script_reconstructs_new() {
        let a = ["a", "b", "c", "d"];
        let b = ["a", "c", "d", "e"];
        let ops = myers(&a, &b);
        let mut rebuilt = Vec::new();
        for op in ops {
            match op {
                Op::Equal(_, y) | Op::Insert(y) => rebuilt.push(b[y]),
                Op::Delete(_) => {}
            }
        }
        assert_eq!(rebuilt, b);
    }
}
