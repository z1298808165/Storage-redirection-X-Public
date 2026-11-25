use super::{lsplant_cmake_stamp, prepare_lsplant_build_dir, write_cxx_modules_manifest};
use std::fs;
use std::path::Path;

#[test]
fn cmake_version_invalidates_the_lsplant_build_cache() {
    let stamp = |version| {
        lsplant_cmake_stamp(
            Path::new("ndk/toolchain.cmake"),
            "arm64-v8a",
            Path::new("libcxx"),
            version,
        )
    };
    let parent = std::env::temp_dir().join(format!("srx-cmake-cache-{}", std::process::id()));
    fs::create_dir(&parent).unwrap();
    let dir = parent.join("cmake");
    let cache = dir.join("CMakeCache.txt");
    let old = stamp("cmake version 4.4.5");
    let new = stamp("cmake version 4.5.1");
    let marker = prepare_lsplant_build_dir(&dir, &old).unwrap();
    fs::write(&cache, "old").unwrap();
    prepare_lsplant_build_dir(&dir, &old).unwrap();
    assert!(!cache.exists());
    fs::write(&marker, &old).unwrap();
    fs::write(&cache, "old").unwrap();
    prepare_lsplant_build_dir(&dir, &old).unwrap();
    assert_eq!(fs::read_to_string(&cache).unwrap(), "old");
    prepare_lsplant_build_dir(&dir, &new).unwrap();
    assert!(dir.is_dir() && !cache.exists() && !marker.exists());
    fs::remove_dir(&dir).unwrap();
    fs::write(&dir, "unrelated").unwrap();
    assert!(prepare_lsplant_build_dir(&dir, &new).is_err());
    assert_eq!(fs::read_to_string(&dir).unwrap(), "unrelated");
    fs::remove_dir_all(parent).unwrap();
}

#[test]
fn cxx_modules_manifest_points_to_the_aar_sources() {
    let output = std::env::temp_dir().join(format!("srx-cmake-modules-{}", std::process::id()));
    let prefix = output.join("aar");
    let include = prefix.join("prefab/modules/cxx/include");
    let sources = include.join("std-modules");
    let manifest = output.join("modules.json");
    fs::create_dir_all(&sources).unwrap();
    fs::write(sources.join("std.cppm"), "").unwrap();
    assert!(write_cxx_modules_manifest(&prefix, &manifest).is_err());
    assert!(!manifest.exists());
    fs::write(sources.join("std.compat.cppm"), "").unwrap();
    write_cxx_modules_manifest(&prefix, &manifest).unwrap();
    let content = fs::read_to_string(&manifest).unwrap();
    let escaped = super::json_escape(&include.display().to_string());
    assert_eq!(content.matches(&escaped).count(), 4);
    for module in ["std.cppm", "std.compat.cppm"] {
        assert!(content.contains(&super::json_escape(
            &sources.join(module).display().to_string()
        )));
    }
    assert!(!prefix.join("lib").exists());
    fs::remove_dir_all(output).unwrap();
}
