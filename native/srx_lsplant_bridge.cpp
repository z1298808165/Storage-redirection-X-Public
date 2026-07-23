#include <jni.h>
#include <srx_inline_hook.h>
#include <android/log.h>
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <sys/mman.h>
#include <time.h>
#include <strings.h>
#include <unistd.h>

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <cstdarg>
#include <mutex>
#include <string>
#include <string_view>
#include <unordered_map>

#include "lsplant.hpp"

extern "C" void *srx_art_symbol_resolve(const char *name);
extern "C" void *srx_art_symbol_resolve_prefix(const char *prefix);
extern "C" bool
srx_should_allow_fuse_private_owner_sqlite_access(const char *path,
                                                  uint32_t uid);
extern "C" bool srx_should_allow_fuse_public_mapping_access(const char *path,
                                                            uint32_t uid);
extern "C" bool
srx_should_force_fuse_userspace_private_owner_sqlite(const char *path);
extern "C" bool srx_prepare_fuse_private_owner_sqlite_sidecar(const char *path);
extern "C" bool srx_is_debug_logging_enabled();
namespace {

constexpr int64_t kVerbosePassthroughLogIntervalUs = 5 * 1000 * 1000;

std::mutex g_hook_mutex;
std::unordered_map<void *, void *> g_hook_stubs;

using IsAppAccessiblePathFn = bool (*)(void *, const std::string &, uint32_t);
using IsPackageOwnedPathFn = bool (*)(const std::string &, const std::string &);
using IsBpfBackingPathFn = bool (*)(const std::string &);
using ShouldOpenWithFuseFn = bool (*)(void *, int, bool, const std::string &);
struct SrxFuseFileInfo {
  int flags;
  uint32_t bit_flags;
  uint32_t padding2;
  uint32_t align_fh;
  uint64_t fh;
  uint32_t passthrough_fh;
  uint32_t align_lock_owner;
  uint64_t lock_owner;
  uint32_t poll_events;
  int32_t backing_id;
};
static_assert(offsetof(SrxFuseFileInfo, fh) == 16);
static_assert(offsetof(SrxFuseFileInfo, passthrough_fh) == 24);
static_assert(offsetof(SrxFuseFileInfo, backing_id) == 44);
struct SrxMediaProviderHandle {
  int fd;
};
using FuseReplyOpenFn = int (*)(void *, const SrxFuseFileInfo *);
using FuseReplyCreateFn = int (*)(void *, const void *, const SrxFuseFileInfo *);
using FusePassthroughEnableFn = int (*)(void *, unsigned int);
using FusePassthroughOpenFn = int (*)(void *, int);

IsAppAccessiblePathFn g_orig_is_app_accessible_path = nullptr;
IsPackageOwnedPathFn g_orig_is_package_owned_path = nullptr;
IsBpfBackingPathFn g_orig_is_bpf_backing_path = nullptr;
ShouldOpenWithFuseFn g_orig_should_open_with_fuse = nullptr;
FuseReplyOpenFn g_orig_fuse_reply_open = nullptr;
FuseReplyCreateFn g_orig_fuse_reply_create = nullptr;
FusePassthroughEnableFn g_orig_fuse_passthrough_enable = nullptr;
FusePassthroughOpenFn g_orig_fuse_passthrough_open = nullptr;
void *g_fuse_fix_is_app_accessible_stub = nullptr;
void *g_fuse_fix_is_package_owned_stub = nullptr;
void *g_fuse_fix_is_bpf_backing_stub = nullptr;
void *g_fuse_fix_should_open_with_fuse_stub = nullptr;
void *g_fuse_fix_reply_open_stub = nullptr;
void *g_fuse_fix_reply_create_stub = nullptr;
void *g_fuse_fix_passthrough_enable_stub = nullptr;
void *g_fuse_fix_passthrough_open_stub = nullptr;
std::atomic_bool g_fuse_fix_enabled{true};
std::atomic<int64_t> g_last_passthrough_log_us{0};
std::atomic<uint32_t> g_suppressed_passthrough_logs{0};

int64_t MonotonicTimeUs() {
  timespec ts{};
  if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0)
    return 0;
  return static_cast<int64_t>(ts.tv_sec) * 1000 * 1000 + ts.tv_nsec / 1000;
}

void LogInfoIfVerbose(const char *fmt, ...) {
  if (!srx_is_debug_logging_enabled())
    return;
  va_list args;
  va_start(args, fmt);
  __android_log_vprint(ANDROID_LOG_INFO, "StorageRedirect", fmt, args);
  va_end(args);
}

bool ShouldLogPassthroughVerbose() {
  if (!srx_is_debug_logging_enabled())
    return false;

  int64_t now_us = MonotonicTimeUs();
  if (now_us <= 0)
    return true;

  int64_t last_us = g_last_passthrough_log_us.load(std::memory_order_relaxed);
  while (last_us == 0 || now_us - last_us >= kVerbosePassthroughLogIntervalUs) {
    if (g_last_passthrough_log_us.compare_exchange_weak(
            last_us, now_us, std::memory_order_relaxed)) {
      return true;
    }
  }

  g_suppressed_passthrough_logs.fetch_add(1, std::memory_order_relaxed);
  return false;
}

bool IsDefaultIgnorableCodePoint(uint32_t ch) {
  return ch == 0x00AD || ch == 0x034F || ch == 0x061C ||
         (0x115F <= ch && ch <= 0x1160) ||
         (0x17B4 <= ch && ch <= 0x17B5) ||
         (0x180B <= ch && ch <= 0x180E) ||
         (0x200B <= ch && ch <= 0x200F) ||
         (0x202A <= ch && ch <= 0x202E) ||
         (0x2060 <= ch && ch <= 0x206F) || ch == 0x3164 ||
         (0xFE00 <= ch && ch <= 0xFE0F) || ch == 0xFEFF ||
         ch == 0xFFA0 || (0xFFF0 <= ch && ch <= 0xFFF8) ||
         (0x1BCA0 <= ch && ch <= 0x1BCA3) ||
         (0x1D173 <= ch && ch <= 0x1D17A) ||
         (0xE0000 <= ch && ch <= 0xE0FFF);
}

bool NextUtf8CodePoint(std::string_view text, size_t *index, uint32_t *out) {
  if (index == nullptr || out == nullptr || *index >= text.size())
    return false;
  const auto *bytes = reinterpret_cast<const uint8_t *>(text.data());
  size_t i = *index;
  uint8_t b0 = bytes[i];
  if (b0 < 0x80) {
    *out = b0;
    *index = i + 1;
    return true;
  }
  if ((b0 & 0xE0) == 0xC0 && i + 1 < text.size()) {
    uint8_t b1 = bytes[i + 1];
    if ((b1 & 0xC0) == 0x80) {
      *out = ((b0 & 0x1F) << 6) | (b1 & 0x3F);
      *index = i + 2;
      return true;
    }
  } else if ((b0 & 0xF0) == 0xE0 && i + 2 < text.size()) {
    uint8_t b1 = bytes[i + 1];
    uint8_t b2 = bytes[i + 2];
    if ((b1 & 0xC0) == 0x80 && (b2 & 0xC0) == 0x80) {
      *out = ((b0 & 0x0F) << 12) | ((b1 & 0x3F) << 6) | (b2 & 0x3F);
      *index = i + 3;
      return true;
    }
  } else if ((b0 & 0xF8) == 0xF0 && i + 3 < text.size()) {
    uint8_t b1 = bytes[i + 1];
    uint8_t b2 = bytes[i + 2];
    uint8_t b3 = bytes[i + 3];
    if ((b1 & 0xC0) == 0x80 && (b2 & 0xC0) == 0x80 &&
        (b3 & 0xC0) == 0x80) {
      *out = ((b0 & 0x07) << 18) | ((b1 & 0x3F) << 12) |
             ((b2 & 0x3F) << 6) | (b3 & 0x3F);
      *index = i + 4;
      return true;
    }
  }
  *out = b0;
  *index = i + 1;
  return true;
}

bool HasDefaultIgnorableCodePoint(std::string_view text) {
  size_t i = 0;
  while (i < text.size()) {
    uint32_t ch = 0;
    if (!NextUtf8CodePoint(text, &i, &ch))
      break;
    if (IsDefaultIgnorableCodePoint(ch))
      return true;
  }
  return false;
}

std::string RemoveDefaultIgnorableCodePoints(std::string_view text) {
  std::string out;
  out.reserve(text.size());
  size_t i = 0;
  while (i < text.size()) {
    size_t start = i;
    uint32_t ch = 0;
    if (!NextUtf8CodePoint(text, &i, &ch))
      break;
    if (!IsDefaultIgnorableCodePoint(ch))
      out.append(text.substr(start, i - start));
  }
  return out;
}

int SrxStrcasecmpFix(std::string_view a, std::string_view b) {
  if (!HasDefaultIgnorableCodePoint(a) && !HasDefaultIgnorableCodePoint(b)) {
    return strcasecmp(std::string(a).c_str(), std::string(b).c_str());
  }
  std::string clean_a = RemoveDefaultIgnorableCodePoints(a);
  std::string clean_b = RemoveDefaultIgnorableCodePoints(b);
  return strcasecmp(clean_a.c_str(), clean_b.c_str());
}

// 这些回调运行在 MediaProvider FUSE 请求线程中；不要在此执行配置重载 I/O，
// 由 Rust 读取 hook 刷新 g_fuse_fix_enabled。
bool ShouldAllowPrivateOwnerSqliteAccess(const std::string &path,
                                         uint32_t uid) {
  if (path.empty())
    return false;
  return srx_should_allow_fuse_private_owner_sqlite_access(path.c_str(), uid);
}

bool ShouldAllowPublicMappingAccess(const std::string &path, uint32_t uid) {
  if (path.empty())
    return false;
  return srx_should_allow_fuse_public_mapping_access(path.c_str(), uid);
}

bool ShouldAllowSrxAccessiblePath(const std::string &path, uint32_t uid) {
  return ShouldAllowPrivateOwnerSqliteAccess(path, uid) ||
         ShouldAllowPublicMappingAccess(path, uid);
}

bool ShouldForceUserspacePrivateOwnerSqlite(const std::string &path) {
  if (path.empty())
    return false;
  return srx_should_force_fuse_userspace_private_owner_sqlite(path.c_str());
}

void PreparePrivateOwnerSqliteSidecar(const std::string &path) {
  if (path.empty())
    return;
  srx_prepare_fuse_private_owner_sqlite_sidecar(path.c_str());
}

std::string ResolveFdPath(int fd) {
  if (fd < 0)
    return {};
  char link_path[64];
  snprintf(link_path, sizeof(link_path), "/proc/self/fd/%d", fd);
  char path[PATH_MAX + 1];
  ssize_t len = readlink(link_path, path, PATH_MAX);
  if (len <= 0)
    return {};
  path[len] = '\0';
  return std::string(path);
}

bool ShouldDisableFusePassthroughForFd(int fd, const char *source) {
  std::string path = ResolveFdPath(fd);
  if (!ShouldForceUserspacePrivateOwnerSqlite(path))
    return false;
  PreparePrivateOwnerSqliteSidecar(path);
  if (ShouldLogPassthroughVerbose()) {
    uint32_t suppressed =
        g_suppressed_passthrough_logs.exchange(0, std::memory_order_relaxed);
    LogInfoIfVerbose(
        "[RsInfo] disable fuse passthrough source=%s fd=%d path=%s suppressed=%u",
        source == nullptr ? "unknown" : source, fd, path.c_str(), suppressed);
  }
  return true;
}

int FdFromFuseFileInfo(const SrxFuseFileInfo *fi) {
  if (fi == nullptr || fi->fh == 0)
    return -1;
  auto handle = reinterpret_cast<const SrxMediaProviderHandle *>(
      static_cast<uintptr_t>(fi->fh));
  int fd = handle->fd;
  if (fd < 0)
    return -1;
  return fd;
}

bool ShouldClearFusePassthroughReply(const SrxFuseFileInfo *fi,
                                     const char *source) {
  if (fi == nullptr)
    return false;
  if (fi->passthrough_fh == 0 && fi->backing_id <= 0)
    return false;
  return ShouldDisableFusePassthroughForFd(FdFromFuseFileInfo(fi), source);
}

bool SrxFuseFixIsAppAccessiblePath(void *fuse, const std::string &path,
                                   uint32_t uid) {
  if (g_orig_is_app_accessible_path == nullptr)
    return false;
  bool enabled = g_fuse_fix_enabled.load(std::memory_order_relaxed);
  bool has_ignorable = HasDefaultIgnorableCodePoint(path);
  bool allowed = g_orig_is_app_accessible_path(fuse, path, uid);
  if (!enabled)
    return allowed || ShouldAllowSrxAccessiblePath(path, uid);
  if (has_ignorable) {
    std::string clean = RemoveDefaultIgnorableCodePoints(path);
    bool clean_allowed = g_orig_is_app_accessible_path(fuse, clean, uid);
    return allowed || clean_allowed || ShouldAllowSrxAccessiblePath(clean, uid) ||
           ShouldAllowSrxAccessiblePath(path, uid);
  }
  return allowed || ShouldAllowSrxAccessiblePath(path, uid);
}

bool SrxFuseFixIsPackageOwnedPath(const std::string &path,
                                  const std::string &fuse_path) {
  if (g_orig_is_package_owned_path == nullptr)
    return false;
  bool enabled = g_fuse_fix_enabled.load(std::memory_order_relaxed);
  bool has_ignorable = HasDefaultIgnorableCodePoint(path);
  if (!enabled)
    return g_orig_is_package_owned_path(path, fuse_path);
  if (has_ignorable) {
    std::string clean = RemoveDefaultIgnorableCodePoints(path);
    return g_orig_is_package_owned_path(clean, fuse_path);
  }
  return g_orig_is_package_owned_path(path, fuse_path);
}

bool SrxFuseFixIsBpfBackingPath(const std::string &path) {
  if (g_orig_is_bpf_backing_path == nullptr)
    return false;
  bool enabled = g_fuse_fix_enabled.load(std::memory_order_relaxed);
  bool has_ignorable = HasDefaultIgnorableCodePoint(path);
  if (!enabled)
    return g_orig_is_bpf_backing_path(path);
  if (has_ignorable) {
    std::string clean = RemoveDefaultIgnorableCodePoints(path);
    return g_orig_is_bpf_backing_path(clean);
  }
  return g_orig_is_bpf_backing_path(path);
}

bool SrxFuseFixShouldOpenWithFuse(void *daemon, int fd, bool for_read,
                                  const std::string &path) {
  if (g_orig_should_open_with_fuse == nullptr)
    return false;
  bool original = g_orig_should_open_with_fuse(daemon, fd, for_read, path);
  bool enabled = g_fuse_fix_enabled.load(std::memory_order_relaxed);
  if (original)
    return original;
  if (enabled && HasDefaultIgnorableCodePoint(path)) {
    std::string clean = RemoveDefaultIgnorableCodePoints(path);
    if (ShouldForceUserspacePrivateOwnerSqlite(clean))
      return true;
  }
  return ShouldForceUserspacePrivateOwnerSqlite(path);
}

int SrxFuseFixReplyOpen(void *req, const SrxFuseFileInfo *fi) {
  if (g_orig_fuse_reply_open == nullptr)
    return -ENOSYS;
  if (ShouldClearFusePassthroughReply(fi, "reply_open")) {
    SrxFuseFileInfo clean = *fi;
    clean.passthrough_fh = 0;
    clean.backing_id = 0;
    return g_orig_fuse_reply_open(req, &clean);
  }
  return g_orig_fuse_reply_open(req, fi);
}

int SrxFuseFixReplyCreate(void *req, const void *entry,
                          const SrxFuseFileInfo *fi) {
  if (g_orig_fuse_reply_create == nullptr)
    return -ENOSYS;
  if (ShouldClearFusePassthroughReply(fi, "reply_create")) {
    SrxFuseFileInfo clean = *fi;
    clean.passthrough_fh = 0;
    clean.backing_id = 0;
    return g_orig_fuse_reply_create(req, entry, &clean);
  }
  return g_orig_fuse_reply_create(req, entry, fi);
}

int SrxFuseFixPassthroughEnable(void *req, unsigned int fd) {
  if (g_orig_fuse_passthrough_enable == nullptr)
    return -ENOSYS;
  if ((g_fuse_fix_reply_open_stub != nullptr || g_fuse_fix_reply_create_stub != nullptr) &&
      ShouldDisableFusePassthroughForFd(static_cast<int>(fd), "passthrough_enable")) {
    return 1;
  }
  return g_orig_fuse_passthrough_enable(req, fd);
}

int SrxFuseFixPassthroughOpen(void *req, int fd) {
  if (g_orig_fuse_passthrough_open == nullptr)
    return -ENOSYS;
  if ((g_fuse_fix_reply_open_stub != nullptr || g_fuse_fix_reply_create_stub != nullptr) &&
      ShouldDisableFusePassthroughForFd(fd, "passthrough_open")) {
    return 1;
  }
  return g_orig_fuse_passthrough_open(req, fd);
}

int SrxFuseFixStrcasecmp(const char *lhs, const char *rhs) {
  if (lhs == nullptr || rhs == nullptr)
    return lhs == rhs ? 0 : (lhs == nullptr ? -1 : 1);
  if (!g_fuse_fix_enabled.load(std::memory_order_relaxed))
    return strcasecmp(lhs, rhs);
  return SrxStrcasecmpFix(lhs, rhs);
}

bool SrxFuseFixEqualsIgnoreCase(std::string_view lhs, std::string_view rhs) {
  if (!g_fuse_fix_enabled.load(std::memory_order_relaxed))
    return strcasecmp(std::string(lhs).c_str(), std::string(rhs).c_str()) == 0;
  return SrxStrcasecmpFix(lhs, rhs) == 0;
}

bool InstallFuseFixHook(void *target, void *replacement, void **orig,
                        void **stub_slot) {
  if (target == nullptr || replacement == nullptr || orig == nullptr ||
      stub_slot == nullptr)
    return false;
  if (*stub_slot != nullptr)
    return true;
  *stub_slot = srx_inline_hook_hook_func_addr(target, replacement, orig);
  return *stub_slot != nullptr && *orig != nullptr;
}

bool PatchFuseFixPltSlot(void *slot, void *replacement, void **orig,
                         void **stub_slot) {
  if (slot == nullptr || replacement == nullptr || orig == nullptr ||
      stub_slot == nullptr) {
    return false;
  }
  if (*stub_slot != nullptr) {
    return true;
  }

  auto slot_ptr = reinterpret_cast<void **>(slot);
  void *current = *slot_ptr;
  if (current == nullptr || current == replacement) {
    return false;
  }

  long page_size = sysconf(_SC_PAGESIZE);
  if (page_size <= 0) {
    return false;
  }
  uintptr_t page_start = reinterpret_cast<uintptr_t>(slot_ptr) &
                         ~(static_cast<uintptr_t>(page_size) - 1);
  if (mprotect(reinterpret_cast<void *>(page_start),
               static_cast<size_t>(page_size), PROT_READ | PROT_WRITE) != 0) {
    __android_log_print(
        ANDROID_LOG_WARN, "StorageRedirect",
        "[RsWarn] fuse fix plt mprotect rw failed slot=%p errno=%d", slot,
        errno);
    return false;
  }

  *orig = current;
  *slot_ptr = replacement;
  if (mprotect(reinterpret_cast<void *>(page_start),
               static_cast<size_t>(page_size), PROT_READ) != 0) {
    __android_log_print(
        ANDROID_LOG_WARN, "StorageRedirect",
        "[RsWarn] fuse fix plt mprotect ro failed slot=%p errno=%d", slot,
        errno);
  }
  *stub_slot = slot;
  return true;
}

void *ResolveDefaultSymbol(const char *name) {
  if (name == nullptr || name[0] == '\0')
    return nullptr;
  return dlsym(RTLD_DEFAULT, name);
}

int SrxFuseFixInstall(void *is_app_accessible_path, void *is_package_owned_path,
                      void *is_bpf_backing_path,
                      void *should_open_with_fuse, void *reply_open_slot,
                      void *reply_create_slot, void *passthrough_enable_slot,
                      void *passthrough_open_slot) {
  if (srx_inline_hook_init(SRX_INLINE_HOOK_MODE_UNIQUE, false) != 0)
    return -1;

  std::lock_guard lock(g_hook_mutex);
  int installed = 0;
  if (InstallFuseFixHook(
          is_app_accessible_path,
          reinterpret_cast<void *>(SrxFuseFixIsAppAccessiblePath),
          reinterpret_cast<void **>(&g_orig_is_app_accessible_path),
          &g_fuse_fix_is_app_accessible_stub)) {
    installed++;
  }
  if (InstallFuseFixHook(
          is_package_owned_path,
          reinterpret_cast<void *>(SrxFuseFixIsPackageOwnedPath),
          reinterpret_cast<void **>(&g_orig_is_package_owned_path),
          &g_fuse_fix_is_package_owned_stub)) {
    installed++;
  }
  if (InstallFuseFixHook(is_bpf_backing_path,
                         reinterpret_cast<void *>(SrxFuseFixIsBpfBackingPath),
                         reinterpret_cast<void **>(&g_orig_is_bpf_backing_path),
                         &g_fuse_fix_is_bpf_backing_stub)) {
    installed++;
  }
  if (InstallFuseFixHook(
          should_open_with_fuse,
          reinterpret_cast<void *>(SrxFuseFixShouldOpenWithFuse),
          reinterpret_cast<void **>(&g_orig_should_open_with_fuse),
          &g_fuse_fix_should_open_with_fuse_stub)) {
    installed++;
  }
  if (InstallFuseFixHook(ResolveDefaultSymbol("fuse_reply_open"),
                         reinterpret_cast<void *>(SrxFuseFixReplyOpen),
                         reinterpret_cast<void **>(&g_orig_fuse_reply_open),
                         &g_fuse_fix_reply_open_stub)) {
    installed++;
  } else if (PatchFuseFixPltSlot(
                 reply_open_slot, reinterpret_cast<void *>(SrxFuseFixReplyOpen),
                 reinterpret_cast<void **>(&g_orig_fuse_reply_open),
                 &g_fuse_fix_reply_open_stub)) {
    installed++;
  }
  if (InstallFuseFixHook(ResolveDefaultSymbol("fuse_reply_create"),
                         reinterpret_cast<void *>(SrxFuseFixReplyCreate),
                         reinterpret_cast<void **>(&g_orig_fuse_reply_create),
                         &g_fuse_fix_reply_create_stub)) {
    installed++;
  } else if (PatchFuseFixPltSlot(
                 reply_create_slot,
                 reinterpret_cast<void *>(SrxFuseFixReplyCreate),
                 reinterpret_cast<void **>(&g_orig_fuse_reply_create),
                 &g_fuse_fix_reply_create_stub)) {
    installed++;
  }
  if (InstallFuseFixHook(
          ResolveDefaultSymbol("fuse_passthrough_enable"),
          reinterpret_cast<void *>(SrxFuseFixPassthroughEnable),
          reinterpret_cast<void **>(&g_orig_fuse_passthrough_enable),
          &g_fuse_fix_passthrough_enable_stub)) {
    installed++;
  } else if (PatchFuseFixPltSlot(
                 passthrough_enable_slot,
                 reinterpret_cast<void *>(SrxFuseFixPassthroughEnable),
                 reinterpret_cast<void **>(&g_orig_fuse_passthrough_enable),
                 &g_fuse_fix_passthrough_enable_stub)) {
    installed++;
  }
  if (InstallFuseFixHook(
          ResolveDefaultSymbol("fuse_passthrough_open"),
          reinterpret_cast<void *>(SrxFuseFixPassthroughOpen),
          reinterpret_cast<void **>(&g_orig_fuse_passthrough_open),
          &g_fuse_fix_passthrough_open_stub)) {
    installed++;
  } else if (PatchFuseFixPltSlot(
                 passthrough_open_slot,
                 reinterpret_cast<void *>(SrxFuseFixPassthroughOpen),
                 reinterpret_cast<void **>(&g_orig_fuse_passthrough_open),
                 &g_fuse_fix_passthrough_open_stub)) {
    installed++;
  }
  LogInfoIfVerbose(
      "[RsInfo] fuse fix native hooks installed=%d app_accessible=%d package_owned=%d bpf_backing=%d should_open=%d reply_open=%d reply_create=%d passthrough_enable=%d passthrough_open=%d",
      installed, g_fuse_fix_is_app_accessible_stub != nullptr,
      g_fuse_fix_is_package_owned_stub != nullptr,
      g_fuse_fix_is_bpf_backing_stub != nullptr,
      g_fuse_fix_should_open_with_fuse_stub != nullptr,
      g_fuse_fix_reply_open_stub != nullptr,
      g_fuse_fix_reply_create_stub != nullptr,
      g_fuse_fix_passthrough_enable_stub != nullptr,
      g_fuse_fix_passthrough_open_stub != nullptr);
  return installed;
}

void *InlineHooker(void *target, void *hooker) {
  void *origin = nullptr;
  void *stub = srx_inline_hook_hook_func_addr(target, hooker, &origin);
  if (stub == nullptr || origin == nullptr)
    return nullptr;
  std::lock_guard lock(g_hook_mutex);
  g_hook_stubs[target] = stub;
  return origin;
}

bool InlineUnhooker(void *func) {
  if (func == nullptr)
    return false;
  void *stub = nullptr;
  {
    std::lock_guard lock(g_hook_mutex);
    auto it = g_hook_stubs.find(func);
    if (it == g_hook_stubs.end())
      return false;
    stub = it->second;
    g_hook_stubs.erase(it);
  }
  return srx_inline_hook_unhook(stub) == 0;
}

void *ResolveArtSymbol(std::string_view name) {
  if (name.empty())
    return nullptr;
  return srx_art_symbol_resolve(std::string{name}.c_str());
}

void *ResolveArtSymbolPrefix(std::string_view prefix) {
  if (prefix.empty())
    return nullptr;
  return srx_art_symbol_resolve_prefix(std::string{prefix}.c_str());
}

} // namespace

extern "C" int srx_fuse_fix_strcasecmp(const char *lhs, const char *rhs) {
  return SrxFuseFixStrcasecmp(lhs, rhs);
}

extern "C" void srx_fuse_fix_set_enabled(bool enabled) {
  g_fuse_fix_enabled.store(enabled, std::memory_order_relaxed);
}

extern "C" bool srx_fuse_fix_is_installed() {
  std::lock_guard lock(g_hook_mutex);
  return g_fuse_fix_is_app_accessible_stub != nullptr ||
         g_fuse_fix_is_package_owned_stub != nullptr ||
         g_fuse_fix_is_bpf_backing_stub != nullptr ||
         g_fuse_fix_should_open_with_fuse_stub != nullptr ||
         g_fuse_fix_reply_open_stub != nullptr ||
         g_fuse_fix_reply_create_stub != nullptr ||
         g_fuse_fix_passthrough_enable_stub != nullptr ||
         g_fuse_fix_passthrough_open_stub != nullptr;
}

extern "C" bool srx_fuse_fix_equals_ignore_case(std::string_view lhs,
                                                  std::string_view rhs) {
  return SrxFuseFixEqualsIgnoreCase(lhs, rhs);
}

extern "C" int srx_fuse_fix_install(void *is_app_accessible_path,
                                      void *is_package_owned_path,
                                      void *is_bpf_backing_path,
                                      void *should_open_with_fuse,
                                      void *reply_open_slot,
                                      void *reply_create_slot,
                                      void *passthrough_enable_slot,
                                      void *passthrough_open_slot) {
  return SrxFuseFixInstall(is_app_accessible_path, is_package_owned_path,
                           is_bpf_backing_path, should_open_with_fuse,
                           reply_open_slot, reply_create_slot,
                           passthrough_enable_slot, passthrough_open_slot);
}

extern "C" bool srx_lsplant_init(JNIEnv *env) {
  if (env == nullptr)
    return false;
  if (srx_inline_hook_init(SRX_INLINE_HOOK_MODE_UNIQUE, false) != 0)
    return false;

  lsplant::InitInfo info{
      .inline_hooker = InlineHooker,
      .inline_unhooker = InlineUnhooker,
      .art_symbol_resolver = ResolveArtSymbol,
      .art_symbol_prefix_resolver = ResolveArtSymbolPrefix,
      .generated_class_name = "SrxHooker_",
      .generated_source_name = "SRX",
      .generated_field_name = "hooker",
      .generated_method_name = "{target}",
  };
  return lsplant::Init(env, info);
}

extern "C" jobject srx_lsplant_hook(JNIEnv *env, jobject target_method,
                                    jobject hooker_object,
                                    jobject callback_method) {
  if (env == nullptr || target_method == nullptr || hooker_object == nullptr ||
      callback_method == nullptr) {
    return nullptr;
  }
  return lsplant::Hook(env, target_method, hooker_object, callback_method);
}

extern "C" bool srx_lsplant_unhook(JNIEnv *env, jobject target_method) {
  if (env == nullptr || target_method == nullptr)
    return false;
  return lsplant::UnHook(env, target_method);
}
