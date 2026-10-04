#[path = "../../monitor/hint_file.rs"]
mod hint_file;
#[path = "../../monitor/source_hint.rs"]
mod source_hint;

pub(crate) use source_hint::{infer_public_path_package_name, infer_recent_path_caller_identity};
