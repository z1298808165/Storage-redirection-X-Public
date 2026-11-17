use super::mount_state_temp_path;
use std::collections::HashSet;

#[test]
fn concurrent_mount_writers_get_distinct_temp_paths() {
    let state_path = "/tmp/srx.mount.state";
    let paths: HashSet<_> = (0..1024)
        .map(|_| mount_state_temp_path(state_path))
        .collect();

    assert_eq!(paths.len(), 1024);
    assert!(paths.iter().all(|path| path.starts_with(state_path)));
}
