use crate::config::ContextConfig;
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    pub symbol: String,
    pub file: PathBuf,
    pub line: u32,
    pub snippet: String,
}

#[derive(Debug, Clone, Copy)]
pub enum SymbolKind {
    Function,
    Struct,
    Trait,
    Impl,
    Constant,
}

pub struct ReferenceCollector;

impl ReferenceCollector {
    pub fn collect(diff: &str, repo_root: &Path, cfg: &ContextConfig) -> Result<Vec<Reference>> {
        let symbols = extract_symbols(diff, repo_root)?;
        trace_references(&symbols, repo_root, cfg)
    }
}

pub fn extract_symbols(diff: &str, repo_root: &Path) -> Result<Vec<Symbol>> {
    let file_ranges = parse_diff_ranges(diff);
    let mut parser = Parser::new();
    let language = tree_sitter_rust::LANGUAGE;
    parser
        .set_language(&language.into())
        .context("set rust grammar")?;

    let mut symbols = Vec::new();
    for (file, ranges) in file_ranges {
        let full_path = repo_root.join(&file);
        if !full_path.exists() {
            continue;
        }
        let source = fs::read_to_string(&full_path)
            .with_context(|| format!("read source {}", full_path.display()))?;
        let tree = parser.parse(&source, None).context("parse source")?;
        let root = tree.root_node();
        let mut cursor = root.walk();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let kind = match node.kind() {
                "function_item" => Some(SymbolKind::Function),
                "struct_item" => Some(SymbolKind::Struct),
                "trait_item" => Some(SymbolKind::Trait),
                "impl_item" => Some(SymbolKind::Impl),
                "const_item" | "static_item" => Some(SymbolKind::Constant),
                _ => None,
            };

            if let Some(kind) = kind {
                if overlaps_any(node, &ranges) {
                    if let Some(name) = symbol_name(&node, &source) {
                        symbols.push(Symbol {
                            name,
                            kind,
                            file: file.clone(),
                        });
                    }
                }
            }

            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
    }

    Ok(symbols)
}

pub fn trace_references(
    symbols: &[Symbol],
    repo_root: &Path,
    cfg: &ContextConfig,
) -> Result<Vec<Reference>> {
    if symbols.is_empty() {
        return Ok(Vec::new());
    }

    let mut refs = Vec::new();
    let mut regexes = HashMap::new();
    for sym in symbols {
        let re = Regex::new(&format!(r"\b{}\b", regex::escape(&sym.name)))?;
        regexes.insert(sym.name.clone(), re);
    }

    let mut candidates = Vec::new();
    for entry in WalkDir::new(repo_root)
        .max_depth(cfg.reference_max_depth.max(1))
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        if let Some(ext) = path.extension() {
            if ext != "rs" {
                continue;
            }
        } else {
            continue;
        }

        candidates.push(path.strip_prefix(repo_root).unwrap_or(path).to_path_buf());
    }

    candidates.sort_by(|a, b| compare_reference_paths(a, b, symbols));
    candidates.truncate(cfg.reference_max_files);

    for relative_path in candidates {
        let path = repo_root.join(&relative_path);
        let contents = fs::read_to_string(&path).unwrap_or_default();
        for (idx, line) in contents.lines().enumerate() {
            for (name, re) in &regexes {
                if re.is_match(line) {
                    refs.push(Reference {
                        symbol: name.clone(),
                        file: relative_path.clone(),
                        line: (idx + 1) as u32,
                        snippet: line.trim().to_string(),
                    });
                }
            }
        }
    }

    Ok(refs)
}

#[derive(Debug, Clone)]
struct LineRange {
    start: u32,
    end: u32,
}

fn overlaps_any(node: Node, ranges: &[LineRange]) -> bool {
    let start = node.start_position().row as u32 + 1;
    let end = node.end_position().row as u32 + 1;
    ranges.iter().any(|r| start <= r.end && end >= r.start)
}

fn symbol_name(node: &Node, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("type"))
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .map(|s| s.to_string())
}

fn parse_diff_ranges(diff: &str) -> HashMap<PathBuf, Vec<LineRange>> {
    let mut map: HashMap<PathBuf, Vec<LineRange>> = HashMap::new();
    let mut current_file: Option<PathBuf> = None;

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            current_file = Some(PathBuf::from(rest.trim()));
        } else if line.starts_with("@@") {
            if let Some(file) = current_file.clone() {
                if let Some(range) = parse_hunk_range(line) {
                    map.entry(file).or_default().push(range);
                }
            }
        }
    }

    map
}

fn parse_hunk_range(line: &str) -> Option<LineRange> {
    let parts: Vec<&str> = line.split(' ').collect();
    let new_part = parts.iter().find(|p| p.starts_with('+'))?;
    let range = new_part.trim_start_matches('+');
    let mut iter = range.split(',');
    let start: u32 = iter.next()?.parse().ok()?;
    let len: u32 = iter.next().unwrap_or("1").parse().ok()?;
    let end = if len == 0 { start } else { start + len - 1 };
    Some(LineRange { start, end })
}

fn compare_reference_paths(a: &Path, b: &Path, symbols: &[Symbol]) -> Ordering {
    reference_rank(a, symbols)
        .cmp(&reference_rank(b, symbols))
        .then_with(|| a.cmp(b))
}

fn reference_rank(path: &Path, symbols: &[Symbol]) -> (u8, usize, usize, String) {
    let changed_files = symbols.iter().map(|s| s.file.as_path()).collect::<HashSet<_>>();
    let same_file = changed_files.contains(path);
    let best_distance = symbols
        .iter()
        .map(|symbol| directory_distance(path, &symbol.file))
        .min()
        .unwrap_or(usize::MAX);
    let name_overlap = symbols
        .iter()
        .filter(|symbol| path.to_string_lossy().contains(symbol.name.as_str()))
        .count();

    (
        if same_file { 0 } else { 1 },
        best_distance,
        usize::MAX - name_overlap,
        path.to_string_lossy().into_owned(),
    )
}

fn directory_distance(a: &Path, b: &Path) -> usize {
    let a_components = a.parent().into_iter().flat_map(|p| p.components()).count();
    let b_components = b.parent().into_iter().flat_map(|p| p.components()).count();
    let common = a
        .parent()
        .into_iter()
        .flat_map(|p| p.components())
        .zip(b.parent().into_iter().flat_map(|p| p.components()))
        .take_while(|(left, right)| left == right)
        .count();
    (a_components - common) + (b_components - common)
}

#[cfg(test)]
mod tests {
    use super::{
        Symbol, SymbolKind, compare_reference_paths, directory_distance, extract_symbols,
        parse_diff_ranges, parse_hunk_range, reference_rank,
    };
    use anyhow::Result;
    use std::cmp::Ordering;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[test]
    fn directory_distance_prefers_nearby_files() {
        assert_eq!(directory_distance(Path::new("src/a.rs"), Path::new("src/b.rs")), 0);
        assert_eq!(
            directory_distance(Path::new("src/a.rs"), Path::new("src/nested/b.rs")),
            1
        );
        assert_eq!(
            directory_distance(Path::new("src/a.rs"), Path::new("tests/b.rs")),
            2
        );
    }

    #[test]
    fn reference_sort_prefers_changed_file_and_nearby_paths() {
        let symbols = vec![Symbol {
            name: "widget".to_string(),
            kind: SymbolKind::Struct,
            file: PathBuf::from("src/widget.rs"),
        }];

        assert_eq!(
            compare_reference_paths(
                Path::new("src/widget.rs"),
                Path::new("tests/widget.rs"),
                &symbols
            ),
            Ordering::Less
        );
        assert_eq!(
            compare_reference_paths(
                Path::new("src/other.rs"),
                Path::new("tests/widget.rs"),
                &symbols
            ),
            Ordering::Less
        );
    }

    #[test]
    fn parse_hunk_range_handles_multi_line_and_zero_length_hunks() {
        let multi = parse_hunk_range("@@ -3,1 +10,4 @@").expect("multi-line hunk");
        assert_eq!(multi.start, 10);
        assert_eq!(multi.end, 13);

        let zero = parse_hunk_range("@@ -3,1 +8,0 @@").expect("zero-length hunk");
        assert_eq!(zero.start, 8);
        assert_eq!(zero.end, 8);
    }

    #[test]
    fn parse_diff_ranges_groups_hunks_by_file() {
        let diff = concat!(
            "diff --git a/src/lib.rs b/src/lib.rs\n",
            "--- a/src/lib.rs\n",
            "+++ b/src/lib.rs\n",
            "@@ -1,1 +5,2 @@\n",
            "@@ -10,2 +20,1 @@\n",
            "diff --git a/src/other.rs b/src/other.rs\n",
            "--- a/src/other.rs\n",
            "+++ b/src/other.rs\n",
            "@@ -1,1 +2,3 @@\n",
        );

        let ranges = parse_diff_ranges(diff);
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[Path::new("src/lib.rs")].len(), 2);
        assert_eq!(ranges[Path::new("src/lib.rs")][0].start, 5);
        assert_eq!(ranges[Path::new("src/lib.rs")][0].end, 6);
        assert_eq!(ranges[Path::new("src/lib.rs")][1].start, 20);
        assert_eq!(ranges[Path::new("src/lib.rs")][1].end, 20);
        assert_eq!(ranges[Path::new("src/other.rs")][0].start, 2);
        assert_eq!(ranges[Path::new("src/other.rs")][0].end, 4);
    }

    #[test]
    fn reference_rank_prefers_same_file_then_name_overlap() {
        let symbols = vec![Symbol {
            name: "widget".to_string(),
            kind: SymbolKind::Struct,
            file: PathBuf::from("src/widget.rs"),
        }];

        assert_eq!(
            reference_rank(Path::new("src/widget.rs"), &symbols),
            (0, 0, usize::MAX - 1, "src/widget.rs".to_string())
        );
        assert_eq!(
            reference_rank(Path::new("src/other.rs"), &symbols),
            (1, 0, usize::MAX, "src/other.rs".to_string())
        );
    }

    #[test]
    fn extract_symbols_finds_all_supported_rust_item_kinds() -> Result<()> {
        let dir = tempdir()?;
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir)?;
        fs::write(
            src_dir.join("lib.rs"),
            concat!(
                "fn top() {}\n",
                "struct Widget;\n",
                "trait Service {}\n",
                "impl Widget { fn new() -> Self { Widget } }\n",
                "const FLAG: bool = true;\n",
            ),
        )?;

        let diff = concat!(
            "diff --git a/src/lib.rs b/src/lib.rs\n",
            "--- a/src/lib.rs\n",
            "+++ b/src/lib.rs\n",
            "@@ -0,0 +1,5 @@\n",
            "+fn top() {}\n",
            "+struct Widget;\n",
            "+trait Service {}\n",
            "+impl Widget { fn new() -> Self { Widget } }\n",
            "+const FLAG: bool = true;\n",
        );

        let symbols = extract_symbols(diff, dir.path())?;
        assert!(symbols.iter().any(|s| matches!(s.kind, SymbolKind::Function) && s.name == "top"));
        assert!(
            symbols
                .iter()
                .any(|s| matches!(s.kind, SymbolKind::Struct) && s.name == "Widget")
        );
        assert!(
            symbols
                .iter()
                .any(|s| matches!(s.kind, SymbolKind::Trait) && s.name == "Service")
        );
        assert!(
            symbols
                .iter()
                .any(|s| matches!(s.kind, SymbolKind::Impl) && s.name == "Widget")
        );
        assert!(
            symbols
                .iter()
                .any(|s| matches!(s.kind, SymbolKind::Constant) && s.name == "FLAG")
        );
        Ok(())
    }
}
