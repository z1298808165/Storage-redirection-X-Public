"""执行生产 APK 定位，验证可执行映射、多个 ABI 与同名库候选。"""

import shutil
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path

from test_mount_identity_syntax import ROOT, extract_fn


class ElfImgMappingTest(unittest.TestCase):
    def test_only_mapped_native_entries_are_read(self):
        rustc = shutil.which("rustc")
        self.assertIsNotNone(rustc)
        source = (ROOT / "src/platform/elf_img.rs").read_text(encoding="utf8")
        functions = "\n".join(
            extract_fn(source, name)
            for name in (
                "find_embedded_elf", "find_embedded_elf_load_bias",
                "find_zip_stored_elf", "find_eocd", "read_u16", "read_u32",
            )
        ).replace("std::fs::", "fs::")
        harness = r'''
use std::sync::Mutex;
extern crate self as log;
#[macro_export] macro_rules! warn { ($($arg:tt)*) => {}; }
static MAPS: Mutex<String> = Mutex::new(String::new());
static READS: Mutex<Vec<String>> = Mutex::new(Vec::new());
struct EmbeddedElf {
    apk_path: String, entry_name: String, data_offset: usize, data_size: usize, image: Vec<u8>,
}
mod fs {
    pub fn read_to_string(path: &str) -> Result<String, ()> {
        assert_eq!(path, "/proc/self/maps");
        Ok(crate::MAPS.lock().unwrap().clone())
    }
    pub fn read(path: &str) -> Result<Vec<u8>, ()> {
        crate::READS.lock().unwrap().push(path.into());
        Ok(include_bytes!("media.apk").to_vec())
    }
}
fn resolve_map_path(path: &str) -> String { path.into() }
// __FUNCTIONS__
fn main() {
    let resources = "1000-2000 r--p 0000 0:1 1 /framework-res.apk\n2000-3000 r--p 0000 0:1 2 /media.apk\n3000-4000 r-xp 0000 0:1 3 /libart.so\n";
    *MAPS.lock().unwrap() = resources.into();
    assert!(find_embedded_elf("libfuse_jni.so").is_none());
    assert!(READS.lock().unwrap().is_empty(), "没有原生映射不能读取 APK");
    assert!(find_embedded_elf_load_bias("/media.apk", X64_OFFSET, 0x3000).is_none());
    // dex 段和未加载的 32 位同名条目在前，都不能遮住已加载的 64 位库。
    let native = format!("9000-a000 r-xp {:x} 0:1 2 /media.apk\na000-b000 r-xp {:x} 0:1 2 /media.apk\n", X64_OFFSET + 0x1000, X64_OFFSET + 0x2000);
    *MAPS.lock().unwrap() = format!("{resources}{native}");
    let found = find_embedded_elf("libfuse_jni.so").unwrap();
    assert_eq!(found.apk_path, "/media.apk");
    assert_eq!(found.entry_name, "lib/x86_64/libfuse_jni.so");
    assert_eq!((found.data_offset, found.data_size), (X64_OFFSET, 0x3000));
    assert_eq!(&found.image[..4], b"\x7fELF");
    assert_eq!(*READS.lock().unwrap(), vec!["/media.apk"]);
    assert_eq!(find_embedded_elf_load_bias("/media.apk", X64_OFFSET, 0x3000), Some(0x8000));
    READS.lock().unwrap().clear();
    assert!(find_embedded_elf("missing.so").is_none());
    assert_eq!(*READS.lock().unwrap(), vec!["/media.apk"], "重复可执行段只读取一次");
    // 同名库只在另一个 APK 或当前 APK 未映射的区间内时，继续找后续候选。
    READS.lock().unwrap().clear();
    *MAPS.lock().unwrap() = format!("{resources}5000-6000 r-xp 100000 0:1 4 /other.apk\n{native}");
    assert_eq!(find_embedded_elf("libfuse_jni.so").unwrap().apk_path, "/media.apk");
    assert_eq!(*READS.lock().unwrap(), vec!["/other.apk", "/media.apk"]);
    *MAPS.lock().unwrap() = "9000-a000 r-xp 100000 0:1 2 /media.apk\n".into();
    assert!(find_embedded_elf("libfuse_jni.so").is_none());
    *MAPS.lock().unwrap() = format!("6000-7000 r-xp {ARM_OFFSET:x} 0:1 2 /media.apk\n");
    assert_eq!(find_embedded_elf("libfuse_jni.so").unwrap().entry_name, "lib/armeabi-v7a/libfuse_jni.so");
}
'''.replace("// __FUNCTIONS__", functions)
        with tempfile.TemporaryDirectory(prefix="srx-elf-mappings-") as directory:
            directory = Path(directory)
            offsets = []
            with zipfile.ZipFile(directory / "media.apk", "w") as archive:
                for abi in ("armeabi-v7a", "x86_64"):
                    entry = zipfile.ZipInfo(f"lib/{abi}/libfuse_jni.so")
                    # 对齐存储条目，使夹具的文件偏移与原生 APK 映射一致。
                    padding = -(archive.fp.tell() + 30 + len(entry.filename) + 4) % 4096
                    entry.extra = b"\xfe\xfe" + padding.to_bytes(2, "little") + bytes(padding)
                    offsets.append(archive.fp.tell() + 30 + len(entry.filename) + len(entry.extra))
                    archive.writestr(entry, b"\x7fELF" + bytes(0x3000 - 4))
            harness += f"\nconst ARM_OFFSET: usize = {offsets[0]};\nconst X64_OFFSET: usize = {offsets[1]};\n"
            source, binary = directory / "main.rs", directory / "mappings"
            mutants = (
                harness.replace("!perms.contains('x')", "false", 1),
                harness.replace("find_embedded_elf_load_bias(apk_path, data_offset, data_size).is_some()", "true", 1),
            )
            for mutant in mutants:
                self.assertNotEqual(harness, mutant)
            for code, succeeds in ((harness, True), *((mutant, False) for mutant in mutants)):
                source.write_text(code, encoding="utf8")
                compiled = subprocess.run([rustc, "--edition=2024", str(source), "-o", str(binary)], capture_output=True, text=True, timeout=30)
                self.assertEqual(compiled.returncode, 0, compiled.stderr)
                result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode == 0, succeeds, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
