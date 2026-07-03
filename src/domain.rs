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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_mapping_chains_drops_cycles() {
        let mappings = filter_valid_path_mapping_chains(vec![
            PathMapping::new("A".to_string(), "B".to_string()),
            PathMapping::new("B".to_string(), "C".to_string()),
            PathMapping::new("C".to_string(), "A".to_string()),
            PathMapping::new("Keep".to_string(), "Target".to_string()),
        ]);

        assert_eq!(
            mappings,
            vec![PathMapping::new("Keep".to_string(), "Target".to_string())]
        );
    }

    #[test]
    fn filter_mapping_chains_drops_case_variant_cycles() {
        let mappings = filter_valid_path_mapping_chains(vec![
            PathMapping::new("/storage/emulated/0/Download".to_string(), "B".to_string()),
            PathMapping::new("b".to_string(), "/storage/emulated/0/download".to_string()),
            PathMapping::new("Keep".to_string(), "Target".to_string()),
        ]);

        assert_eq!(
            mappings,
            vec![PathMapping::new("Keep".to_string(), "Target".to_string())]
        );
    }

    #[test]
    fn filter_mapping_chains_drops_overly_deep_sources() {
        let mappings = filter_valid_path_mapping_chains(vec![
            PathMapping::new("A".to_string(), "B".to_string()),
            PathMapping::new("B".to_string(), "C".to_string()),
            PathMapping::new("C".to_string(), "D".to_string()),
            PathMapping::new("D".to_string(), "E".to_string()),
            PathMapping::new("E".to_string(), "F".to_string()),
            PathMapping::new("F".to_string(), "G".to_string()),
            PathMapping::new("G".to_string(), "H".to_string()),
            PathMapping::new("H".to_string(), "I".to_string()),
            PathMapping::new("I".to_string(), "J".to_string()),
            PathMapping::new("J".to_string(), "K".to_string()),
            PathMapping::new("K".to_string(), "L".to_string()),
        ]);

        assert_eq!(
            mappings
                .first()
                .map(|mapping| mapping.request_path.as_str()),
            Some("B")
        );
        assert_eq!(mappings.len(), 10);
    }

    #[test]
    fn sort_mapping_helpers_keep_runtime_and_mount_ordering_distinct() {
        let mut longest_first = vec![
            PathMapping::new("/storage/emulated/0/DCIM".to_string(), "A".to_string()),
            PathMapping::new(
                "/storage/emulated/0/DCIM/Nested".to_string(),
                "B".to_string(),
            ),
            PathMapping::new("/storage/emulated/0/Download".to_string(), "C".to_string()),
        ];
        sort_path_mappings_longest_request_first(&mut longest_first);
        assert_eq!(
            longest_first
                .iter()
                .map(|mapping| mapping.request_path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "/storage/emulated/0/DCIM/Nested",
                "/storage/emulated/0/Download",
                "/storage/emulated/0/DCIM",
            ]
        );

        let mut shortest_first = longest_first.clone();
        sort_path_mappings_shortest_request_first(&mut shortest_first);
        assert_eq!(
            shortest_first
                .iter()
                .map(|mapping| mapping.request_path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "/storage/emulated/0/DCIM",
                "/storage/emulated/0/Download",
                "/storage/emulated/0/DCIM/Nested",
            ]
        );
    }

    #[test]
    fn case_insensitive_mapping_helpers_sort_and_keep_first_request_variant() {
        let mut mappings = vec![
            PathMapping::new(
                "/storage/emulated/0/download/App".to_string(),
                "A".to_string(),
            ),
            PathMapping::new(
                "/storage/emulated/0/Download/app".to_string(),
                "B".to_string(),
            ),
            PathMapping::new("/storage/emulated/0/DCIM".to_string(), "C".to_string()),
        ];

        sort_path_mappings_longest_request_first_case_insensitive(&mut mappings);
        dedup_path_mappings_by_request_case_insensitive(&mut mappings);

        assert_eq!(mappings.len(), 2);
        assert_eq!(mappings[0].request_path, "/storage/emulated/0/Download/app");
        assert_eq!(mappings[0].final_path, "B");
        assert_eq!(mappings[1].request_path, "/storage/emulated/0/DCIM");
    }
}
