"""配置规范化规则在 App（Kotlin）与 WebUI（JS）两侧的跨语言一致性不变量。

`SrxConfigNormalizer.kt` 与 `api.js` 各自实现同一套监视过滤路径规范化规则：
两套实现无法合并成单一源码（运行环境不同），因此用源码级不变量钉住关键规则，
防止一侧修订规则时另一侧漂移。任何一侧改动规则时必须两侧同步，并让本守卫
保持在绿。规则以两侧各自的惯用写法锚定，断言的是「规则值」而非实现细节。
"""

import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


# 两侧都必须拒绝的存储根前缀：规范化后的过滤路径只能落在共享存储的相对子路径上，
# 整根或存储别名会让过滤语义退化为「排除全部存储」。
STORAGE_ROOT_PREFIXES = (
    "sdcard",
    "sdcard/",
    "storage/emulated",
    "storage/emulated/",
    "storage/self/primary",
    "storage/self/primary/",
    "data/media",
    "data/media/",
)


class ConfigNormalizerParityTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.kotlin = read("app/src/main/java/org/srx/manager/data/SrxConfigNormalizer.kt")
        cls.js = read("assets/zygisk_module/webroot/js/api.js")

    def test_storage_root_prefixes_rejected_on_both_sides(self) -> None:
        for prefix in STORAGE_ROOT_PREFIXES:
            quoted = f'"{prefix}"'
            self.assertIn(quoted, self.kotlin, prefix)
            self.assertIn(quoted, self.js, prefix)

    def test_path_length_cap_matches(self) -> None:
        # 单条过滤路径长度上限 512。
        self.assertIn("512", self.kotlin)
        self.assertIn("512", self.js)

    def test_entry_cap_and_dedupe_match(self) -> None:
        # 列表上限 200 条，且两侧都必须去重。
        self.assertIn("200", self.kotlin)
        self.assertIn("distinct()", self.kotlin)
        self.assertIn("200", self.js)
        self.assertIn("seen.has(value)", self.js)

    def test_rejected_shapes_match(self) -> None:
        # 拒绝语义：排除项前缀 `!`、相对段 `.`/`..`、同一批不安全字符。
        self.assertIn('startsWith("!")', self.kotlin)
        self.assertIn('value.startsWith("!")', self.js)
        self.assertIn('it == "." || it == ".."', self.kotlin)
        self.assertIn('part === "." || part === ".."', self.js)
        # 不安全字符类：Kotlin 字符串里带转义、JS 为字面正则，两侧字符集合一致。
        self.assertIn(r'[<>:\"|\\x00-\\x1F]', self.kotlin)
        self.assertIn(r'[<>:"|\x00-\x1f]', self.js)

    def test_sort_order_matches(self) -> None:
        # 排序口径：先按小写比较，再按原串比较；两侧行为必须等价。
        self.assertIn("it.lowercase()", self.kotlin)
        self.assertIn("toLowerCase()", self.js)
        self.assertIn(".sort(compareMonitorFilterValues)", self.js)

    def test_legacy_absolute_paths_stripped_on_both_sides(self) -> None:
        # 遗留绝对路径按「去掉前导斜杠」收编，两侧都必须保留该宽容分支。
        self.assertIn("allowLegacyAbsolute", self.kotlin)
        self.assertIn("allowLegacyAbsolute", self.js)


if __name__ == "__main__":
    unittest.main()
