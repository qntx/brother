//! Snapshot and screenshot diffing utilities.
//!
//! - **Text diff**: Compare accessibility snapshots using the Myers algorithm.
//! - **Screenshot diff**: Compare raw RGBA pixel buffers (decoded by the
//!   daemon via the browser Canvas API, not in this module).

use std::fmt;

use serde::Serialize;

/// Result of comparing two text snapshots.
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotDiff {
    /// Unified diff output.
    pub diff: String,
    /// Number of added lines.
    pub added: usize,
    /// Number of removed lines.
    pub removed: usize,
    /// Number of unchanged lines.
    pub unchanged: usize,
}

impl SnapshotDiff {
    /// Whether the two snapshots are identical.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0
    }

    /// Summary string: `"+N -M (~U unchanged)"`.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "+{} -{} (~{} unchanged)",
            self.added, self.removed, self.unchanged
        )
    }
}

impl fmt::Display for SnapshotDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.diff)
    }
}

/// Compare two text snapshots and produce a unified diff.
///
/// Uses the Myers diff algorithm (O((N+M)D) time, O(N+M) space).
///
/// # Example
///
/// ```
/// let before = "- heading \"Hello\" [ref=e1]";
/// let after = "- heading \"Hello\" [ref=e1]\n- link \"New\" [ref=e2]";
/// let diff = brother::diff_snapshots(before, after);
/// assert_eq!(diff.added, 1);
/// assert!(diff.diff.contains("+ - link"));
/// ```
#[must_use]
pub fn diff_snapshots(before: &str, after: &str) -> SnapshotDiff {
    let old_lines: Vec<&str> = before.lines().collect();
    let new_lines: Vec<&str> = after.lines().collect();

    let edits = myers_diff(&old_lines, &new_lines);

    let mut diff = String::new();
    let mut added: usize = 0;
    let mut removed: usize = 0;
    let mut unchanged: usize = 0;

    for edit in &edits {
        match edit {
            Edit::Equal(line) => {
                diff.push_str("  ");
                diff.push_str(line);
                diff.push('\n');
                unchanged += 1;
            }
            Edit::Insert(line) => {
                diff.push_str("+ ");
                diff.push_str(line);
                diff.push('\n');
                added += 1;
            }
            Edit::Delete(line) => {
                diff.push_str("- ");
                diff.push_str(line);
                diff.push('\n');
                removed += 1;
            }
        }
    }

    SnapshotDiff {
        diff,
        added,
        removed,
        unchanged,
    }
}

/// An edit operation in the diff.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Edit<'a> {
    Equal(&'a str),
    Insert(&'a str),
    Delete(&'a str),
}

/// Myers diff algorithm — produces a minimal edit script.
///
/// Stores a snapshot of `v` after each edit-distance step, then backtracks
/// through the snapshots to reconstruct the edit sequence.
#[allow(clippy::many_single_char_names, clippy::suspicious_operation_groupings)]
fn myers_diff<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Edit<'a>> {
    let n = old.len();
    let m = new.len();

    if n == 0 && m == 0 {
        return Vec::new();
    }
    if n == 0 {
        return new.iter().map(|l| Edit::Insert(l)).collect();
    }
    if m == 0 {
        return old.iter().map(|l| Edit::Delete(l)).collect();
    }

    let max_d = n + m;
    let offset = max_d;
    let v_len = 2 * max_d + 1;
    let mut v = vec![0usize; v_len];
    let mut trace: Vec<Vec<usize>> = Vec::new();

    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    'outer: for d in 0..=max_d {
        for k_off in (0..=2 * d).step_by(2) {
            let k: isize = k_off as isize - d as isize;
            let ki = (k + offset as isize) as usize;

            let mut x = if k == -(d as isize)
                || (k != d as isize && v[ki - 1] < v[ki + 1])
            {
                v[ki + 1]
            } else {
                v[ki - 1] + 1
            };
            let mut y = (x as isize - k) as usize;

            while x < n && y < m && old[x] == new[y] {
                x += 1;
                y += 1;
            }

            v[ki] = x;

            if x >= n && y >= m {
                trace.push(v.clone());
                break 'outer;
            }
        }
        trace.push(v.clone());
    }

    backtrack_myers(old, new, &trace, offset)
}

/// Backtrack through the Myers trace to produce edit operations.
///
/// `trace[d]` holds the `v` array state **after** processing edit-distance `d`.
/// To reconstruct step `d`, we look at `trace[d-1]` (the state from the
/// previous step) to determine the non-diagonal move direction.
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn backtrack_myers<'a>(
    old: &[&'a str],
    new: &[&'a str],
    trace: &[Vec<usize>],
    offset: usize,
) -> Vec<Edit<'a>> {
    let mut edits = Vec::new();
    let mut x = old.len();
    let mut y = new.len();

    for d in (0..trace.len()).rev() {
        let k: isize = x as isize - y as isize;

        if d == 0 {
            // Edit distance 0: only diagonal (equal) moves from (0,0).
            while x > 0 && y > 0 {
                x -= 1;
                y -= 1;
                edits.push(Edit::Equal(old[x]));
            }
            break;
        }

        let prev_v = &trace[d - 1];
        let ki = (k + offset as isize) as usize;

        let came_from_above = k == -(d as isize)
            || (k != d as isize
                && prev_v.get(ki.wrapping_sub(1)).copied().unwrap_or(0)
                    < prev_v.get(ki + 1).copied().unwrap_or(0));

        let prev_k: isize = if came_from_above { k + 1 } else { k - 1 };
        let prev_ki = (prev_k + offset as isize) as usize;
        let end_x = prev_v.get(prev_ki).copied().unwrap_or(0);
        let end_y = (end_x as isize - prev_k) as usize;

        // Position after the non-diagonal move (before diagonals).
        let (mid_x, mid_y) = if came_from_above {
            (end_x, end_y + 1) // insert: moved down
        } else {
            (end_x + 1, end_y) // delete: moved right
        };

        // Emit diagonal (equal) moves from (mid_x, mid_y) to (x, y).
        while x > mid_x && y > mid_y {
            x -= 1;
            y -= 1;
            edits.push(Edit::Equal(old[x]));
        }

        // Emit the non-diagonal move.
        if came_from_above {
            y -= 1;
            edits.push(Edit::Insert(new[y]));
        } else {
            x -= 1;
            edits.push(Edit::Delete(old[x]));
        }
    }

    edits.reverse();
    edits
}

/// Result of comparing two screenshots pixel-by-pixel.
///
/// The raw RGBA buffers are produced by the daemon decoding PNGs via the
/// browser Canvas API (see `Page::decode_png_to_rgba`). This struct only
/// holds the comparison result.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ScreenshotDiff {
    /// Total number of pixels compared.
    pub total_pixels: u64,
    /// Number of pixels that differ.
    pub diff_pixels: u64,
    /// Percentage of pixels that differ (0.0 – 100.0).
    pub diff_percentage: f64,
    /// Whether the images have different dimensions.
    pub size_mismatch: bool,
    /// Width of image A.
    pub width_a: u32,
    /// Height of image A.
    pub height_a: u32,
    /// Width of image B.
    pub width_b: u32,
    /// Height of image B.
    pub height_b: u32,
}

impl ScreenshotDiff {
    /// Whether the two screenshots are pixel-identical.
    #[must_use]
    pub const fn is_identical(&self) -> bool {
        !self.size_mismatch && self.diff_pixels == 0
    }

    /// Human-readable summary.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.size_mismatch {
            format!(
                "size mismatch: {}x{} vs {}x{}",
                self.width_a, self.height_a, self.width_b, self.height_b,
            )
        } else if self.diff_pixels == 0 {
            "identical".to_owned()
        } else {
            format!(
                "{} pixels differ ({:.2}%)",
                self.diff_pixels, self.diff_percentage,
            )
        }
    }
}

impl fmt::Display for ScreenshotDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.summary())
    }
}

/// Compare two raw RGBA pixel buffers.
///
/// `threshold` (0–255): per-channel tolerance. If any R/G/B channel
/// differs by more than `threshold`, the pixel counts as different.
/// Alpha is ignored.
///
/// The caller is responsible for decoding PNG screenshots into RGBA
/// buffers (e.g. via the browser Canvas API in the daemon).
#[must_use]
pub fn diff_rgba(
    rgba_a: &[u8],
    width_a: u32,
    height_a: u32,
    rgba_b: &[u8],
    width_b: u32,
    height_b: u32,
    threshold: u8,
) -> ScreenshotDiff {
    if width_a != width_b || height_a != height_b {
        return ScreenshotDiff {
            total_pixels: 0,
            diff_pixels: 0,
            diff_percentage: 100.0,
            size_mismatch: true,
            width_a,
            height_a,
            width_b,
            height_b,
        };
    }

    let total = u64::from(width_a) * u64::from(height_a);
    let mut diff_count: u64 = 0;
    let thresh = i16::from(threshold);

    for (chunk_a, chunk_b) in rgba_a.chunks_exact(4).zip(rgba_b.chunks_exact(4)) {
        let dr = (i16::from(chunk_a[0]) - i16::from(chunk_b[0])).abs();
        let dg = (i16::from(chunk_a[1]) - i16::from(chunk_b[1])).abs();
        let db = (i16::from(chunk_a[2]) - i16::from(chunk_b[2])).abs();
        if dr > thresh || dg > thresh || db > thresh {
            diff_count += 1;
        }
    }

    #[allow(clippy::cast_precision_loss)]
    let pct = if total > 0 {
        (diff_count as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    ScreenshotDiff {
        total_pixels: total,
        diff_pixels: diff_count,
        diff_percentage: pct,
        size_mismatch: false,
        width_a,
        height_a,
        width_b,
        height_b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_identical() {
        let text = "- heading \"Hello\" [ref=e1]\n- link \"Click\" [ref=e2]";
        let diff = diff_snapshots(text, text);
        assert!(diff.is_empty());
        assert_eq!(diff.added, 0);
        assert_eq!(diff.removed, 0);
        assert_eq!(diff.unchanged, 2);
    }

    #[test]
    fn diff_additions() {
        let before = "- heading \"Hello\" [ref=e1]";
        let after = "- heading \"Hello\" [ref=e1]\n- link \"New\" [ref=e2]";
        let diff = diff_snapshots(before, after);
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 0);
        assert_eq!(diff.unchanged, 1);
        assert!(diff.diff.contains("+ - link \"New\""));
    }

    #[test]
    fn diff_deletions() {
        let before = "- heading \"Hello\" [ref=e1]\n- link \"Old\" [ref=e2]";
        let after = "- heading \"Hello\" [ref=e1]";
        let diff = diff_snapshots(before, after);
        assert_eq!(diff.added, 0);
        assert_eq!(diff.removed, 1);
        assert_eq!(diff.unchanged, 1);
        assert!(diff.diff.contains("- - link \"Old\""));
    }

    #[test]
    fn diff_modifications() {
        let before = "- heading \"Hello\" [ref=e1]\n- link \"Old\" [ref=e2]";
        let after = "- heading \"Hello\" [ref=e1]\n- link \"New\" [ref=e2]";
        let diff = diff_snapshots(before, after);
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 1);
        assert_eq!(diff.unchanged, 1);
    }

    #[test]
    fn diff_empty_inputs() {
        let diff = diff_snapshots("", "");
        assert!(diff.is_empty());
    }

    #[test]
    fn diff_from_empty() {
        let diff = diff_snapshots("", "line1\nline2");
        assert_eq!(diff.added, 2);
        assert_eq!(diff.removed, 0);
    }

    #[test]
    fn diff_to_empty() {
        let diff = diff_snapshots("line1\nline2", "");
        assert_eq!(diff.added, 0);
        assert_eq!(diff.removed, 2);
    }

    #[test]
    fn rgba_identical() {
        let rgba = vec![255u8; 4 * 4]; // 2x2 white
        let diff = diff_rgba(&rgba, 2, 2, &rgba, 2, 2, 0);
        assert!(diff.is_identical());
        assert_eq!(diff.total_pixels, 4);
    }

    #[test]
    fn rgba_size_mismatch() {
        let a = vec![0u8; 4 * 4];
        let b = vec![0u8; 4 * 6];
        let diff = diff_rgba(&a, 2, 2, &b, 3, 2, 0);
        assert!(diff.size_mismatch);
    }

    #[test]
    fn rgba_all_different() {
        let a = vec![0u8; 4 * 4]; // 2x2 black
        let b = vec![255u8; 4 * 4]; // 2x2 white
        let diff = diff_rgba(&a, 2, 2, &b, 2, 2, 0);
        assert_eq!(diff.diff_pixels, 4);
        assert!((diff.diff_percentage - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn rgba_threshold() {
        let a = vec![100u8; 4 * 4];
        let mut b = vec![100u8; 4 * 4];
        b[0] = 105; // 5 difference on first pixel R channel
        let diff = diff_rgba(&a, 2, 2, &b, 2, 2, 10);
        assert!(diff.is_identical()); // within threshold
        let diff2 = diff_rgba(&a, 2, 2, &b, 2, 2, 3);
        assert_eq!(diff2.diff_pixels, 1); // exceeds threshold
    }
}
