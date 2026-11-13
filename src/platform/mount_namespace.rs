/// 固定目标 namespace 的 FD，并拒绝与其它进程共享的视图。
pub fn private_mount_namespace(pid: i32) -> Option<std::os::fd::OwnedFd> {
    use std::os::unix::fs::MetadataExt;
    let namespace = std::fs::File::open(format!("/proc/{pid}/ns/mnt")).ok()?;
    let identity = namespace.metadata().ok()?;
    for entry in std::fs::read_dir("/proc").ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let Some(other) = name.to_str().and_then(|name| name.parse::<i32>().ok()) else {
            continue;
        };
        if other == pid {
            continue;
        }
        match std::fs::metadata(entry.path().join("ns/mnt")) {
            Ok(other) if other.dev() == identity.dev() && other.ino() == identity.ino() => {
                return None;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    Some(namespace.into())
}
