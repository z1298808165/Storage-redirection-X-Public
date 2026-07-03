use crate::zygisk::{abi, jni};

pub(super) struct ProcessIdentity {
    pub(super) nice_name: String,
    pub(super) package_name: String,
    pub(super) uid: i32,
    pub(super) pid: i32,
}

impl ProcessIdentity {
    pub(super) fn from_args(
        env: *mut jni_sys::JNIEnv,
        args: &abi::AppSpecializeArgs,
    ) -> Option<Self> {
        let nice_name = if args.nice_name.is_null() {
            String::new()
        } else {
            jni::get_jstring_utf8(env, unsafe { *args.nice_name })
        };
        if nice_name.is_empty() {
            return None;
        }

        let mut package_name = nice_name.clone();
        if let Some(pos) = package_name.find(':') {
            package_name.truncate(pos);
        }

        Some(Self {
            nice_name,
            package_name,
            uid: unsafe { *args.uid },
            pid: unsafe { libc::getpid() } as i32,
        })
    }
}
