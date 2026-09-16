use native_config_fixtures::domain::{
    PathMapping, expand_namespace_path_mappings, map_path_by_mappings,
};

fn mapping(request: &str, target: &str) -> PathMapping {
    PathMapping::new(request.into(), target.into())
}

#[test]
fn nested_mounts_preserve_direct_and_all_parent_entries() {
    let rules = vec![
        mapping("/a", "/b"),
        mapping("/other", "/b"),
        mapping("/b/child", "/c"),
    ];
    let result = expand_namespace_path_mappings(&rules);
    for request in ["/a/child", "/other/child", "/b/child"] {
        assert!(result.contains(&mapping(request, "/c")));
    }
    assert!(result.contains(&mapping("/a", "/b")));
}

#[test]
fn nested_mounts_follow_multiple_parents_and_longest_prefix() {
    let rules = vec![
        mapping("/a", "/b"),
        mapping("/b/x", "/c"),
        mapping("/c/y", "/d"),
        mapping("/a/x/override", "/explicit"),
    ];
    let result = expand_namespace_path_mappings(&rules);
    assert!(result.contains(&mapping("/a/x/y", "/d")));
    assert!(result.contains(&mapping("/a/x/override", "/explicit")));
    for rule in result {
        assert_eq!(
            rule.final_path,
            map_path_by_mappings(&rule.request_path, &rules)
        );
    }
}

#[test]
fn exact_chains_use_terminal_targets_and_cycles_are_dropped() {
    let rules = vec![mapping("/a", "/b"), mapping("/b", "/c")];
    let result = expand_namespace_path_mappings(&rules);
    assert!(result.contains(&mapping("/a", "/c")));
    assert!(result.contains(&mapping("/b", "/c")));
    assert!(expand_namespace_path_mappings(&[mapping("/a", "/b"), mapping("/b", "/a")]).is_empty());
}
