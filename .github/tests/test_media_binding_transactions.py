"""验证媒体绑定表的约束、迁移和行身份查询。"""

import json
from pathlib import Path
import re
import sqlite3
import unittest


SOURCE = (Path(__file__).resolve().parents[2] / 'java_src/org/srx/hook/MediaBindings.java').read_text(encoding='utf8')


def sql(name, source=SOURCE):
    literal = r'"(?:[^"\\]|\\.)*"'
    declaration = re.search(
        rf'\bString\s+{re.escape(name)}\s*=\s*({literal}(?:\s*\+\s*{literal})*)\s*;', source)
    if declaration is None:
        raise ValueError(f'无法找到字符串常量 {name} 的完整声明')
    return ''.join(json.loads(value) for value in re.findall(literal, declaration.group(1)))


class SqlDeclarationTest(unittest.TestCase):
    def test_layout_and_name_boundary(self):
        for gap in (' ', '\n    ', '\t'):
            with self.subTest(gap=gap):
                source = f'String JOIN_OTHER = "wrong"; String JOIN{gap}={gap}"SELECT "{gap}+{gap}"1;";'
                self.assertEqual(sql('JOIN', source), 'SELECT 1;')

    def test_rejects_missing_or_nonliteral_expression(self):
        for source in ('String JOIN_OTHER = "wrong";', 'String JOIN = "SELECT " + variable;'):
            with self.subTest(source=source), self.assertRaisesRegex(ValueError, 'JOIN'):
                sql('JOIN', source)


class MediaBindingTransactions(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(':memory:', isolation_level=None)
        self.addCleanup(self.db.close)
        self.db.execute('PRAGMA foreign_keys=ON')
        self.db.execute('CREATE TABLE files(_id INTEGER PRIMARY KEY, _data TEXT UNIQUE COLLATE NOCASE, '
                        'volume_name TEXT, generation_added INTEGER, owner_package_name TEXT, _user_id INTEGER)')
        self.db.execute(sql('CREATE'))
        self.db.execute(sql('INVALIDATE'))
        self.db.execute(sql('MOVE_SCOPE'))
        self.db.execute(sql('REVOKE_MOVE'))

    def row(self, rid=1, owner='owner', gen=17):
        self.db.execute('INSERT INTO files VALUES(?,?,\'external_primary\',?,?,0)',
                        (rid, f'/Documents/{rid}', gen, owner))

    def bind(self, rid=1, owner='owner', path='/Documents/1'):
        return self.db.execute('INSERT INTO srx_private_media ' + sql('SELECT_BINDING') +
                               ' AND _data=? COLLATE BINARY', ('db-a', rid, owner, path)).rowcount

    def visible(self, version='db-a'):
        return [r[0] for r in self.db.execute('SELECT f._id' + sql('JOIN') + ' ORDER BY f._id', (version,))]

    def test_failed_binding_and_outer_rollback(self):
        self.db.execute('BEGIN')
        self.row(); self.assertEqual(self.bind(), 1)
        self.db.execute('SAVEPOINT failed_item')
        self.row(2)
        self.assertEqual(self.bind(2, path='/documents/2'), 0)
        self.db.execute('ROLLBACK TO failed_item'); self.db.execute('RELEASE failed_item')
        self.row(3); self.assertEqual(self.bind(3, path='/Documents/3'), 1)
        self.assertEqual(self.visible(), [1, 3])
        self.db.execute('ROLLBACK')
        self.assertEqual(self.visible(), [])
        self.assertEqual(self.db.execute('SELECT COUNT(*) FROM files').fetchone(), (0,))

    def test_delete_rollback_replace_and_id_reuse(self):
        self.row(); self.bind()
        self.db.execute('BEGIN')
        self.db.execute('DELETE FROM files'); self.assertEqual(self.visible(), [])
        self.db.execute('ROLLBACK'); self.assertEqual(self.visible(), [1])
        self.db.execute("INSERT OR REPLACE INTO files VALUES(42,'/Documents/1','external_primary',17,'owner',0)")
        self.assertEqual(self.db.execute('SELECT COUNT(*) FROM srx_private_media').fetchone(), (0,))
        self.db.execute('DELETE FROM files'); self.row()
        self.assertEqual(self.visible(), [])

    def test_revokes_all_identity_changes(self):
        for column, value in [('_id', 10), ('generation_added', 18), ('owner_package_name', 'peer'),
                              ('owner_package_name', None), ('volume_name', 'external_secondary'), ('_user_id', 10)]:
            with self.subTest(column=column, value=value):
                self.db.execute('BEGIN'); self.row(); self.bind()
                self.db.execute(f'UPDATE files SET {column}=?', (value,))
                self.assertEqual(self.db.execute('SELECT COUNT(*) FROM srx_private_media').fetchone(), (0,))
                self.db.execute('ROLLBACK')

    def test_rename_and_wrong_database(self):
        self.row(); self.bind()
        self.db.execute("INSERT INTO srx_private_media_move VALUES('/Pictures/published','owner')")
        self.db.execute("UPDATE files SET _data='/Pictures/published' WHERE _id=1")
        self.db.execute("DELETE FROM srx_private_media_move")
        self.assertEqual(self.visible(), [1]); self.assertEqual(self.visible('db-b'), [])
        self.assertEqual(self.db.execute('PRAGMA foreign_key_check').fetchall(), [])

    def test_unverified_move_never_reclaims_binding(self):
        self.row(); self.bind()
        self.db.execute("INSERT INTO srx_private_media_move VALUES('/Pictures/published','peer')")
        self.db.execute("UPDATE files SET _data='/Pictures/published'")
        self.assertEqual(self.visible(), [])
        self.db.execute("UPDATE files SET _data='/Documents/1'")
        self.assertEqual(self.visible(), [])

    def test_upsert_public_and_foreign_owner(self):
        self.row(owner='peer'); self.assertEqual(self.bind(), 0)
        self.db.execute("UPDATE files SET generation_added=18 WHERE _id=1")
        self.assertEqual(self.visible(), [])
        self.db.execute('DELETE FROM files')
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute("INSERT INTO srx_private_media VALUES(1,'db-a','external_primary',17,'owner',0)")


if __name__ == '__main__':
    unittest.main()
