use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const MIN_HOOKER_DEX_BYTES: u64 = 1024;

// LSPlant 上游 master（std modules 迁移后）依赖 org.lsposed.libcxx AAR 提供
// cxx CMake 包与 std 模块源；构件版本必须与 NDK 版本一致（std 模块 BMI 与
// 消费方 clang 同版本编译），升级任一方时需同步更新以下三个常量与 SHA256。
// 离线或预置环境可设置 SRX_LIBCXX_PREFIX 直接指向已解包的 AAR 根目录跳过下载。
const LIBCXX_AAR_VERSION: &str = "30.0.16248370";
const LIBCXX_AAR_SHA256_HEX: &str =
    "8bb6839964cdd5b814255c2674a9bb4a7ba5a6b2e15284c307694a571e8d5f7c";
const LIBCXX_AAR_URL: &str = "https://repo1.maven.org/maven2/org/lsposed/libcxx/libcxx/30.0.16248370/libcxx-30.0.16248370.aar";

// 执行构建配置
fn main() {
    println!("cargo:rustc-check-cfg=cfg(srx_no_path_metadata_repair)");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    build_hooker_dex(&target_os);

    if target_os != "android" {
        return;
    }

    println!("cargo:rustc-link-lib=log");
    println!("cargo:rustc-link-lib=android");

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    build_lsplant_bridge(&target_arch);

    let mut link_args = vec![
        "-Wl,--gc-sections".to_string(),
        "-Wl,--exclude-libs,ALL".to_string(),
        "-Wl,--pack-dyn-relocs=none".to_string(),
        "-Wl,-soname,libsrx_core.so".to_string(),
    ];

    link_args.push("-Wl,-s".to_string());

    // 16KB page 仅 arm64 真机需要；x86_64 模拟器 4KB page 下 zygisksu loader 会漏 mmap RW segment
    if target_arch == "aarch64" {
        link_args.push("-Wl,-z,max-page-size=16384".to_string());
    }

    for arg in &link_args {
        println!("cargo:rustc-link-arg={}", arg);
    }
}

fn build_lsplant_bridge(target_arch: &str) {
    println!("cargo:rerun-if-changed=native/CMakeLists.txt");
    println!("cargo:rerun-if-changed=native/srx_lsplant_bridge.cpp");
    println!("cargo:rerun-if-changed=vendor/lsplant/CMakeLists.txt");
    println!("cargo:rerun-if-changed=vendor/lsplant/external/dex_builder/CMakeLists.txt");
    println!("cargo:rerun-if-env-changed=ANDROID_NDK_HOME");
    println!("cargo:rerun-if-env-changed=NDK_ROOT");
    println!("cargo:rerun-if-env-changed=ANDROID_HOME");
    println!("cargo:rerun-if-env-changed=ANDROID_SDK_ROOT");
    println!("cargo:rerun-if-env-changed=SRX_LIBCXX_PREFIX");

    let Some(ndk) = locate_ndk() else {
        if env::var("CARGO_CFG_CLIPPY").is_ok()
            || env::var("PROFILE").unwrap_or_default() == "debug"
        {
            println!("cargo:warning=srx_core: LSPlant build skipped: Android NDK not found");
            return;
        }
        panic!("Android NDK not found for LSPlant build");
    };
    let Some(abi) = cmake_android_abi(target_arch) else {
        panic!("unsupported Android arch for LSPlant: {target_arch}");
    };
    let target_triple = android_target_triple(target_arch);

    let libcxx_prefix = match ensure_libcxx_prefix() {
        Ok(prefix) => prefix,
        Err(err) => {
            if env::var("CARGO_CFG_CLIPPY").is_ok()
                || env::var("PROFILE").unwrap_or_default() == "debug"
            {
                println!(
                    "cargo:warning=srx_core: LSPlant build skipped: libcxx AAR provisioning failed: {err}"
                );
                return;
            }
            panic!("libcxx AAR provisioning failed: {err}");
        }
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let build_dir = out_dir.join("lsplant_cmake").join(abi);
    let install_dir = out_dir.join("lsplant_install").join(abi);
    let profile = "Release";
    let inline_hook_include = PathBuf::from(
        env::var_os("DEP_SRX_INLINE_HOOK_INCLUDE").expect("DEP_SRX_INLINE_HOOK_INCLUDE"),
    );

    let toolchain_file = ndk.join("build/cmake/android.toolchain.cmake");
    // CMakeCache.txt 会粘住 toolchain 与编译器路径，-D 覆盖不了缓存里的旧值：
    // cargo build 缓存跨 NDK 版本/构件前缀复用时（CI 的 actions/cache 或本地换 NDK），
    // 旧缓存会把 configure 带向已不存在的旧 NDK 路径。用参数指纹戳识别并整目录重建。
    let config_stamp = lsplant_cmake_stamp(&toolchain_file, abi, &libcxx_prefix);
    let stamp_path = build_dir.join(".srx-cmake-config-stamp");
    if build_dir.exists() {
        let stale = std::fs::read_to_string(&stamp_path)
            .map(|content| content != config_stamp)
            .unwrap_or(true);
        if stale {
            let _ = std::fs::remove_dir_all(&build_dir);
        }
    }
    std::fs::create_dir_all(&build_dir)
        .unwrap_or_else(|err| panic!("create {} failed: {err}", build_dir.display()));

    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg("native")
        .arg("-B")
        .arg(&build_dir)
        .arg("-G")
        .arg("Ninja")
        .arg(format!(
            "-DCMAKE_TOOLCHAIN_FILE={}",
            toolchain_file.display()
        ))
        .arg(format!("-DANDROID_ABI={abi}"))
        .arg("-DANDROID_PLATFORM=android-29")
        .arg("-DANDROID_STL=c++_static")
        .arg(format!("-DCMAKE_BUILD_TYPE={profile}"))
        .arg(format!("-DCMAKE_INSTALL_PREFIX={}", install_dir.display()))
        .arg(format!(
            "-DSRX_INLINE_HOOK_INCLUDE_DIR={}",
            inline_hook_include.display()
        ))
        .arg("-DLSPLANT_BUILD_SHARED=OFF")
        .arg("-DDEX_BUILDER_BUILD_SHARED=OFF")
        .arg("-DANDROID_SUPPORT_FLEXIBLE_PAGE_SIZES=ON")
        .arg(format!("-DCMAKE_PREFIX_PATH={}", libcxx_prefix.display()))
        // NDK 工具链默认 CMAKE_FIND_ROOT_PATH_MODE_PACKAGE=ONLY，会把 find_package
        // 限制在 sysroot 内；cxx 包在宿主侧，按 NDK 工具链注释建议显式放开为 BOTH。
        .arg("-DCMAKE_FIND_ROOT_PATH_MODE_PACKAGE=BOTH");
    run_command(&mut configure, "configure LSPlant");
    // 只在 configure 完整成功后落戳，失败遗留的半配置目录下次会被识别为过期重建。
    std::fs::write(&stamp_path, &config_stamp)
        .unwrap_or_else(|err| panic!("write {} failed: {err}", stamp_path.display()));

    let mut build = Command::new("cmake");
    build
        .arg("--build")
        .arg(&build_dir)
        .arg("--target")
        .arg("srx_lsplant_bridge");
    run_command(&mut build, "build LSPlant");

    println!("cargo:rustc-link-search=native={}", build_dir.display());
    println!(
        "cargo:rustc-link-search=native={}",
        build_dir.join("lsplant").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        build_dir.join("lsplant/external/dex_builder").display()
    );
    let cxx_lib_dir = ndk
        .join("toolchains/llvm/prebuilt")
        .join(host_tag())
        .join("sysroot/usr/lib")
        .join(target_triple);
    println!("cargo:rustc-link-lib=static=srx_lsplant_bridge");
    println!("cargo:rustc-link-lib=static=lsplant_static");
    println!("cargo:rustc-link-lib=static=dex_builder_static");
    println!(
        "cargo:rustc-link-arg={}",
        cxx_lib_dir.join("libc++_static.a").display()
    );
    println!(
        "cargo:rustc-link-arg={}",
        cxx_lib_dir.join("libc++abi.a").display()
    );
    println!("cargo:rustc-link-lib=z");
}

fn android_target_triple(target_arch: &str) -> &'static str {
    match target_arch {
        "aarch64" => "aarch64-linux-android",
        "x86_64" => "x86_64-linux-android",
        "arm" => "arm-linux-androideabi",
        "x86" => "i686-linux-android",
        _ => "aarch64-linux-android",
    }
}

fn host_tag() -> &'static str {
    if cfg!(windows) {
        "windows-x86_64"
    } else if cfg!(target_os = "macos") {
        "darwin-x86_64"
    } else {
        "linux-x86_64"
    }
}

fn cmake_android_abi(target_arch: &str) -> Option<&'static str> {
    match target_arch {
        "aarch64" => Some("arm64-v8a"),
        "x86_64" => Some("x86_64"),
        _ => None,
    }
}

fn locate_ndk() -> Option<PathBuf> {
    if let Some(path) = env::var_os("ANDROID_NDK_HOME") {
        let ndk = PathBuf::from(path);
        if ndk.exists() {
            return Some(ndk);
        }
    }
    if let Some(path) = env::var_os("NDK_ROOT") {
        let ndk = PathBuf::from(path);
        if ndk.exists() {
            return Some(ndk);
        }
    }
    let sdk = env::var_os("ANDROID_HOME").or_else(|| env::var_os("ANDROID_SDK_ROOT"))?;
    let ndk_dir = PathBuf::from(sdk).join("ndk");
    let mut versions = std::fs::read_dir(ndk_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect::<Vec<_>>();
    versions.sort_by(|a, b| b.cmp(a));
    versions.into_iter().next()
}

// LSPlant 的 cxx 包来自 org.lsposed.libcxx AAR：下载到用户缓存并解包，
// 再补一个 AGP prefab 本会生成、直接 CMake 消费所需的 cxxConfig.cmake 垫片。
fn ensure_libcxx_prefix() -> Result<PathBuf, String> {
    if let Some(prefix) = env::var_os("SRX_LIBCXX_PREFIX") {
        let prefix = PathBuf::from(prefix);
        if !is_libcxx_extracted(&prefix) {
            return Err(format!(
                "SRX_LIBCXX_PREFIX 指向的目录缺少 AAR 内容（prefab/modules/cxx/include）: {}",
                prefix.display()
            ));
        }
        ensure_cxx_config_shim(&prefix)?;
        return Ok(prefix);
    }
    let Some(home) = home_dir() else {
        return Err("无法定位用户主目录以缓存 libcxx AAR，请设置 SRX_LIBCXX_PREFIX".to_string());
    };
    let root = libcxx_cache_root_from_home(&home);
    let aar_path = root.join(format!("libcxx-{LIBCXX_AAR_VERSION}.aar"));
    let extracted = root.join("extracted");
    if is_libcxx_extracted(&extracted) {
        ensure_cxx_config_shim(&extracted)?;
        return Ok(extracted);
    }
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("创建缓存目录 {} 失败: {e}", root.display()))?;
    // 两处哈希比较统一忽略大小写：sha256 十六进制大小写等价，避免缓存文件
    // 因校验工具输出大小写不同而被误判失配、多下载一次。
    if !aar_path.exists()
        || !verify_file_sha256(&aar_path)?.eq_ignore_ascii_case(LIBCXX_AAR_SHA256_HEX)
    {
        download_libcxx_aar(&aar_path)?;
        let actual = verify_file_sha256(&aar_path)?;
        if !actual.eq_ignore_ascii_case(LIBCXX_AAR_SHA256_HEX) {
            return Err(format!(
                "libcxx AAR sha256 不符：expected {LIBCXX_AAR_SHA256_HEX}, got {actual}"
            ));
        }
    }
    let staging = root.join("extracted.staging");
    let _ = std::fs::remove_dir_all(&staging);
    // AAR 内容经 sha256 钉死校验后可信，解包走系统工具即可，无需逐条目消毒。
    extract_libcxx_aar(&aar_path, &staging)?;
    ensure_cxx_config_shim(&staging)?;
    let _ = std::fs::remove_dir_all(&extracted);
    std::fs::rename(&staging, &extracted)
        .map_err(|e| format!("落位 {} 失败: {e}", extracted.display()))?;
    Ok(extracted)
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

fn libcxx_cache_root_from_home(home: &Path) -> PathBuf {
    home.join(".cache")
        .join("srx")
        .join(format!("libcxx-{LIBCXX_AAR_VERSION}"))
}

fn is_libcxx_extracted(prefix: &Path) -> bool {
    prefix
        .join("prefab/modules/cxx/include/prefab/cmake-config.cmake")
        .is_file()
}

fn download_libcxx_aar(dest: &Path) -> Result<(), String> {
    let partial = dest.with_extension("part");
    let _ = std::fs::remove_file(&partial);
    let status = Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "--location",
            "--fail",
            "--retry",
            "3",
            "--connect-timeout",
            "30",
            "--silent",
            "--show-error",
        ])
        .arg("--output")
        .arg(&partial)
        .arg(LIBCXX_AAR_URL)
        .status()
        .map_err(|e| {
            format!(
                "启动 curl 失败（离线环境请手动下载 {LIBCXX_AAR_URL} 后设置 SRX_LIBCXX_PREFIX）: {e}"
            )
        })?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!(
            "curl 下载失败（exit {status}；离线环境请手动下载 {LIBCXX_AAR_URL} 后设置 SRX_LIBCXX_PREFIX）"
        ));
    }
    std::fs::rename(&partial, dest).map_err(|e| format!("落位 {} 失败: {e}", dest.display()))
}

// LSPlant cmake 构建目录的参数指纹：toolchain、ABI、cxx 包前缀任一变化都视为缓存过期。
fn lsplant_cmake_stamp(toolchain: &Path, abi: &str, libcxx_prefix: &Path) -> String {
    format!(
        "toolchain={}\nabi={}\nlibcxx_prefix={}\n",
        toolchain.display(),
        abi,
        libcxx_prefix.display()
    )
}

fn verify_file_sha256(path: &Path) -> Result<String, String> {
    let path_text = path.display().to_string();
    let candidates: Vec<Vec<String>> = if cfg!(windows) {
        vec![vec![
            "certutil".to_string(),
            "-hashfile".to_string(),
            path_text,
            "SHA256".to_string(),
        ]]
    } else {
        vec![
            vec!["sha256sum".to_string(), path_text.clone()],
            vec![
                "shasum".to_string(),
                "-a".to_string(),
                "256".to_string(),
                path_text,
            ],
        ]
    };
    for mut command in candidates {
        let program = command.remove(0);
        if let Ok(output) = Command::new(&program).args(&command).output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(hex) = extract_sha256_hex(&stdout) {
                return Ok(hex);
            }
        }
    }
    Err("无法计算 sha256（certutil/sha256sum/shasum 均不可用）".to_string())
}

// certutil 把哈希单独放一行；sha256sum/shasum 是 "<hex>  <path>"。
// 两者都满足「取输出里第一个恰为 64 位十六进制的词」，避免逐工具解析。
fn extract_sha256_hex(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|word| word.len() == 64 && word.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_string)
}

fn extract_libcxx_aar(aar: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest)
        .map_err(|e| format!("创建解包目录 {} 失败: {e}", dest.display()))?;
    if cfg!(windows) {
        // Windows 10+ 自带 bsdtar 可解 zip；必须显式用 System32 的 tar.exe，
        // 避免 PATH 里 Git Bash 的 GNU tar（不支持 zip）抢先。
        let system_tar = PathBuf::from(r"C:\Windows\System32\tar.exe");
        let status = if system_tar.is_file() {
            Command::new(&system_tar)
                .arg("-xf")
                .arg(aar)
                .arg("-C")
                .arg(dest)
                .status()
        } else {
            Command::new("tar")
                .arg("-xf")
                .arg(aar)
                .arg("-C")
                .arg(dest)
                .status()
        };
        let status = status.map_err(|e| format!("启动 tar 失败: {e}"))?;
        if !status.success() {
            return Err(format!("tar 解包 AAR 失败: exit {status}"));
        }
    } else {
        let status = Command::new("unzip")
            .arg("-q")
            .arg(aar)
            .arg("-d")
            .arg(dest)
            .status()
            .map_err(|e| format!("启动 unzip 失败: {e}"))?;
        if !status.success() {
            return Err(format!("unzip 解包 AAR 失败: exit {status}"));
        }
    }
    Ok(())
}

// AGP prefab 为该包生成的 cxxConfig.cmake 等价物：定义 cxx::cxx 并暴露捆绑的
// libc++ 头文件，真正的 std 模块注册由 AAR 自带的 prefab/cmake-config.cmake 完成。
fn cxx_config_shim_content() -> &'static str {
    r#"# 由 build.rs 生成：直接 CMake 消费 org.lsposed.libcxx AAR（不经 AGP prefab）。
get_filename_component(_srx_cxx_aar_root "${CMAKE_CURRENT_LIST_DIR}/../../.." ABSOLUTE)
add_library(cxx::cxx INTERFACE IMPORTED)
set_target_properties(cxx::cxx PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "${_srx_cxx_aar_root}/prefab/modules/cxx/include")
include("${_srx_cxx_aar_root}/prefab/modules/cxx/include/prefab/cmake-config.cmake")
"#
}

fn ensure_cxx_config_shim(prefix: &Path) -> Result<(), String> {
    let shim_dir = prefix.join("lib").join("cmake").join("cxx");
    let shim = shim_dir.join("cxxConfig.cmake");
    if shim.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(&shim_dir)
        .map_err(|e| format!("创建 {} 失败: {e}", shim_dir.display()))?;
    std::fs::write(&shim, cxx_config_shim_content())
        .map_err(|e| format!("写入 {} 失败: {e}", shim.display()))
}

fn run_command(command: &mut Command, label: &str) {
    let status = command
        .status()
        .unwrap_or_else(|err| panic!("{label} failed to start: {err}"));
    if !status.success() {
        panic!("{label} failed: {status}");
    }
}

// 从 java_src 生成 Hooker.dex 到 OUT_DIR；仅开发/host 场景允许显式降级为空文件
fn build_hooker_dex(target_os: &str) {
    println!("cargo:rerun-if-changed=java_src");
    println!("cargo:rerun-if-env-changed=ANDROID_HOME");
    println!("cargo:rerun-if-env-changed=ANDROID_SDK_ROOT");
    println!("cargo:rerun-if-env-changed=JAVA_HOME");
    println!("cargo:rerun-if-env-changed=SRX_ALLOW_EMPTY_HOOKER_DEX");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let dex_out = out_dir.join("Hooker.dex");
    let receiver_dex_out = out_dir.join("PackageEventReceiver.dex");
    let java_src_dir = PathBuf::from("java_src");
    if let Ok(java_files) = collect_java_files(&java_src_dir) {
        for java_file in java_files {
            println!("cargo:rerun-if-changed={}", java_file.display());
        }
    }

    match compile_dex(&java_src_dir, &out_dir, &dex_out).and_then(|_| validate_hooker_dex(&dex_out))
    {
        Ok(()) => {}
        Err(err) => {
            if should_allow_empty_hooker_dex(target_os) {
                println!("cargo:warning=srx_core: Hooker.dex build skipped: {err}");
                let _ = std::fs::write(&dex_out, b"");
            } else {
                panic!("Hooker.dex build failed: {err}");
            }
        }
    }

    let receiver_source = java_src_dir.join("org/srx/hook/PackageEventReceiver.java");
    match compile_dex_sources(
        &[receiver_source],
        &out_dir,
        "package_event_receiver_classes",
        &receiver_dex_out,
        Some("org/srx/hook/PackageEventReceiver.class"),
    )
    .and_then(|_| validate_hooker_dex(&receiver_dex_out))
    {
        Ok(()) => {}
        Err(err) => {
            if should_allow_empty_hooker_dex(target_os) {
                println!("cargo:warning=srx_core: PackageEventReceiver.dex build skipped: {err}");
                let _ = std::fs::write(&receiver_dex_out, b"");
            } else {
                panic!("PackageEventReceiver.dex build failed: {err}");
            }
        }
    }
}

fn compile_dex(java_src_dir: &Path, out_dir: &Path, dex_out: &Path) -> Result<(), String> {
    let java_files = collect_java_files(java_src_dir)?;
    compile_dex_sources(
        &java_files,
        out_dir,
        "java_classes",
        dex_out,
        Some("org/srx/hook/Hooker.class"),
    )
}

fn compile_dex_sources(
    java_files: &[PathBuf],
    out_dir: &Path,
    classes_dir_name: &str,
    dex_out: &Path,
    expected_class: Option<&str>,
) -> Result<(), String> {
    let javac = locate_javac().ok_or_else(|| "javac not found".to_string())?;
    let d8 = locate_d8().ok_or_else(|| "d8 not found".to_string())?;
    let android_jar = locate_android_jar().ok_or_else(|| "android.jar not found".to_string())?;
    if java_files.is_empty() {
        return Err("no Java sources provided".to_string());
    }

    let classes_dir = out_dir.join(classes_dir_name);
    if classes_dir.exists() {
        std::fs::remove_dir_all(&classes_dir).map_err(|e| format!("clean classes: {e}"))?;
    }
    std::fs::create_dir_all(&classes_dir).map_err(|e| format!("mkdir classes: {e}"))?;

    let mut javac_cmd = Command::new(&javac);
    javac_cmd
        .args(["--release", "11"])
        .arg("-classpath")
        .arg(android_jar)
        .arg("-d")
        .arg(&classes_dir);
    for java_file in java_files {
        javac_cmd.arg(java_file);
    }
    let javac_status = javac_cmd
        .status()
        .map_err(|e| format!("run javac {javac:?}: {e}"))?;
    if !javac_status.success() {
        return Err(format!("javac exit {javac_status}"));
    }

    if let Some(expected_class) = expected_class {
        let class_file = classes_dir.join(expected_class);
        if !class_file.exists() {
            return Err(format!("expected {class_file:?} not produced"));
        }
    }

    let class_files = collect_class_files(&classes_dir)?;

    // On Windows, d8.bat cannot handle $ in filenames (inner classes).
    // Use `java -cp d8.jar com.android.tools.r8.D8` directly instead.
    let mut d8_cmd = if cfg!(windows) {
        let java = locate_java().ok_or_else(|| "java not found".to_string())?;
        let d8_jar = d8.parent().unwrap().join("lib").join("d8.jar");
        let mut cmd = Command::new(&java);
        cmd.arg("-cp").arg(&d8_jar).arg("com.android.tools.r8.D8");
        cmd
    } else {
        Command::new(&d8)
    };
    for class_file in &class_files {
        d8_cmd.arg(class_file);
    }
    d8_cmd
        .args(["--min-api", "31"])
        .arg("--output")
        .arg(out_dir);
    let d8_status = d8_cmd.status().map_err(|e| format!("run d8: {e}"))?;
    if !d8_status.success() {
        return Err(format!("d8 exit {d8_status}"));
    }

    let produced = out_dir.join("classes.dex");
    std::fs::rename(&produced, dex_out)
        .map_err(|e| format!("rename {produced:?} -> {dex_out:?}: {e}"))?;
    Ok(())
}

fn validate_hooker_dex(dex_out: &Path) -> Result<(), String> {
    let size = std::fs::metadata(dex_out)
        .map_err(|e| format!("stat {dex_out:?}: {e}"))?
        .len();
    if size < MIN_HOOKER_DEX_BYTES {
        return Err(format!(
            "Hooker.dex too small: {size} bytes, expected at least {MIN_HOOKER_DEX_BYTES}"
        ));
    }
    Ok(())
}

fn should_allow_empty_hooker_dex(target_os: &str) -> bool {
    should_allow_empty_hooker_dex_with(
        target_os,
        env_flag("SRX_ALLOW_EMPTY_HOOKER_DEX"),
        env::var("CARGO_CFG_CLIPPY").is_ok(),
        env::var("PROFILE").unwrap_or_default() == "debug",
    )
}

fn should_allow_empty_hooker_dex_with(
    target_os: &str,
    allow_empty_env: bool,
    is_clippy: bool,
    is_debug: bool,
) -> bool {
    allow_empty_env || target_os != "android" || is_clippy || is_debug
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn collect_java_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    collect_files_by_extension(root, "java", "Java source", "found")
}

fn collect_class_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    collect_files_by_extension(root, "class", "class", "produced")
}

fn collect_files_by_extension(
    root: &Path,
    extension: &str,
    label: &str,
    empty_verb: &str,
) -> Result<Vec<PathBuf>, String> {
    fn visit(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| format!("read source dir {dir:?}: {e}"))? {
            let entry = entry.map_err(|e| format!("read source entry: {e}"))?;
            let path = entry.path();
            if path.is_dir() {
                visit(&path, extension, out)?;
            } else if path.extension().is_some_and(|ext| ext == extension) {
                out.push(path);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    visit(root, extension, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!("no {label} files {empty_verb}"));
    }
    Ok(files)
}

fn locate_javac() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "javac.exe" } else { "javac" };
    if let Ok(home) = env::var("JAVA_HOME") {
        let p = PathBuf::from(home).join("bin").join(exe);
        if p.exists() {
            return Some(p);
        }
    }
    Some(PathBuf::from("javac"))
}

fn locate_android_jar() -> Option<PathBuf> {
    let sdk = env::var("ANDROID_HOME")
        .or_else(|_| env::var("ANDROID_SDK_ROOT"))
        .ok()?;
    let platforms = PathBuf::from(sdk).join("platforms");
    let mut best: Option<(u32, PathBuf)> = None;
    for entry in std::fs::read_dir(&platforms).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(api) = name.strip_prefix("android-").and_then(|s| s.parse().ok()) else {
            continue;
        };
        let jar = entry.path().join("android.jar");
        if !jar.exists() {
            continue;
        }
        if best.as_ref().is_none_or(|(v, _)| api > *v) {
            best = Some((api, jar));
        }
    }
    best.map(|(_, p)| p)
}

fn locate_java() -> Option<PathBuf> {
    if let Ok(java_home) = env::var("JAVA_HOME") {
        let exe = if cfg!(windows) { "java.exe" } else { "java" };
        let candidate = PathBuf::from(&java_home).join("bin").join(exe);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    // fallback: assume java is on PATH
    Some(PathBuf::from(if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }))
}

fn locate_d8() -> Option<PathBuf> {
    let sdk = env::var("ANDROID_HOME")
        .or_else(|_| env::var("ANDROID_SDK_ROOT"))
        .ok()?;
    let build_tools = PathBuf::from(sdk).join("build-tools");
    let exe = if cfg!(windows) { "d8.bat" } else { "d8" };

    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    for entry in std::fs::read_dir(&build_tools).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let ver: Vec<u32> = name.split('.').filter_map(|s| s.parse().ok()).collect();
        if ver.is_empty() {
            continue;
        }
        let candidate = entry.path().join(exe);
        if !candidate.exists() {
            continue;
        }
        if best.as_ref().is_none_or(|(v, _)| &ver > v) {
            best = Some((ver, candidate));
        }
    }
    best.map(|(_, p)| p)
}
