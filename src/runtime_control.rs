use crate::platform::module_paths;

pub fn is_module_runtime_enabled() -> bool {
    std::fs::metadata(module_paths::RUNTIME_DISABLE_FILE).is_err()
}
