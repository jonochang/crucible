use crate::config::ContextConfig;
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

    let mut files_scanned = 0usize;
    for entry in WalkDir::new(repo_root).into_iter().filter_map(Result::ok) {
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

        files_scanned += 1;
        if files_scanned > cfg.reference_max_files {
            break;
        }

        let contents = fs::read_to_string(path).unwrap_or_default();
        for (idx, line) in contents.lines().enumerate() {
            for (name, re) in &regexes {
                if re.is_match(line) {
                    refs.push(Reference {
                        symbol: name.clone(),
                        file: path.strip_prefix(repo_root).unwrap_or(path).to_path_buf(),
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
