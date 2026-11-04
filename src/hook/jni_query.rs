// CursorWindow 查询层 Hook：按重定向规则过滤与改写 MediaStore 查询结果

mod dir_snapshot;
mod path_util;
mod rewrite;
mod types;

pub(crate) use dir_snapshot::path_exists_for_cursor_row;
pub(crate) use rewrite::{
    cursor_row_action, is_redirect_enabled_for_caller_uid,
    resolve_download_media_placeholder_path_for_caller, resolve_media_store_direct_path_for_caller,
    resolve_open_storage_path_for_caller, rewrite_cursor_storage_path_for_caller,
    rewrite_media_store_bucket_id_for_caller, rewrite_media_store_storage_path_for_caller,
    should_hide_cursor_storage_path_for_caller, storage_path_exists_by_syscall,
};
