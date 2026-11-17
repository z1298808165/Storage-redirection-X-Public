#[path = "../../monitor/hint_file.rs"]
mod hint_file;
#[path = "../../monitor/source_hint.rs"]
mod source_hint;

// daemon 不承载 hook 线程的请求作用域；日志入口另行核验发送者与记录边界。
fn is_caller_attribution_stopped() -> bool {
    false
}

pub(crate) use source_hint::{
    has_recent_public_path_request, infer_public_path_package_name,
    infer_recent_path_caller_identity, remember_public_path_caller_hint_with_op_filter,
};
