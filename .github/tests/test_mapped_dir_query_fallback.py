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
            body.index("mapped_request_realpath(&path_text)"),
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
        self.assertIn("crate::hook::process_mapped_dir_target(&normalized)?", body)

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


if __name__ == "__main__":
    unittest.main()
