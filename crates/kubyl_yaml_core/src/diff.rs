//! Line diffs between the live object and the buffer: gutter markers, unified and side-by-side
//! hunks, the change summary for confirmations, and the three-way merge used when the object
//! changes on the server while the buffer has edits.
//!
//! The live text is rendered the same way the buffer was (same key order, same masking), so a
//! diff shows exactly the lines the user edited. Server-managed fields (`resourceVersion`,
//! `managedFields`, `status`…) are ignored.

use std::collections::BTreeSet;

use similar::{DiffOp, TextDiff, TextMerge};

use crate::parse::{self, Path, Seg};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Equal,
    Added,
    Removed,
}

/// One line of a hunk. Line numbers are 0-based.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub tag: Tag,
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The YAML section the hunk starts in (`spec`, `metadata.labels`).
    pub section: String,
    pub lines: Vec<DiffLine>,
}

/// Gutter marker for a buffer line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    Added,
    Modified,
    /// Lines were removed just above this line.
    Removed,
}

/// What changed between two texts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineDiff {
    pub hunks: Vec<Hunk>,
    /// `(buffer line, marker)`, ascending.
    pub markers: Vec<(usize, Marker)>,
    pub added: usize,
    pub removed: usize,
    /// `~ spec.renewBefore`, `+ spec.dnsNames[1]`, `- metadata.labels.team`.
    pub summary: Vec<String>,
}

impl LineDiff {
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty()
    }

    /// Changed lines, counting a modified line once (the "N changes vs live" chip).
    pub fn change_count(&self) -> usize {
        self.markers.len()
    }
}

/// Paths that the server owns; changes there aren't the user's.
fn is_server_managed(path: &Path) -> bool {
    const IGNORED: &[&[&str]] = &[
        &["status"],
        &["metadata", "managedFields"],
        &["metadata", "resourceVersion"],
        &["metadata", "generation"],
        &["metadata", "uid"],
        &["metadata", "creationTimestamp"],
        &["metadata", "selfLink"],
    ];
    IGNORED.iter().any(|prefix| path.starts_with(prefix))
}

/// The YAML path of each line of `text` (`None` for blank lines, comments, `---`, or when the
/// text doesn't parse there).
pub fn line_paths(text: &str) -> Vec<Option<Path>> {
    let parsed = parse::parse(text);
    let mut out = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let content_start = offset + (line.len() - trimmed.len());
        let trimmed = trimmed.trim_end();
        let path = if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" {
            None
        } else {
            // Step over a list marker onto the item's content.
            let skip = trimmed.len() - trimmed.trim_start_matches(['-', ' ']).len();
            let at = content_start + skip;
            parsed
                .doc_at(at)
                .and_then(|d| d.root.as_ref())
                .map(|root| root.path_at(at).0)
        };
        out.push(path);
        offset += line.len();
    }
    out
}

/// The top-level section of a path (`spec`), like the mockup's `@@ spec @@`.
fn section_of(path: &Path) -> String {
    Path(path.0.iter().take(1).cloned().collect::<Vec<Seg>>()).to_string()
}

/// A line without its trailing comment and whitespace. Diffs compare these, so adding a
/// comment isn't a change vs live (the server drops comments), while edited lines still show
/// with their comments.
fn strip_comment(line: &str) -> &str {
    let mut quote = None;
    let mut prev_space = true;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '#') if prev_space => return line[..i].trim_end(),
            _ => {}
        }
        prev_space = c == ' ' || c == '\t';
    }
    line.trim_end()
}

fn normalized(text: &str) -> String {
    text.split_inclusive('\n')
        .map(|l| format!("{}\n", strip_comment(l.trim_end_matches(['\n', '\r']))))
        .collect()
}

/// Diffs `old` (live) against `new` (buffer) with `context` lines around changes.
pub fn diff(old: &str, new: &str, context: usize) -> LineDiff {
    let old_norm = normalized(old);
    let new_norm = normalized(new);
    let diff = TextDiff::from_lines(&old_norm, &new_norm);
    let old_blank: Vec<bool> = old_norm.lines().map(str::is_empty).collect();
    let new_blank: Vec<bool> = new_norm.lines().map(str::is_empty).collect();
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let old_paths = line_paths(old);
    let new_paths = line_paths(new);
    let ignored = |paths: &[Option<Path>], blank: &[bool], ix: usize| {
        blank.get(ix).copied().unwrap_or(false)
            || paths
                .get(ix)
                .and_then(Option::as_ref)
                .is_some_and(is_server_managed)
    };
    // Changes that only touch server-managed, blank or comment lines become equal.
    let ops: Vec<DiffOp> = diff
        .ops()
        .iter()
        .copied()
        .filter(|op| {
            let (tag, old_range, new_range) = op.as_tag_tuple();
            tag == similar::DiffTag::Equal
                || !(old_range
                    .clone()
                    .all(|i| ignored(&old_paths, &old_blank, i))
                    && new_range
                        .clone()
                        .all(|i| ignored(&new_paths, &new_blank, i)))
        })
        .collect();

    let mut out = LineDiff::default();
    let mut summary = BTreeSet::new();
    let text = |lines: &[&str], ix: usize| {
        lines
            .get(ix)
            .map(|l| l.trim_end_matches(['\n', '\r']).to_string())
            .unwrap_or_default()
    };
    for op in &ops {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for i in new_index..new_index + new_len {
                    out.markers.push((i, Marker::Added));
                    out.added += 1;
                    if let Some(Some(p)) = new_paths.get(i) {
                        summary.insert(format!("+ {p}"));
                    }
                }
            }
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => {
                out.markers.push((new_index, Marker::Removed));
                out.removed += old_len;
                for i in old_index..old_index + old_len {
                    if let Some(Some(p)) = old_paths.get(i) {
                        summary.insert(format!("- {p}"));
                    }
                }
            }
            DiffOp::Replace {
                old_index: _,
                old_len,
                new_index,
                new_len,
            } => {
                for i in new_index..new_index + new_len {
                    out.markers.push((i, Marker::Modified));
                    if let Some(Some(p)) = new_paths.get(i) {
                        summary.insert(format!("~ {p}"));
                    }
                }
                out.added += new_len;
                out.removed += old_len;
            }
        }
    }
    out.markers.sort_by_key(|(line, _)| *line);
    out.summary = summary.into_iter().collect();

    // Hunks: group changes with `context` equal lines around them.
    let mut current: Option<Hunk> = None;
    let mut pending_equal: Vec<DiffLine> = Vec::new();
    let flush = |current: &mut Option<Hunk>, out: &mut LineDiff| {
        if let Some(hunk) = current.take() {
            out.hunks.push(hunk);
        }
    };
    for op in &ops {
        let (tag, old_range, new_range) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            for (o, n) in old_range.zip(new_range) {
                pending_equal.push(DiffLine {
                    tag: Tag::Equal,
                    old: Some(o),
                    new: Some(n),
                    text: text(&new_lines, n),
                });
            }
            continue;
        }
        // Trailing context of the previous hunk, or a gap that closes it.
        if let Some(hunk) = current.as_mut() {
            if pending_equal.len() <= context * 2 {
                hunk.lines.append(&mut pending_equal);
            } else {
                hunk.lines.extend(pending_equal.drain(..context));
                flush(&mut current, &mut out);
            }
        }
        if current.is_none() {
            let first_path = new_paths
                .get(new_range.start)
                .cloned()
                .flatten()
                .or_else(|| old_paths.get(old_range.start).cloned().flatten());
            let lead = pending_equal.len().saturating_sub(context);
            current = Some(Hunk {
                section: first_path.as_ref().map(section_of).unwrap_or_default(),
                lines: pending_equal.split_off(lead),
            });
        }
        pending_equal.clear();
        let hunk = current.as_mut().expect("hunk");
        for o in old_range {
            hunk.lines.push(DiffLine {
                tag: Tag::Removed,
                old: Some(o),
                new: None,
                text: text(&old_lines, o),
            });
        }
        for n in new_range {
            hunk.lines.push(DiffLine {
                tag: Tag::Added,
                old: None,
                new: Some(n),
                text: text(&new_lines, n),
            });
        }
    }
    if let Some(hunk) = current.as_mut() {
        let keep = pending_equal.len().min(context);
        hunk.lines.extend(pending_equal.drain(..keep));
    }
    flush(&mut current, &mut out);
    out
}

/// One row of a side-by-side view: the old line on the left, the new one on the right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideBySide {
    pub left: Option<(usize, String)>,
    pub right: Option<(usize, String)>,
    pub changed: bool,
}

/// Pairs removed and added lines of a hunk for a side-by-side view.
pub fn side_by_side(hunk: &Hunk) -> Vec<SideBySide> {
    let mut rows = Vec::new();
    let mut removed: Vec<(usize, String)> = Vec::new();
    let mut added: Vec<(usize, String)> = Vec::new();
    let flush = |rows: &mut Vec<SideBySide>,
                 removed: &mut Vec<(usize, String)>,
                 added: &mut Vec<(usize, String)>| {
        let n = removed.len().max(added.len());
        let mut r = removed.drain(..);
        let mut a = added.drain(..);
        for _ in 0..n {
            rows.push(SideBySide {
                left: r.next(),
                right: a.next(),
                changed: true,
            });
        }
    };
    for line in &hunk.lines {
        match line.tag {
            Tag::Removed => removed.push((line.old.unwrap_or_default(), line.text.clone())),
            Tag::Added => added.push((line.new.unwrap_or_default(), line.text.clone())),
            Tag::Equal => {
                flush(&mut rows, &mut removed, &mut added);
                rows.push(SideBySide {
                    left: line.old.map(|o| (o, line.text.clone())),
                    right: line.new.map(|n| (n, line.text.clone())),
                    changed: false,
                });
            }
        }
    }
    flush(&mut rows, &mut removed, &mut added);
    rows
}

/// Result of merging the server's new version into an edited buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Merge {
    Clean(String),
    /// Both sides changed the same lines; the user picks reload or keep mine.
    Conflict,
}

/// Three-way merge: `base` is what the buffer was loaded from, `ours` the buffer, `theirs`
/// the new live text.
pub fn merge(base: &str, ours: &str, theirs: &str) -> Merge {
    let merged = TextMerge::from_lines(base, ours, theirs);
    if merged.is_conflicted() {
        Merge::Conflict
    } else {
        Merge::Clean(merged.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = "apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: api-tls
  resourceVersion: \"614\"
spec:
  secretName: api-tls
  renewBefore: 360h
  dnsNames:
  - api.payments.example.com
  issuerRef:
    name: letsencrypt-prod
";

    #[test]
    fn diff_shows_exactly_the_edited_lines() {
        let edited = LIVE
            .replace("renewBefore: 360h", "renewBefore: 720h # 30d")
            .replace(
                "  - api.payments.example.com\n",
                "  - api.payments.example.com\n  - checkout.payments.example.com\n",
            );
        let d = diff(LIVE, &edited, 1);
        assert_eq!(d.markers, [(7, Marker::Modified), (10, Marker::Added)]);
        assert_eq!(d.hunks.len(), 1);
        let hunk = &d.hunks[0];
        assert_eq!(hunk.section, "spec");
        let changed: Vec<_> = hunk
            .lines
            .iter()
            .filter(|l| l.tag != Tag::Equal)
            .map(|l| (l.tag, l.text.as_str()))
            .collect();
        assert_eq!(
            changed,
            [
                (Tag::Removed, "  renewBefore: 360h"),
                (Tag::Added, "  renewBefore: 720h # 30d"),
                (Tag::Added, "  - checkout.payments.example.com"),
            ]
        );
        assert_eq!(d.summary, ["+ spec.dnsNames[1]", "~ spec.renewBefore"]);
        assert_eq!(d.change_count(), 2);
    }

    #[test]
    fn comments_alone_are_not_changes() {
        let edited = LIVE
            .replace("renewBefore: 360h", "renewBefore: 360h # 15d")
            .replace("spec:\n", "spec:\n  # managed by helm\n");
        assert!(diff(LIVE, &edited, 3).is_empty());
        assert_eq!(strip_comment("a: \"x # y\" # z"), "a: \"x # y\"");
        assert_eq!(strip_comment("url: http://x/#frag"), "url: http://x/#frag");
    }

    #[test]
    fn server_managed_changes_are_ignored() {
        let newer = LIVE.replace("\"614\"", "\"700\"");
        assert!(diff(&newer, LIVE, 3).is_empty());
    }

    #[test]
    fn removed_lines_mark_the_next_line() {
        let edited = LIVE.replace("  secretName: api-tls\n", "");
        let d = diff(LIVE, &edited, 0);
        assert_eq!(d.markers, [(6, Marker::Removed)]);
        assert_eq!(d.summary, ["- spec.secretName"]);
        let rows = side_by_side(&d.hunks[0]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].right.is_none());
    }

    #[test]
    fn merges_non_overlapping_changes() {
        let ours = LIVE.replace("360h", "720h");
        let theirs = LIVE.replace("letsencrypt-prod", "letsencrypt-staging");
        assert_eq!(
            merge(LIVE, &ours, &theirs),
            Merge::Clean(
                LIVE.replace("360h", "720h")
                    .replace("letsencrypt-prod", "letsencrypt-staging")
            )
        );
        let theirs = LIVE.replace("360h", "100h");
        assert_eq!(merge(LIVE, &ours, &theirs), Merge::Conflict);
    }
}
