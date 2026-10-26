"""映射请求侧「写后回查」的查询回退不变量检查。

系统写者（MediaProvider 的 FuseDaemon）把 mkdir/create 落到映射目标之后，会用
**原请求路径**回查 attr、并在 Java 侧走 `File.getCanonicalFile()` → realpath 校验。
请求路径在真实存储里并不存在（只是应用视图里的 bind 挂载点），这两步都会失败：

* attr 回查失败 → FUSE 把 ENOENT 回给调用方（DownloadProvider 的 mkdir 拿到
  -1/ENOENT，映射目标里的目录却已建好，2026-10-26 A17 场景 39）。
* realpath 失败 → `File.mkdirs()` 在 `getCanonicalFile()` 处 catch 到 IOException
  并返回 false，AOSP Helpers 随即抛 "Failed to create parent for ..."。

这里守住三条容易在后续改动中被收回的约束：写成功后必须留下**进程级**登记（线程级
登记覆盖不到 FUSE 读线程）、realpath 必须单独 hook（bionic 内部逐级走非导出的
`__lstat`，PLT 侧抓不到）、回退只允许发生在系统写者进程内且不伪造含 `.`/`..`
的路径。
"""

import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def function_body(source: str, signature: str) -> str:
    """取出以 `signature` 开头的函数体，按花括号配对，跳过字符串字面量。"""
    start = source.index(signature)
    open_index = source.index("{", start)
    depth = 0
    index = open_index
    in_string = False
    while index < len(source):
        char = source[index]
        if in_string:
            if char == "\\":
                index += 2
                continue
            if char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[open_index : index + 1]
        index += 1
    raise AssertionError(f"未找到完整函数体: {signature}")


class MappedDirQueryFallbackTest(unittest.TestCase):
    def test_realpath_is_hooked_as_required_entry(self) -> None:
        entries = read("src/hook/entries.rs")
        self.assertIn('symbol: "realpath"', entries)
        self.assertIn("hooked_realpath", entries)
        body = function_body(entries, "pub fn build_hook_entries()")
        realpath_entry = body[body.index('symbol: "realpath"') :][:400]
        # realpath 是 POSIX 必备符号，缺失说明 hook 框架异常，按必需项处理才能及时发现
        self.assertIn("is_optional: false", realpath_entry)
        self.assertIn("profiles: PROFILE_FULL_WRITER", realpath_entry)

    def test_realpath_fallback_only_after_original_failure(self) -> None:
        query = read("src/hook/ops/query.rs")
        body = function_body(query, "unsafe fn realpath_with_mapped_fallback(")
        # 先跑原函数，成功即返回；任何回退都必须在原调用失败之后
        self.assertIn("call_realpath_prev(proxy_fn, path, resolved)", body)
        self.assertLess(
            body.index("call_realpath_prev(proxy_fn, path, resolved)"),
            body.index("mapped_request_realpath(hub, &path_text)"),
        )
        self.assertIn("if !ret.is_null()", body)
        self.assertIn("is_retryable_system_writer_query_error(error_no)", body)

    def test_realpath_fallback_is_limited_to_system_writer(self) -> None:
        query = read("src/hook/ops/query.rs")
        body = function_body(query, "unsafe fn realpath_with_mapped_fallback(")
        self.assertIn("hub.with_package_name(policy::is_system_writer_package)", body)
        # 应用进程里请求侧由 bind 挂载点承接，不该被伪造
        self.assertIn("return ret", body)

    def test_realpath_fallback_rejects_dot_segments(self) -> None:
        query = read("src/hook/ops/query.rs")
        body = function_body(query, "fn mapped_request_realpath(")
        self.assertIn('*segment == "."', body)
        self.assertIn('*segment == ".."', body)
        # 未命中进程级登记时不得伪造
        self.assertIn("crate::hook::process_mapped_dir_target(&normalized)", body)

    def test_realpath_writes_bounded_output(self) -> None:
        query = read("src/hook/ops/query.rs")
        body = function_body(query, "unsafe fn fill_realpath_result(")
        self.assertIn("REALPATH_BUFFER_LIMIT", body)
        self.assertIn("libc::ENAMETOOLONG", body)
        # resolved 为 NULL 时按 bionic 契约返回 malloc 内存，由调用方 free
        self.assertIn("libc::malloc(length)", body)

    def test_process_scoped_registry_has_ttl_and_cap(self) -> None:
        context = read("src/hook/context.rs")
        self.assertIn("PROCESS_MAPPED_DIR_TTL_MS", context)
        self.assertIn("PROCESS_MAPPED_DIR_MAX", context)
        remember = function_body(context, "pub fn remember_process_mapped_dir(")
        self.assertIn("virtual_dir_key(request_path)", remember)
        self.assertIn("retain(", remember)
        lookup = function_body(context, "pub fn process_mapped_dir_target(")
        # 过期项必须在读取时清理，否则删除后重建的同名目录会继续报「存在」
        self.assertIn("retain(", lookup)
        self.assertIn("max_by_key", lookup)

    def test_query_fallback_accepts_process_scoped_signal(self) -> None:
        query = read("src/hook/ops/query.rs")
        body = function_body(query, "fn should_attempt_system_writer_query_fallback(")
        self.assertIn("has_process_mapped_dir_signal_for_query_fallback(path)", body)
        signal = function_body(query, "fn has_process_mapped_dir_signal_for_query_fallback(")
        self.assertIn("crate::hook::process_mapped_dir_target(value)", signal)

    def test_mapping_redirect_registers_process_scoped_dir(self) -> None:
        dir_rs = read("src/hook/ops/mutation/dir.rs")
        body = function_body(dir_rs, "fn handle_mkdir_like<F>(")
        self.assertIn("crate::hook::remember_process_mapped_dir(", body)
        # 只在映射重定向后登记：普通沙箱重定向的源目录通常已在真实存储落地
        register_index = body.index("crate::hook::remember_process_mapped_dir(")
        guard = body[:register_index]
        self.assertIn("if redirect_result.is_mapping {", guard)

    def test_realpath_fallback_also_consults_configured_mappings(self) -> None:
        # MediaProvider 的 FuseDaemon 把下层写直接打到映射目标上，不经过 hook，
        # 进程内不会有任何登记。realpath 回退若只认进程登记表，在「下载器经 MP
        # 代写」这条链路上永远不命中（run 40291854472 实证：整条链路只留下一条
        # req map）。因此必须带配置驱动兜底。
        query = read("src/hook/ops/query.rs")
        body = function_body(query, "fn mapped_request_realpath(")
        self.assertIn("crate::hook::process_mapped_dir_target(&normalized)", body)
        self.assertIn("writer::resolve_system_writer_user_id(&normalized", body)
        self.assertIn("resolve_mapping_request_owner_for_path(user_id, &normalized)", body)
        # 进程登记优先：同进程刚做过 mkdir 重定向的路径最可靠。
        registry_at = body.index("crate::hook::process_mapped_dir_target(&normalized)")
        config_at = body.index("resolve_mapping_request_owner_for_path(user_id, &normalized)")
        self.assertLess(registry_at, config_at, "进程内登记必须优先于配置兜底")


    def test_mapping_mkdir_lands_request_dir_before_target(self) -> None:
        # 映射场景的 mkdir 不能只改写目标：请求路径是应用视图里的 bind 挂载点，
        # 未登记进程（下载提供器）走的是 MediaProvider 的 FUSE 视图，请求路径在
        # 后端没有真实目录时内核拿不到可用的 FUSE entry——daemon 侧返回成功，
        # 客户端仍然 isDirectory()==false && mkdirs()==false
        # （2026-10-26 A17 场景 39：Failed to create parent）。
        dir_source = read("src/hook/ops/mutation/dir.rs")
        body = function_body(dir_source, "fn handle_mapping_mkdir<F>(")
        self.assertIn("storage_backend_dir(source_path)", body)
        self.assertIn("create_backend_dir(&request_backend)", body)
        self.assertIn("create_backend_dir(&target_backend)", body)
        # 请求路径必须先落盘，再补齐映射目标
        self.assertLess(
            body.index("create_backend_dir(&request_backend)"),
            body.index("create_backend_dir(&target_backend)"),
            "请求路径后端目录必须先于映射目标落盘",
        )
        # 请求路径的目录创建不得再走重定向（否则又回到只建目标的老路）：
        # 建目录的 helper 必须直接落到后端并接受「已是目录」
        helper = function_body(dir_source, "fn create_backend_dir(")
        self.assertIn("crate::platform::fs::is_directory(path)", helper)
        self.assertIn("libc::mkdir(", helper)
        # 映射场景要提前收口，不能继续走「只 mkdir 映射目标」的统一收尾
        outer = function_body(dir_source, "fn handle_mkdir_like<F>(")
        self.assertIn("if redirect_result.is_mapping {", outer)
        self.assertLess(
            outer.index("if redirect_result.is_mapping {"),
            outer.index("let result = if redirect_result.is_redirect() {"),
            "映射分支必须在通用重定向收尾之前",
        )

    def test_download_provider_gets_system_writer_hooks(self) -> None:
        # 下载提供器是下载链路里真正执行 mkdir/create/getCanonicalPath 的进程。
        # 只把它当 monitor bridge 时，客户端路径判定拿不到映射（A17 场景 39 本地
        # 与 CI 双复现），因此必须纳入 system-writer 候选，拿到 stat/realpath/open。
        policy = read("src/redirect/policy.rs")
        body = function_body(policy, "fn is_system_writer_candidate(")
        self.assertIn("is_media_provider_candidate(package_name)", body)
        self.assertIn("package_name == DOWNLOAD_PROVIDER", body)


if __name__ == "__main__":
    unittest.main()
