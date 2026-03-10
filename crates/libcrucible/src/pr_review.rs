use crate::report::{
    CanonicalIssue, PullRequestCommentDraft, PullRequestCommentMappingStatus,
    PullRequestCommentSide, PullRequestOverviewComment, PullRequestReviewDraft,
};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub fn build_review_draft(
    overview_body: String,
    issues: &[CanonicalIssue],
    diff: &str,
) -> PullRequestReviewDraft {
    let index = DiffIndex::from_patch(diff);
    let mut inline_comments = Vec::new();
    let mut overview_only_comments = Vec::new();

    for issue in issues {
        let draft = build_comment_draft(issue, &index);
        if draft.mapping_status == PullRequestCommentMappingStatus::Inline {
            inline_comments.push(draft);
        } else {
            overview_only_comments.push(draft);
        }
    }

    PullRequestReviewDraft {
        overview_comment: PullRequestOverviewComment {
            body: overview_body,
        },
        inline_comments,
        overview_only_comments,
    }
}

fn build_comment_draft(issue: &CanonicalIssue, index: &DiffIndex) -> PullRequestCommentDraft {
    let body = render_comment_body(issue);
    let mut draft = PullRequestCommentDraft {
        severity: issue.severity.clone(),
        category: issue.category.clone(),
        title: issue.title.clone(),
        description: issue.description.clone(),
        body,
        path: issue.file.clone(),
        line: None,
        side: None,
        start_line: None,
        start_side: None,
        source_agents: issue.raised_by.clone(),
        mapping_status: PullRequestCommentMappingStatus::OverviewOnly,
        mapping_note: Some("Issue could not be mapped to a changed diff hunk".to_string()),
    };

    let Some(path) = issue.file.as_ref() else {
        return draft;
    };
    let Some(start) = issue.line_start else {
        return draft;
    };
    let end = issue.line_end.unwrap_or(start);

    if let Some(mapped) = index.map_range(path, start, end) {
        draft.path = Some(path.clone());
        draft.line = Some(mapped.line);
        draft.side = Some(mapped.side);
        draft.start_line = mapped.start_line;
        draft.start_side = mapped.start_side;
        draft.mapping_status = PullRequestCommentMappingStatus::Inline;
        draft.mapping_note = None;
    }

    draft
}

fn render_comment_body(issue: &CanonicalIssue) -> String {
    let mut out = format!(
        "**{}**\n\n{}\n\nRaised by: {}",
        issue.title,
        issue.description,
        issue.raised_by.join(", ")
    );
    if let Some(fix) = &issue.suggested_fix {
        out.push_str(&format!("\n\nSuggested fix: {}", fix));
    }
    out
}

#[derive(Debug, Default)]
struct DiffIndex {
    files: HashMap<PathBuf, FileLines>,
}

#[derive(Debug, Default)]
struct FileLines {
    left: BTreeSet<u32>,
    right: BTreeSet<u32>,
}

#[derive(Debug, Clone, Copy)]
struct MappedRange {
    line: u32,
    side: PullRequestCommentSide,
    start_line: Option<u32>,
    start_side: Option<PullRequestCommentSide>,
}

impl DiffIndex {
    fn from_patch(diff: &str) -> Self {
        let mut index = DiffIndex::default();
        let mut current_file: Option<PathBuf> = None;
        let mut old_line = 0u32;
        let mut new_line = 0u32;

        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("+++ b/") {
                if path != "/dev/null" {
                    current_file = Some(PathBuf::from(path));
                }
                continue;
            }
            if let Some((old_start, new_start)) = parse_hunk_header(line) {
                old_line = old_start;
                new_line = new_start;
                continue;
            }
            let Some(file) = current_file.as_ref() else {
                continue;
            };
            let Some(first) = line.chars().next() else {
                continue;
            };
            let entry = index.files.entry(file.clone()).or_default();
            match first {
                ' ' => {
                    entry.left.insert(old_line);
                    entry.right.insert(new_line);
                    old_line += 1;
                    new_line += 1;
                }
                '+' => {
                    entry.right.insert(new_line);
                    new_line += 1;
                }
                '-' => {
                    entry.left.insert(old_line);
                    old_line += 1;
                }
                _ => {}
            }
        }

        index
    }

    fn map_range(&self, path: &Path, start: u32, end: u32) -> Option<MappedRange> {
        let file = self.files.get(path)?;
        if contains_range(&file.right, start, end) {
            return Some(MappedRange {
                line: end,
                side: PullRequestCommentSide::Right,
                start_line: (start != end).then_some(start),
                start_side: (start != end).then_some(PullRequestCommentSide::Right),
            });
        }
        if contains_range(&file.left, start, end) {
            return Some(MappedRange {
                line: end,
                side: PullRequestCommentSide::Left,
                start_line: (start != end).then_some(start),
                start_side: (start != end).then_some(PullRequestCommentSide::Left),
            });
        }
        if file.right.contains(&start) {
            return Some(MappedRange {
                line: start,
                side: PullRequestCommentSide::Right,
                start_line: None,
                start_side: None,
            });
        }
        if file.left.contains(&start) {
            return Some(MappedRange {
                line: start,
                side: PullRequestCommentSide::Left,
                start_line: None,
                start_side: None,
            });
        }
        None
    }
}

fn contains_range(lines: &BTreeSet<u32>, start: u32, end: u32) -> bool {
    (start..=end).all(|line| lines.contains(&line))
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    if !line.starts_with("@@ ") {
        return None;
    }
    let mut parts = line.split_whitespace();
    let _at = parts.next()?;
    let old = parts.next()?;
    let new = parts.next()?;
    Some((parse_hunk_start(old)?, parse_hunk_start(new)?))
}

fn parse_hunk_start(part: &str) -> Option<u32> {
    let trimmed = part.trim_start_matches(['-', '+']);
    let start = trimmed.split(',').next()?;
    start.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{CanonicalIssue, Severity};

    #[test]
    fn maps_added_line_to_right_side_comment() {
        let diff = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1,2 @@\n old\n+new\n";
        let issue = CanonicalIssue {
            severity: Severity::Warning,
            category: "maintainability".to_string(),
            file: Some(PathBuf::from("src/lib.rs")),
            line_start: Some(2),
            line_end: Some(2),
            title: "Issue".to_string(),
            description: "Desc".to_string(),
            suggested_fix: None,
            raised_by: vec!["codex".to_string()],
            evidence: Vec::new(),
        };

        let draft = build_review_draft("summary".to_string(), &[issue], diff);
        assert_eq!(draft.inline_comments.len(), 1);
        let comment = &draft.inline_comments[0];
        assert_eq!(comment.side, Some(PullRequestCommentSide::Right));
        assert_eq!(comment.line, Some(2));
    }

    #[test]
    fn falls_back_when_issue_cannot_be_mapped() {
        let diff = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1,2 @@\n old\n+new\n";
        let issue = CanonicalIssue {
            severity: Severity::Warning,
            category: "maintainability".to_string(),
            file: Some(PathBuf::from("src/lib.rs")),
            line_start: Some(99),
            line_end: Some(99),
            title: "Issue".to_string(),
            description: "Desc".to_string(),
            suggested_fix: None,
            raised_by: vec!["codex".to_string()],
            evidence: Vec::new(),
        };

        let draft = build_review_draft("summary".to_string(), &[issue], diff);
        assert!(draft.inline_comments.is_empty());
        assert_eq!(draft.overview_only_comments.len(), 1);
    }
}
