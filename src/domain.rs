use crate::platform::paths;
use std::collections::{HashMap, HashSet};

pub const MAX_PATH_MAPPING_DEPTH: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathMapping {
    pub request_path: String,
    pub final_path: String,
}

impl PathMapping {
    pub fn new(request_path: String, final_path: String) -> Self {
        Self {
            request_path,
            final_path,
        }
    }
}

pub fn sort_path_mappings_longest_request_first(mappings: &mut [PathMapping]) {
    mappings.sort_by(|a, b| {
        if a.request_path.len() != b.request_path.len() {
            b.request_path.len().cmp(&a.request_path.len())
        } else {
            a.request_path.cmp(&b.request_path)
        }
    });
}

pub fn sort_path_mappings_longest_request_first_case_insensitive(mappings: &mut [PathMapping]) {
    mappings.sort_by(|a, b| {
        if a.request_path.len() != b.request_path.len() {
            b.request_path.len().cmp(&a.request_path.len())
        } else {
            paths::match_key(&a.request_path)
                .cmp(&paths::match_key(&b.request_path))
                .then_with(|| a.request_path.cmp(&b.request_path))
        }
    });
}

pub fn sort_path_mappings_shortest_request_first(mappings: &mut [PathMapping]) {
    mappings.sort_by(|a, b| {
        if a.request_path.len() != b.request_path.len() {
            a.request_path.len().cmp(&b.request_path.len())
        } else {
            a.request_path.cmp(&b.request_path)
        }
    });
}

/// 按最长请求前缀逐层应用映射，支持“父映射结果继续命中子映射”的规则链。
pub fn map_path_by_mappings(path: &str, mappings: &[PathMapping]) -> String {
    map_mapping_chain(path, mappings, false)
}

/// 按最长目标前缀反向还原映射链，用于 MediaStore 展示路径恢复。
pub fn reverse_map_path_by_mappings(path: &str, mappings: &[PathMapping]) -> String {
    map_mapping_chain(path, mappings, true)
}

fn map_mapping_chain(path: &str, mappings: &[PathMapping], reverse: bool) -> String {
    if path.is_empty() || mappings.is_empty() {
        return String::new();
    }

    let mut current = path.to_string();
    let mut changed = false;
    let mut visited = HashSet::new();
    visited.insert(paths::match_key(&current));

    for _ in 0..=MAX_PATH_MAPPING_DEPTH {
        let best = mappings
            .iter()
            .filter_map(|mapping| {
                let root = if reverse {
                    &mapping.final_path
                } else {
                    &mapping.request_path
                };
                paths::child_suffix(&current, root).map(|suffix| (mapping, suffix))
            })
            .max_by(|(left, _), (right, _)| {
                let left_root = if reverse {
                    &left.final_path
                } else {
                    &left.request_path
                };
                let right_root = if reverse {
                    &right.final_path
                } else {
                    &right.request_path
                };
                left_root
                    .len()
                    .cmp(&right_root.len())
                    .then_with(|| paths::match_key(left_root).cmp(&paths::match_key(right_root)))
            });

        let Some((mapping, suffix)) = best else {
            return if changed { current } else { String::new() };
        };
        let target = if reverse {
            &mapping.request_path
        } else {
            &mapping.final_path
        };
        let next = if suffix.is_empty() {
            target.clone()
        } else {
            format!("{}{}", target, suffix)
        };
        if !visited.insert(paths::match_key(&next)) {
            return String::new();
        }
        current = next;
        changed = true;
    }

    String::new()
}

pub fn dedup_path_mappings_by_request_case_insensitive(mappings: &mut Vec<PathMapping>) {
    mappings.dedup_by(|a, b| paths::eq_ignore_case(&a.request_path, &b.request_path));
}

pub fn filter_valid_path_mapping_chains(mappings: Vec<PathMapping>) -> Vec<PathMapping> {
    if mappings.is_empty() {
        return mappings;
    }

    let target_by_request: HashMap<String, String> = mappings
        .iter()
        .map(|mapping| {
            (
                paths::match_key(&mapping.request_path),
                paths::match_key(&mapping.final_path),
            )
        })
        .collect();
    let cyclic_sources = detect_mapping_cycles(&target_by_request);
    let over_depth_sources: HashSet<String> = detect_mapping_depths(&target_by_request)
        .into_iter()
        .filter_map(|(source, depth)| (depth > MAX_PATH_MAPPING_DEPTH).then_some(source))
        .collect();

    if cyclic_sources.is_empty() && over_depth_sources.is_empty() {
        return mappings;
    }

    mappings
        .into_iter()
        .filter(|mapping| {
            let source_key = paths::match_key(&mapping.request_path);
            !cyclic_sources.contains(&source_key) && !over_depth_sources.contains(&source_key)
        })
        .collect()
}

fn detect_mapping_cycles(mappings: &HashMap<String, String>) -> HashSet<String> {
    let mut cycles = HashSet::new();
    let mut visit_state: HashMap<String, u8> = HashMap::new();
    let mut stack: Vec<String> = Vec::new();

    for source in mappings.keys() {
        visit_mapping_cycle(source, mappings, &mut visit_state, &mut stack, &mut cycles);
    }

    cycles
}

fn visit_mapping_cycle(
    source: &str,
    mappings: &HashMap<String, String>,
    visit_state: &mut HashMap<String, u8>,
    stack: &mut Vec<String>,
    cycles: &mut HashSet<String>,
) {
    match visit_state.get(source).copied() {
        Some(1) => {
            if let Some(index) = stack.iter().position(|path| path == source) {
                cycles.extend(stack[index..].iter().cloned());
            }
            return;
        }
        Some(2) => return,
        _ => {}
    }

    visit_state.insert(source.to_string(), 1);
    stack.push(source.to_string());
    if let Some(target) = mappings.get(source) {
        visit_mapping_cycle(target, mappings, visit_state, stack, cycles);
    }
    stack.pop();
    visit_state.insert(source.to_string(), 2);
}

fn detect_mapping_depths(mappings: &HashMap<String, String>) -> HashMap<String, usize> {
    let mut depths = HashMap::new();
    for source in mappings.keys() {
        if !depths.contains_key(source) {
            compute_mapping_depth(source, mappings, &mut depths, &mut HashSet::new());
        }
    }
    depths
}

fn compute_mapping_depth(
    source: &str,
    mappings: &HashMap<String, String>,
    depths: &mut HashMap<String, usize>,
    visiting: &mut HashSet<String>,
) -> usize {
    if visiting.contains(source) {
        return MAX_PATH_MAPPING_DEPTH + 1;
    }
    if let Some(depth) = depths.get(source) {
        return *depth;
    }

    let Some(target) = mappings.get(source) else {
        return 0;
    };
    visiting.insert(source.to_string());
    let depth = 1 + compute_mapping_depth(target, mappings, depths, visiting);
    visiting.remove(source);
    depths.insert(source.to_string(), depth);
    depth
}
