#[repr(u32)]
pub enum HookProfile {
    Full = 1 << 0,
    Monitor = 1 << 1,
    SystemWriter = 1 << 2,
    FuseFix = 1 << 3,
    SystemWriterBootLite = 1 << 4,
    AppWrite = 1 << 5,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HookProfileSet {
    mask: u32,
}

impl HookProfileSet {
    pub const fn from_profile(profile: HookProfile) -> Self {
        Self {
            mask: profile as u32,
        }
    }

    pub const fn with(self, profile: HookProfile) -> Self {
        Self {
            mask: self.mask | profile as u32,
        }
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.mask & other.mask) != 0
    }
}

const PROFILE_RW_MONITORED: HookProfileSet = HookProfileSet::from_profile(HookProfile::Full)
    .with(HookProfile::Monitor)
    .with(HookProfile::SystemWriter)
    .with(HookProfile::SystemWriterBootLite);
const PROFILE_OPEN_MONITORED: HookProfileSet = PROFILE_RW_MONITORED.with(HookProfile::AppWrite);
const PROFILE_FULL_WRITER: HookProfileSet = HookProfileSet::from_profile(HookProfile::Full)
    .with(HookProfile::SystemWriter)
    .with(HookProfile::SystemWriterBootLite);
const PROFILE_READ_RUNTIME: HookProfileSet =
    HookProfileSet::from_profile(HookProfile::SystemWriter)
        .with(HookProfile::FuseFix)
        .with(HookProfile::SystemWriterBootLite);
const PROFILE_BINDER_WRITER: HookProfileSet =
    HookProfileSet::from_profile(HookProfile::SystemWriter).with(HookProfile::SystemWriterBootLite);
const PROFILE_LATE_LOAD_REFRESH: HookProfileSet =
    HookProfileSet::from_profile(HookProfile::AppWrite);

pub struct HookEntry {
    pub symbol: &'static str,
    pub new_func: *mut std::ffi::c_void,
    pub is_optional: bool,
    pub profiles: HookProfileSet,
}

pub fn is_hook_enabled(active_profiles: HookProfileSet, entry_profiles: HookProfileSet) -> bool {
    active_profiles.intersects(entry_profiles)
}

pub fn count_hooks_for_profile(active_profiles: HookProfileSet) -> usize {
    build_hook_entries()
        .into_iter()
        .filter(|entry| is_hook_enabled(active_profiles, entry.profiles))
        .count()
}

pub fn build_hook_entries() -> Vec<HookEntry> {
    vec![
        HookEntry {
            symbol: "open",
            new_func: super::ops::open::hooked_open as *mut _,
            is_optional: false,
            profiles: PROFILE_OPEN_MONITORED,
        },
        HookEntry {
            symbol: "open64",
            new_func: super::ops::open::hooked_open as *mut _,
            is_optional: true,
            profiles: PROFILE_OPEN_MONITORED,
        },
        HookEntry {
            symbol: "__open_2",
            new_func: super::ops::open::hooked_open as *mut _,
            is_optional: true,
            profiles: PROFILE_OPEN_MONITORED,
        },
        HookEntry {
            symbol: "openat",
            new_func: super::ops::open::hooked_openat as *mut _,
            is_optional: false,
            profiles: PROFILE_OPEN_MONITORED,
        },
        HookEntry {
            symbol: "openat2",
            new_func: super::ops::open::hooked_openat2 as *mut _,
            is_optional: true,
            profiles: PROFILE_OPEN_MONITORED,
        },
        HookEntry {
            symbol: "openat64",
            new_func: super::ops::open::hooked_openat as *mut _,
            is_optional: true,
            profiles: PROFILE_OPEN_MONITORED,
        },
        HookEntry {
            symbol: "__openat_2",
            new_func: super::ops::open::hooked_openat as *mut _,
            is_optional: true,
            profiles: PROFILE_OPEN_MONITORED,
        },
        HookEntry {
            symbol: "creat",
            new_func: super::ops::open::hooked_creat as *mut _,
            is_optional: true,
            profiles: PROFILE_OPEN_MONITORED,
        },
        HookEntry {
            symbol: "stat",
            new_func: super::ops::query::hooked_stat as *mut _,
            is_optional: false,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "lstat",
            new_func: super::ops::query::hooked_lstat as *mut _,
            is_optional: false,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "fstatat",
            new_func: super::ops::query::hooked_fstatat as *mut _,
            is_optional: true,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "fstatat64",
            new_func: super::ops::query::hooked_fstatat as *mut _,
            is_optional: true,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "__fstatat64",
            new_func: super::ops::query::hooked_fstatat as *mut _,
            is_optional: true,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "newfstatat",
            new_func: super::ops::query::hooked_fstatat as *mut _,
            is_optional: true,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "access",
            new_func: super::ops::query::hooked_access as *mut _,
            is_optional: false,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "faccessat",
            new_func: super::ops::query::hooked_faccessat as *mut _,
            is_optional: true,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "faccessat2",
            new_func: super::ops::query::hooked_faccessat as *mut _,
            is_optional: true,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "statx",
            new_func: super::ops::query::hooked_statx as *mut _,
            is_optional: true,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "mkdir",
            new_func: super::ops::mutation::hooked_mkdir as *mut _,
            is_optional: false,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "mkdirat",
            new_func: super::ops::mutation::hooked_mkdirat as *mut _,
            is_optional: false,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "mknod",
            new_func: super::ops::mutation::hooked_mknod as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "mknodat",
            new_func: super::ops::mutation::hooked_mknodat as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "unlink",
            new_func: super::ops::mutation::hooked_unlink as *mut _,
            is_optional: false,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "unlinkat",
            new_func: super::ops::mutation::hooked_unlinkat as *mut _,
            is_optional: false,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "rmdir",
            new_func: super::ops::mutation::hooked_rmdir as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "link",
            new_func: super::ops::mutation::hooked_link as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "linkat",
            new_func: super::ops::mutation::hooked_linkat as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "symlink",
            new_func: super::ops::mutation::hooked_symlink as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "symlinkat",
            new_func: super::ops::mutation::hooked_symlinkat as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "truncate",
            new_func: super::ops::mutation::hooked_truncate as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "truncate64",
            new_func: super::ops::mutation::hooked_truncate64 as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "ftruncate",
            new_func: super::ops::mutation::hooked_ftruncate as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "ftruncate64",
            new_func: super::ops::mutation::hooked_ftruncate64 as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "chmod",
            new_func: super::ops::mutation::hooked_chmod as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "fchmod",
            new_func: super::ops::mutation::hooked_fchmod as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "fchmodat",
            new_func: super::ops::mutation::hooked_fchmodat as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "utimensat",
            new_func: super::ops::mutation::hooked_utimensat as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "futimens",
            new_func: super::ops::mutation::hooked_futimens as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "rename",
            new_func: super::ops::rename::hooked_rename as *mut _,
            is_optional: false,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "renameat",
            new_func: super::ops::rename::hooked_renameat as *mut _,
            is_optional: false,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "renameat2",
            new_func: super::ops::rename::hooked_renameat2 as *mut _,
            is_optional: true,
            profiles: PROFILE_RW_MONITORED,
        },
        HookEntry {
            symbol: "opendir",
            new_func: super::ops::query::hooked_opendir as *mut _,
            is_optional: false,
            profiles: HookProfileSet::from_profile(HookProfile::Full),
        },
        HookEntry {
            symbol: "readlink",
            new_func: super::ops::query::hooked_readlink as *mut _,
            is_optional: false,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "readlinkat",
            new_func: super::ops::query::hooked_readlinkat as *mut _,
            is_optional: false,
            profiles: PROFILE_FULL_WRITER,
        },
        HookEntry {
            symbol: "read",
            new_func: super::ops::read::hooked_read as *mut _,
            is_optional: false,
            profiles: PROFILE_READ_RUNTIME,
        },
        HookEntry {
            symbol: "dlopen",
            new_func: super::ops::dlopen::hooked_dlopen as *mut _,
            is_optional: true,
            profiles: PROFILE_LATE_LOAD_REFRESH,
        },
        HookEntry {
            symbol: "android_dlopen_ext",
            new_func: super::ops::dlopen::hooked_android_dlopen_ext as *mut _,
            is_optional: true,
            profiles: PROFILE_LATE_LOAD_REFRESH,
        },
        HookEntry {
            symbol: "_ZN7android14IPCThreadState20clearCallingIdentityEv",
            new_func: super::ops::binder::hooked_clear_calling_identity as *mut _,
            is_optional: true,
            profiles: PROFILE_BINDER_WRITER,
        },
        HookEntry {
            symbol: "_ZN7android14IPCThreadState21restoreCallingIdentityEl",
            new_func: super::ops::binder::hooked_restore_calling_identity as *mut _,
            is_optional: true,
            profiles: PROFILE_BINDER_WRITER,
        },
    ]
}
