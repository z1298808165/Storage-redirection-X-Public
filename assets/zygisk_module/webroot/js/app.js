/**
 * SRX Core WebUI - Main Application
 */
(function () {
  "use strict";

  const State = {
    currentPage: "dashboard",
    currentApp: null,
    currentUserId: "0",
    configUserId: "0",
    templateEditor: null,
    appsLoaded: false,
    globalConfig: null,
    configuredApps: [],
    templates: [],
    selectedApps: new Set(),
    appSelectionMode: false,
    allApps: [],
    availableUsers: ["0"],
    moduleStatus: "unknown",
    compatAppListPreparedUsers: new Set(),
    appListScrollTop: 0,
    shouldRestoreAppListScroll: false,
    logListScrollTop: 0,
    shouldRestoreLogListScroll: false,
    appConfigOriginPage: "apps",
    modalHistoryActive: false,
    ignoreNextModalPopstate: false,
    modalCleanup: null,
    logEntries: [],
    logSearchQuery: "",
    logFullTime: false,
    monitorFilters: null,
    dashboardRequestId: 0,
    dashboardCountsRequestId: 0,
    dashboardLastCountsRefreshAt: 0,
    dashboardShown: false,
    dashboardCountsLoaded: false,
    runtimeActivationExact: "0",
    appListRequestId: 0,
    renderedAppListId: 0,
    autoTemplateFallbackNoticeId: "",
    appConfigSaveQueue: Promise.resolve(),
    monitorFilterSaveQueue: Promise.resolve(),
    isReturningFromConfig: false,
    isReturningFromConfigToLogs: false,
    appListDirty: false,
    readOnlyEditorKeys: new Set(),
    startupUpdateCheckDone: false,
    updateCheckRunning: false,
  };

  const LICENSES = [
    {
      group: "模块核心",
      name: "SRX Core",
      license: "GPL-3.0-or-later",
      url: "https://github.com/z1298808165/storage-redirect-x",
    },
    {
      group: "模块核心",
      name: "Storage-redirection-X-Public",
      license: "GPL-3.0-or-later",
      url: "https://github.com/Kindness-Kismet/Storage-redirection-X-Public",
    },
    {
      group: "模块核心",
      name: "srx_hook",
      license: "MIT",
      url: "https://github.com/Kindness-Kismet/srx_hook",
    },
    {
      group: "模块核心",
      name: "srx_inline_hook",
      license: "MIT",
      url: "https://github.com/Kindness-Kismet/srx_inline_hook",
    },
    {
      group: "模块核心",
      name: "fusefixer",
      license: "MIT",
      url: "https://github.com/MaterialCleaner/Media-Provider-FuseFixer",
    },
    {
      group: "Root / WebUI",
      name: "KernelSU",
      license: "GPL 3.0 / GPL 2.0",
      url: "https://github.com/tiann/KernelSU",
    },
    {
      group: "Hook 与 DEX",
      name: "LSPlant",
      license: "LGPL 3.0",
      url: "https://github.com/LSPosed/LSPlant",
    },
    {
      group: "Hook 与 DEX",
      name: "DexBuilder",
      license: "Apache 2.0",
      url: "https://android.googlesource.com/platform/tools/dexter",
    },
    {
      group: "Hook 与 DEX",
      name: "parallel-hashmap",
      license: "Apache 2.0",
      url: "https://github.com/greg7mdp/parallel-hashmap",
    },
    {
      group: "Hook 与 DEX",
      name: "abseil-cpp",
      license: "Apache 2.0",
      url: "https://github.com/abseil/abseil-cpp",
    },
    {
      group: "Rust 依赖",
      name: "libc",
      license: "MIT / Apache 2.0",
      url: "https://github.com/rust-lang/libc",
    },
    {
      group: "Rust 依赖",
      name: "jni-sys",
      license: "MIT / Apache 2.0",
      url: "https://github.com/jni-rs/jni-sys",
    },
    {
      group: "Rust 依赖",
      name: "serde",
      license: "MIT / Apache 2.0",
      url: "https://github.com/serde-rs/serde",
    },
    {
      group: "Rust 依赖",
      name: "serde_json",
      license: "MIT / Apache 2.0",
      url: "https://github.com/serde-rs/json",
    },
    {
      group: "Rust 依赖",
      name: "once_cell",
      license: "MIT / Apache 2.0",
      url: "https://github.com/matklad/once_cell",
    },
    {
      group: "Rust 依赖",
      name: "log",
      license: "MIT / Apache 2.0",
      url: "https://github.com/rust-lang/log",
    },
    {
      group: "APP / UI",
      name: "AndroidX / Jetpack Compose",
      license: "Apache 2.0",
      url: "https://github.com/androidx/androidx",
    },
    {
      group: "APP / UI",
      name: "Miuix",
      license: "Apache 2.0",
      url: "https://github.com/compose-miuix-ui/miuix",
    },
    {
      group: "APP / UI",
      name: "AppIconLoader",
      license: "Apache 2.0",
      url: "https://github.com/zhanghai/AppIconLoader",
    },
    {
      group: "APP / Kotlin",
      name: "kotlinx.coroutines",
      license: "Apache 2.0",
      url: "https://github.com/Kotlin/kotlinx.coroutines",
    },
    {
      group: "APP / Kotlin",
      name: "kotlinx.serialization",
      license: "Apache 2.0",
      url: "https://github.com/Kotlin/kotlinx.serialization",
    },
    {
      group: "APP / 系统兼容",
      name: "AndroidHiddenApiBypass",
      license: "Apache 2.0",
      url: "https://github.com/LSPosed/AndroidHiddenApiBypass",
    },
  ];

  const $ = (sel) => document.querySelector(sel);
  const $$ = (sel) => document.querySelectorAll(sel);
  const FILE_MONITOR_LOG = "/data/adb/modules/storage.redirect.x/logs/file_monitor.log";
  const DASHBOARD_REFRESH_THROTTLE_MS = 1200;
  const BACKUP_MAGIC = "storage.redirect.x.backup";
  const BACKUP_SCHEMA_VERSION = 2;
  const BACKUP_MODULE_ID = "storage.redirect.x";
  const BACKUP_FILE_SUFFIX = ".srxbak.zip";
  const BACKUP_ZIP_ENTRY = "backup.json";
  const BACKUP_MAX_BYTES = 8 * 1024 * 1024;
  const BACKUP_MAX_APPS = 3000;
  const BACKUP_STORAGE_BASE = "/storage/emulated/0";
  const BACKUP_DEFAULT_DIR = "Download";
  const DEFAULT_MONITOR_FILTER_OPERATIONS = [
    "attrib*",
    "chmod*",
    "delete*",
    "fchmod*",
    "ftruncate*",
    "futimens*",
    "link*",
    "open*:read",
    "open:read",
    "provider_open:read",
    "rename*",
    "rmdir*",
    "symlink*",
    "truncate*",
    "unlink*",
    "utimens*",
  ];
  const LEGACY_DEFAULT_MONITOR_FILTER_OPERATIONS = ["open:read", "rename*", "unlink*", "delete*"];
  const LEGACY_FULL_DEFAULT_MONITOR_FILTER_OPERATIONS = [
    "open:read",
    "open*:read",
    "rename*",
    "unlink*",
    "delete*",
    "rmdir*",
    "link*",
    "symlink*",
    "truncate*",
    "ftruncate*",
    "chmod*",
    "fchmod*",
    "utimens*",
    "futimens*",
    "attrib*",
  ];
  const PULL_REFRESH_TRIGGER_PX = 58;
  const PULL_REFRESH_MAX_PX = 82;
  const WEBUI_PAGE_SCALE_MIN = 0.8;
  const WEBUI_PAGE_SCALE_MAX = 1.1;
  const WEBUI_ACCENT_COLOR_OPTIONS = [
    { value: 0, label: "系统取色", color: "" },
    { value: 0xfff44336, label: "红色", color: "#F44336" },
    { value: 0xffe91e63, label: "粉色", color: "#E91E63" },
    { value: 0xff9c27b0, label: "紫色", color: "#9C27B0" },
    { value: 0xff673ab7, label: "深紫", color: "#673AB7" },
    { value: 0xff3f51b5, label: "靛蓝", color: "#3F51B5" },
    { value: 0xff2196f3, label: "蓝色", color: "#2196F3" },
    { value: 0xff00bcd4, label: "青色", color: "#00BCD4" },
    { value: 0xff009688, label: "水鸭绿", color: "#009688" },
    { value: 0xff4faf50, label: "绿色", color: "#4FAF50" },
    { value: 0xffffeb3b, label: "黄色", color: "#FFEB3B" },
    { value: 0xffffc107, label: "琥珀", color: "#FFC107" },
    { value: 0xffff9800, label: "橙色", color: "#FF9800" },
    { value: 0xff795548, label: "棕色", color: "#795548" },
    { value: 0xff607d8f, label: "蓝灰", color: "#607D8F" },
    { value: 0xffff9ca8, label: "樱花", color: "#FF9CA8" },
  ];
  const WEBUI_COLOR_STYLE_OPTIONS = [
    { value: "TonalSpot", label: "柔和" },
    { value: "Neutral", label: "中性" },
    { value: "Vibrant", label: "鲜艳" },
    { value: "Expressive", label: "表现" },
    { value: "Rainbow", label: "彩虹" },
    { value: "FruitSalad", label: "果蔬" },
    { value: "Monochrome", label: "单色" },
    { value: "Fidelity", label: "保真" },
    { value: "Content", label: "内容" },
  ];
  const WEBUI_COLOR_SPEC_OPTIONS = [
    { value: "Spec2025", label: "2025" },
    { value: "Spec2021", label: "2021" },
  ];
  const UPDATE_PREFS_KEY = "srx_update_prefs";
  const UPDATE_CHANNEL_OPTIONS = [
    { value: "Stable", label: "正式版" },
    { value: "Beta", label: "测试版" },
    { value: "All", label: "全通道最新版" },
  ];

  // ═══ Navigation ═══
  function routeTo(page, options) {
    if (!page) return;
    const opts = options || {};
    const samePage = State.currentPage === page;
    if (samePage && !opts.force) return;
    if (page === "apps" && !opts.preserveAppScroll) State.shouldRestoreAppListScroll = false;
    if (page === "logs" && !opts.preserveLogScroll) State.shouldRestoreLogListScroll = false;
    if (page !== "app-config") State.templateEditor = null;
    State.currentPage = page;
    const themeOptions = Object.assign({}, opts.themeOptions || {});
    if (page === "dashboard" && State.dashboardShown) themeOptions.noAnimation = true;
    Theme.navigateTo(page, themeOptions);
    loadPage(page);
    if (!opts.skipHistory) {
      if (opts.replaceHistory) replaceHistory(page, opts.historyState || {});
      else pushHistory(page, opts.historyState || {});
    }
  }

  function pushHistory(page, extra) {
    if (!window.history?.pushState) return;
    const state = Object.assign({ srxPage: page }, extra || {});
    window.history.pushState(state, "", "#/" + page);
  }

  function pushModalHistory() {
    if (!window.history?.pushState || State.modalHistoryActive) return;
    window.history.pushState(
      { srxPage: State.currentPage, modal: true },
      "",
      "#/" + State.currentPage + "/modal",
    );
    State.modalHistoryActive = true;
  }

  function closeActiveModal(options) {
    const modal = $("#modalOverlay");
    const wasOpen = modal && !modal.classList.contains("hidden");
    modal?.classList.add("hidden");
    Theme.releaseModalViewport?.();
    const dialog = $("#dialogOverlay");
    if (!dialog || dialog.classList.contains("hidden")) {
      document.body.classList.remove("modal-open");
    }
    State.modalCleanup?.();
    State.modalCleanup = null;
    if (State.modalHistoryActive) {
      State.modalHistoryActive = false;
      if (!options?.fromPopstate && window.history?.back) {
        State.ignoreNextModalPopstate = true;
        window.history.back();
      }
    }
    return !!wasOpen;
  }

  function showModalWithHistory(contentHtml, options) {
    const modal = Theme.showModal(contentHtml, { disableBackdropClose: true });
    pushModalHistory();
    const overlay = $("#modalOverlay");
    State.modalCleanup?.();
    State.modalCleanup = null;
    if (options?.backdropClose && overlay) {
      const closeOnBackdrop = (event) => {
        if (event.target === overlay) closeActiveModal();
      };
      overlay.addEventListener("click", closeOnBackdrop);
      State.modalCleanup = () => overlay.removeEventListener("click", closeOnBackdrop);
    }
    return modal;
  }

  function focusWithoutViewportJump(input) {
    if (!input) return;
    try {
      input.focus({ preventScroll: true });
    } catch {
      input.focus();
    }
  }

  function replaceHistory(page, extra) {
    if (!window.history?.replaceState) return;
    const state = Object.assign({ srxPage: page }, extra || {});
    window.history.replaceState(state, "", "#/" + page);
  }

  function setupHistory() {
    replaceHistory(State.currentPage);
    window.addEventListener("popstate", (event) => {
      if (State.ignoreNextModalPopstate) {
        State.ignoreNextModalPopstate = false;
        return;
      }
      if (State.modalHistoryActive) {
        closeActiveModal({ fromPopstate: true });
        return;
      }
      const page = event.state?.srxPage || "dashboard";
      if (!document.getElementById("page-" + page)) return;
      if (page === "app-config" && event.state?.templateEditor && event.state?.templateId) {
        const template = State.templates.find((item) => item.id === event.state.templateId);
        openTemplateEditor(
          template || {
            id: event.state.templateId,
            name: "",
            config: { users: { [String(State.currentUserId || "0")]: createDefaultProfile() } },
          },
          {
            skipHistory: true,
            originPage: event.state.originPage || "settings",
            sourcePackage: event.state.sourcePackage || null,
          },
        );
        return;
      }
      if (page === "app-config" && event.state?.packageName) {
        openAppConfig(event.state.packageName, {
          skipHistory: true,
          originPage: event.state.originPage || "apps",
        });
        return;
      }
      const returningFromConfig = State.currentPage === "app-config";
      const returningToApps = returningFromConfig && page === "apps";
      const returningToLogs = returningFromConfig && page === "logs";
      if (returningToApps) {
        State.shouldRestoreAppListScroll = true;
        State.isReturningFromConfig = true;
        primeAppListScrollPosition();
      }
      if (returningToLogs) {
        State.shouldRestoreLogListScroll = true;
        State.isReturningFromConfigToLogs = true;
        primeLogListScrollPosition();
      }
      if (page !== "app-config") State.templateEditor = null;
      State.currentPage = page;
      if (page !== "app-config") State.currentApp = null;
      Theme.navigateTo(page, {
        preserveScroll: page === "apps" || page === "logs",
        noAnimation:
          returningToApps || returningToLogs || (page === "dashboard" && State.dashboardShown),
      });
      loadPage(page);
    });
  }

  function returnToAppListFromConfig(options) {
    State.shouldRestoreAppListScroll = true;
    State.isReturningFromConfig = true;
    primeAppListScrollPosition();
    routeTo("apps", {
      preserveAppScroll: true,
      replaceHistory: !!options?.replaceHistory,
      themeOptions: { preserveScroll: true, noAnimation: true },
    });
  }

  function returnToLogListFromConfig(options) {
    State.shouldRestoreLogListScroll = true;
    State.isReturningFromConfigToLogs = true;
    primeLogListScrollPosition();
    routeTo("logs", {
      preserveLogScroll: true,
      replaceHistory: !!options?.replaceHistory,
      themeOptions: { preserveScroll: true, noAnimation: true },
    });
  }

  function isTemplateEditorActive() {
    return !!State.templateEditor;
  }

  function returnFromAppConfig(options) {
    if (isTemplateEditorActive()) {
      closeTemplateEditor(options);
      return;
    }
    if (State.appConfigOriginPage === "logs") {
      returnToLogListFromConfig(options);
      return;
    }
    returnToAppListFromConfig(options);
  }

  function closeTemplateEditor(options) {
    State.templateEditor = null;
    State.currentApp = null;
    routeTo("settings", {
      replaceHistory: !!options?.replaceHistory,
      themeOptions: { preserveScroll: true, noAnimation: true },
    });
  }

  function openTemplateEditor(template, options) {
    const source = template || {};
    const users = source.config?.users
      ? normalizeUsersConfig(source.config.users)
      : { [String(State.currentUserId || "0")]: createDefaultProfile() };
    State.templateEditor = {
      id: isSafeTemplateId(source.id) ? String(source.id) : createTemplateId(),
      name: normalizeTemplateName(source.name || ""),
      originPage: options?.originPage || "settings",
      sourcePackage: options?.sourcePackage || null,
    };
    State.currentApp = null;
    State.currentPage = "app-config";
    State.configUserId = State.currentUserId;
    Theme.navigateTo("app-config", options?.themeOptions || {});
    if (!options?.skipHistory) {
      pushHistory("app-config", {
        templateEditor: true,
        templateId: State.templateEditor.id,
        originPage: State.templateEditor.originPage,
        sourcePackage: State.templateEditor.sourcePackage,
      });
    }
    $("#configAppName").textContent = State.templateEditor.name || "\u914d\u7f6e\u6a21\u677f";
    renderAppConfig("__template__", { users }, { mode: "template" });
  }

  function setScrollerTopInstant(scroller, top) {
    if (!scroller) return;
    const prevInlineBehavior = scroller.style.scrollBehavior;
    scroller.style.scrollBehavior = "auto";
    scroller.scrollTop = top || 0;
    if (typeof scroller.scrollTo === "function") {
      try {
        scroller.scrollTo({ top: top || 0, behavior: "instant" });
      } catch {
        scroller.scrollTo({ top: top || 0, behavior: "auto" });
      }
    }
    requestAnimationFrame(() => {
      scroller.scrollTop = top || 0;
      if (typeof scroller.scrollTo === "function") {
        try {
          scroller.scrollTo({ top: top || 0, behavior: "instant" });
        } catch {
          scroller.scrollTo({ top: top || 0, behavior: "auto" });
        }
      }
      requestAnimationFrame(() => {
        scroller.style.scrollBehavior = prevInlineBehavior;
      });
    });
  }

  function getAppListScroller() {
    return document.getElementById("appList");
  }

  function primeAppListScrollPosition() {
    const scroller = getAppListScroller();
    setScrollerTopInstant(scroller, State.appListScrollTop || 0);
  }

  function getLogListScroller() {
    return document.getElementById("logViewer");
  }

  function primeLogListScrollPosition() {
    const scroller = getLogListScroller();
    setScrollerTopInstant(scroller, State.logListScrollTop || 0);
  }

  function restoreLogListScrollIfNeeded() {
    if (!State.shouldRestoreLogListScroll) return;
    const scroller = getLogListScroller();
    if (!scroller) return;
    setScrollerTopInstant(scroller, State.logListScrollTop || 0);
  }

  function initPullRefresh(containerId, scrollerId, onRefresh) {
    const container = document.getElementById(containerId);
    const scroller = document.getElementById(scrollerId);
    if (!container || !scroller || container._srxPullRefreshBound) return;
    container._srxPullRefreshBound = true;
    const text = container.querySelector(".pull-refresh-text");
    const state = {
      active: false,
      pulling: false,
      refreshing: false,
      startY: 0,
      lastOffset: 0,
    };
    const setText = (value) => {
      if (text) text.textContent = value;
    };
    const setOffset = (offset) => {
      const next = Math.max(0, Math.min(PULL_REFRESH_MAX_PX, offset));
      state.lastOffset = next;
      container.style.setProperty("--pull-refresh-offset", next.toFixed(1) + "px");
      container.style.setProperty("--pull-refresh-spin", (next * 4).toFixed(1) + "deg");
      container.classList.toggle("is-pulling", next > 0 && !state.refreshing);
      if (!state.refreshing) setText(next >= PULL_REFRESH_TRIGGER_PX ? "释放刷新" : "下拉刷新");
    };
    const finish = () => {
      state.refreshing = false;
      state.pulling = false;
      state.active = false;
      container.classList.add("is-settling");
      container.classList.remove("is-refreshing", "is-pulling");
      setOffset(0);
      setTimeout(() => container.classList.remove("is-settling"), 280);
    };
    const refresh = async () => {
      if (state.refreshing) return;
      state.refreshing = true;
      state.pulling = false;
      container.classList.add("is-refreshing");
      container.classList.remove("is-pulling");
      container.style.setProperty("--pull-refresh-offset", "48px");
      setText("正在刷新");
      let failed = false;
      try {
        await onRefresh();
      } catch {
        failed = true;
      } finally {
        setText(failed ? "刷新失败" : "刷新完成");
        setTimeout(finish, 220);
      }
    };
    scroller.addEventListener(
      "touchstart",
      (event) => {
        if (state.refreshing || event.touches.length !== 1 || scroller.scrollTop > 0) return;
        state.active = true;
        state.pulling = false;
        state.startY = event.touches[0].clientY;
        state.lastOffset = 0;
      },
      { passive: true },
    );
    scroller.addEventListener(
      "touchmove",
      (event) => {
        if (!state.active || state.refreshing || event.touches.length !== 1) return;
        const delta = event.touches[0].clientY - state.startY;
        if (delta <= 0 || scroller.scrollTop > 0) {
          if (state.pulling) setOffset(0);
          state.pulling = false;
          return;
        }
        const offset = Math.min(PULL_REFRESH_MAX_PX, Math.pow(delta, 0.82) * 1.42);
        if (offset < 6 && !state.pulling) return;
        state.pulling = true;
        setOffset(offset);
        event.preventDefault();
      },
      { passive: false },
    );
    const endPull = () => {
      if (!state.active || state.refreshing) return;
      const shouldRefresh = state.pulling && state.lastOffset >= PULL_REFRESH_TRIGGER_PX;
      state.active = false;
      if (shouldRefresh) refresh();
      else {
        state.pulling = false;
        container.classList.add("is-settling");
        setOffset(0);
        setTimeout(() => container.classList.remove("is-settling"), 280);
      }
    };
    scroller.addEventListener("touchend", endPull, { passive: true });
    scroller.addEventListener("touchcancel", endPull, { passive: true });
  }

  function initPullRefreshControls() {
    initPullRefresh("appPullRefresh", "appList", () => loadAppList(true, { pullRefresh: true }));
    initPullRefresh("logPullRefresh", "logViewer", () => loadLogs({ pullRefresh: true }));
  }

  function navigateFromNav(page) {
    if (!page) return;
    if (State.currentPage === page) {
      if (page === "dashboard") refreshDashboardCounts({ force: true });
      return;
    }
    routeTo(page);
  }

  function initNav() {
    $$(".nav-item").forEach((item) => {
      item.addEventListener("click", () => {
        const nav = document.getElementById("bottomNav");
        if (nav?.classList.contains("dragging") || Date.now() < (nav?._suppressClickUntil || 0))
          return;
        navigateFromNav(item.dataset.page);
      });
    });
    $("#appConfigBack")?.addEventListener("click", () => {
      returnFromAppConfig({ replaceHistory: true });
    });
    $("#aboutBack")?.addEventListener("click", () => {
      routeTo("dashboard", { replaceHistory: true });
    });
    $("#updateBack")?.addEventListener("click", () => {
      routeTo("dashboard", { replaceHistory: true });
    });
    $("#themeBack")?.addEventListener("click", () => {
      routeTo("settings", { replaceHistory: true });
    });
  }

  function loadPage(page) {
    switch (page) {
      case "dashboard":
        loadDashboard();
        break;
      case "apps":
        loadAppList(false);
        break;
      case "settings":
        loadSettings();
        break;
      case "theme":
        renderThemePage();
        break;
      case "logs":
        loadLogs();
        break;
      case "about":
        loadAbout();
        break;
      case "update":
        loadUpdate();
        break;
    }
  }

  // ═══ Dashboard ═══
  async function loadDashboard() {
    if (!State.dashboardShown) {
      State.dashboardShown = true;
      const card = document.querySelector(".module-status-card");
      const settleCard = () => card?.classList.add("is-settled");
      card?.addEventListener("animationend", settleCard, { once: true });
      setTimeout(settleCard, 360);
    }
    renderQuickActions();
    renderDashboardPlaceholders();
    refreshDashboardCounts({ force: true });
    const requestId = ++State.dashboardRequestId;
    const requestPage = State.currentPage;
    try {
      const [globalConfig, status, version] = await Promise.all([
        Api.readGlobalConfig(),
        Api.getModuleStatus(),
        Api.getModuleVersion(),
      ]);
      if (State.currentPage !== requestPage || State.dashboardRequestId !== requestId) return;
      State.globalConfig = globalConfig;
      State.moduleStatus = status;
      $("#moduleVersionDisplay").textContent = version || "--";
      setFeatureChipState(
        "monitorStatusChip",
        !!State.globalConfig.file_monitor_enabled,
        "文件监控",
      );
      setFeatureChipState(
        "fuseFixStatusChip",
        State.globalConfig.fuse_fix_enabled !== false,
        "FuseFixer",
      );
      setFeatureChipState(
        "verboseLogStatusChip",
        State.globalConfig.verbose_logging_enabled === true,
        "详细日志",
      );
      updatePowerButton(status);
      maybeRunStartupUpdateCheck(version);
    } catch {
      Theme.showToast("加载状态失败", "error");
    }
  }

  function refreshDashboardCountsIfVisible(options) {
    if (State.currentPage !== "dashboard" || document.hidden) return;
    const now = Date.now();
    if (!options?.force && now - State.dashboardLastCountsRefreshAt < DASHBOARD_REFRESH_THROTTLE_MS)
      return;
    refreshDashboardCounts(options);
  }

  function refreshDashboardCounts(options) {
    if (State.currentPage !== "dashboard") return;
    const requestPage = State.currentPage;
    const requestId = ++State.dashboardCountsRequestId;
    State.dashboardLastCountsRefreshAt = Date.now();
    const showInitialLoading = !State.dashboardCountsLoaded;
    if (showInitialLoading) {
      setDashboardCountLoading("statAppCount", true);
      setDashboardCountLoading("statEnabledAppCount", true);
    }
    const run = async () => {
      try {
        await Api.ensureLogCollectors?.();
        const [configuredConfigs, runtimeActivations] = await Promise.all([
          Api.readConfiguredAppConfigs({ force: true }),
          Api.readStatsCount(),
        ]);
        if (State.currentPage !== requestPage || State.dashboardCountsRequestId !== requestId)
          return;
        const counts = countConfiguredProfiles(configuredConfigs);
        State.dashboardCountsLoaded = true;
        State.runtimeActivationExact = runtimeActivations;
        setDashboardCountValue("statAppCount", counts.enabled);
        setDashboardCountValue(
          "statEnabledAppCount",
          Api.formatCompactRuntimeActivationCount(runtimeActivations),
          runtimeActivations,
        );
        setDashboardCountLoading("statAppCount", false);
        setDashboardCountLoading("statEnabledAppCount", false);
      } catch {
        if (State.currentPage === requestPage && State.dashboardCountsRequestId === requestId) {
          if (!State.dashboardCountsLoaded) {
            setDashboardCountValue("statAppCount", "--");
            setDashboardCountValue("statEnabledAppCount", "--");
          }
          setDashboardCountLoading("statAppCount", false);
          setDashboardCountLoading("statEnabledAppCount", false);
        }
      }
    };
    if ("requestIdleCallback" in window) window.requestIdleCallback(run, { timeout: 900 });
    else setTimeout(run, 32);
  }

  function setDashboardCountLoading(id, loading) {
    const el = document.getElementById(id);
    if (!el) return;
    el.classList.toggle("is-loading", !!loading);
    el.setAttribute("aria-busy", loading ? "true" : "false");
  }

  function setDashboardCountValue(id, value, exactValue) {
    const el = document.getElementById(id);
    if (!el) return;
    const text = String(value);
    el.textContent = text;
    if (exactValue != null && String(exactValue) !== text) el.title = String(exactValue);
    else el.removeAttribute("title");
  }

  function showRuntimeActivationDetails() {
    const exactValue = State.runtimeActivationExact || "0";
    showModalWithHistory(
      '<div class="modal-title">生效次数</div>' +
        '<div class="runtime-stats-exact">' +
        escapeHtml(exactValue) +
        "</div>" +
        '<div class="modal-actions"><button class="btn btn-primary modal-close" type="button">关闭</button></div>',
      { backdropClose: true },
    );
    document.querySelector(".modal-close")?.addEventListener("click", () => closeActiveModal());
  }

  function showResetRuntimeStatsDialog() {
    showModalWithHistory(
      '<div class="modal-title">清除生效次数</div>' +
        '<div class="modal-hint runtime-stats-reset-hint">清除当前生效次数并从 0 重新统计？此操作不会修改应用配置或重定向状态。</div>' +
        '<div class="modal-actions"><button class="btn btn-secondary modal-close" type="button">取消</button><button class="btn btn-primary" id="resetRuntimeStatsConfirm" type="button">清除</button></div>',
    );
    document.querySelector(".modal-close")?.addEventListener("click", () => closeActiveModal());
    document.getElementById("resetRuntimeStatsConfirm")?.addEventListener("click", async () => {
      const button = document.getElementById("resetRuntimeStatsConfirm");
      if (button) button.disabled = true;
      try {
        await Api.resetRuntimeStats();
        State.runtimeActivationExact = "0";
        setDashboardCountValue("statEnabledAppCount", "0", "0");
        closeActiveModal();
        Theme.showToast("生效次数已清零", "success");
        refreshDashboardCounts({ force: true });
      } catch {
        if (button) button.disabled = false;
        Theme.showToast("生效次数清零失败", "error");
      }
    });
  }

  function initRuntimeActivationInteractions() {
    const metric = document.getElementById("runtimeActivationMetric");
    if (!metric) return;
    let timer = null;
    let longPressed = false;
    let startX = 0;
    let startY = 0;
    const cancel = () => {
      clearTimeout(timer);
      timer = null;
    };
    metric.addEventListener("pointerdown", (event) => {
      if (event.pointerType === "mouse" && event.button !== 0) return;
      longPressed = false;
      startX = event.clientX;
      startY = event.clientY;
      cancel();
      timer = setTimeout(() => {
        longPressed = true;
        showResetRuntimeStatsDialog();
      }, 480);
    });
    metric.addEventListener("pointermove", (event) => {
      if (Math.abs(event.clientX - startX) > 8 || Math.abs(event.clientY - startY) > 8) cancel();
    });
    ["pointerup", "pointercancel", "pointerleave"].forEach((name) =>
      metric.addEventListener(name, cancel),
    );
    metric.addEventListener("click", (event) => {
      if (longPressed) {
        event.preventDefault();
        longPressed = false;
        return;
      }
      showRuntimeActivationDetails();
    });
    metric.addEventListener("contextmenu", (event) => event.preventDefault());
    metric.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        showRuntimeActivationDetails();
      }
    });
  }

  function renderDashboardPlaceholders() {
    if (!$("#statAppCount")?.textContent || $("#statAppCount")?.textContent === "--")
      setDashboardCountValue("statAppCount", "0");
    if (!$("#statEnabledAppCount")?.textContent || $("#statEnabledAppCount")?.textContent === "--")
      setDashboardCountValue("statEnabledAppCount", "0");
    if (
      !$("#moduleVersionDisplay")?.textContent ||
      $("#moduleVersionDisplay")?.textContent === "--"
    )
      $("#moduleVersionDisplay").textContent = "...";
  }

  function renderQuickActions() {
    const actions = document.getElementById("quickActions");
    if (!actions || actions.dataset.ready === "1") return;
    actions.dataset.ready = "1";
    const items = [
      {
        icon: "refresh",
        label: "重启 MediaProvider",
        description: "让系统写入进程重新加载当前配置",
        action: restartMediaProviderWithLoading,
      },
      { icon: "download", label: "检查更新", description: "查看模块的新版本", page: "update" },
      { icon: "file", label: "关于与开源协议", description: "版本信息与依赖许可", page: "about" },
    ];
    items.forEach((item) => {
      const el = document.createElement("button");
      el.className = "action-item";
      el.innerHTML =
        '<span class="action-item-icon">' +
        iconHtml(item.icon) +
        '</span><span class="action-item-copy"><span class="action-item-label">' +
        escapeHtml(item.label) +
        '</span><span class="action-item-description">' +
        escapeHtml(item.description) +
        '</span></span><span class="action-item-arrow" aria-hidden="true">›</span>';
      el.addEventListener("click", () => {
        if (item.page) routeTo(item.page);
        else if (item.action) item.action();
      });
      actions.appendChild(el);
    });
  }

  function countConfiguredProfiles(configs) {
    const items = Array.isArray(configs) ? configs : [];
    let enabled = 0;
    items.forEach((item) => {
      const cfg = item && item.config ? item.config : item;
      const users = (cfg && cfg.users) || {};
      if (Object.values(users).some((profile) => profile && profile.enabled === true)) enabled += 1;
    });
    return { configured: items.length, enabled };
  }

  function setFeatureChipState(id, enabled, label, stateText) {
    const chip = document.getElementById(id);
    if (!chip) return;
    chip.classList.toggle("is-active", !!enabled);
    chip.classList.toggle("is-inactive", !enabled);
    const finalStateText = stateText || (enabled ? "已开启" : "已关闭");
    const labelEl = Array.from(chip.children || []).find(
      (el) => !el.classList.contains("status-dot"),
    );
    if (labelEl) labelEl.textContent = label;
    chip.title = label + "：" + finalStateText;
    chip.setAttribute("aria-label", label + "：" + finalStateText);
  }

  function modulePowerLabel(status) {
    if (status === "enabled") return "模块已激活";
    if (status === "disabled") return "模块已停用";
    if (status === "reboot_required") return "需要重启";
    return "状态未知";
  }

  function updatePowerButton(status) {
    const btn = $("#modulePowerBtn");
    const card = $(".module-status-card");
    if (!btn) return;
    const enabled = status === "enabled";
    const canToggle = status === "enabled" || status === "disabled";
    btn.textContent = modulePowerLabel(status);
    btn.disabled = !canToggle;
    btn.classList.toggle("is-active", enabled);
    btn.classList.toggle("is-stopped", status === "disabled");
    btn.classList.toggle("is-warning", status === "reboot_required" || status === "unknown");
    btn.title = canToggle ? (enabled ? "点击停止模块" : "点击启动模块") : modulePowerLabel(status);
    btn.setAttribute(
      "aria-label",
      canToggle ? modulePowerLabel(status) + "，" + btn.title : modulePowerLabel(status),
    );
    card?.classList.toggle("is-stopped", !enabled);
    if (!canToggle) {
      btn.onclick = null;
      return;
    }
    btn.onclick = async () => {
      const confirmed = await confirmAction(
        enabled
          ? "停止模块会结束已配置应用进程和 MediaProvider 进程，让已安装的 hook 退出当前进程。KernelSU、Magisk、APatch 管理器等受保护应用会被跳过。是否继续？"
          : "启动模块会重启已配置应用进程和 MediaProvider 进程，让新进程按当前配置重新安装 hook。KernelSU、Magisk、APatch 管理器等受保护应用会被跳过。是否继续？",
      );
      if (!confirmed) return;
      const nextStatus = enabled ? "disabled" : "enabled";
      btn.disabled = true;
      btn.textContent = enabled ? "停止中..." : "启动中...";
      btn.classList.add("is-busy");
      State.moduleStatus = nextStatus;
      try {
        await toggleModuleRuntimeWithLoading(enabled);
        const verifiedStatus = await Api.getModuleStatus();
        State.moduleStatus = verifiedStatus;
        if (!enabled && verifiedStatus === "reboot_required") {
          Api.showManagerToast(
            "\u6a21\u5757\u5df2\u542f\u7528\uff0c\u91cd\u542f\u8bbe\u5907\u540e\u751f\u6548",
          );
        } else {
          Api.showManagerToast(
            enabled ? "模块已停止，MediaProvider 已重启" : "模块已启动，MediaProvider 已重启",
          );
        }
        updatePowerButton(verifiedStatus);
      } catch (e) {
        Theme.showToast((enabled ? "停止" : "启动") + "失败", "error");
        State.moduleStatus = status;
        updatePowerButton(status);
      } finally {
        btn.classList.remove("is-busy");
      }
    };
  }

  async function toggleModuleRuntimeWithLoading(isStopping) {
    const loading = Theme.showLoadingDialog(
      isStopping ? "正在停止模块并重启 MediaProvider..." : "正在启动模块并重启 MediaProvider...",
    );
    try {
      const ok = isStopping ? await Api.stopModule() : await Api.startModule();
      if (!ok) throw new Error("MediaProvider restart timeout");
    } finally {
      loading.close();
    }
  }

  function confirmAction(message) {
    return new Promise((resolve) => {
      Theme.showDialog(
        message,
        () => resolve(true),
        () => resolve(false),
      );
    });
  }

  // ═══ App List ═══
  let appListCache = { user: [], system: [] };
  let appListCacheUser = "0";
  let currentFilter = "all";
  const appStatusOrder = { enabled: 0, disabled: 1, unconfigured: 2 };

  function compareAppsByStatus(a, b) {
    const diff =
      (appStatusOrder[a.status] ?? appStatusOrder.unconfigured) -
      (appStatusOrder[b.status] ?? appStatusOrder.unconfigured);
    return diff || a.package.localeCompare(b.package);
  }

  function sortAppCache() {
    appListCache.user = [...(appListCache.user || [])].sort(compareAppsByStatus);
    appListCache.system = [...(appListCache.system || [])].sort(compareAppsByStatus);
  }

  function yieldToUi() {
    return new Promise((resolve) => setTimeout(resolve, 0));
  }

  async function buildAppListBuckets(installed, configuredState, requestId) {
    const userApps = [];
    const systemApps = [];
    for (let i = 0; i < installed.length; i += 1) {
      if (requestId !== State.appListRequestId) return null;
      const pkg = installed[i];
      const info = Api.getCachedAppInfo ? Api.getCachedAppInfo(pkg) : null;
      const isSystem =
        pkg.startsWith("com.android.") ||
        pkg.startsWith("com.google.android.") ||
        pkg.startsWith("android.");
      const label = info ? info.appLabel || info.label || info.name || "" : "";
      const status = configuredState.get(pkg) || "unconfigured";
      const app = {
        package: pkg,
        configured: status !== "unconfigured",
        enabled: status === "enabled",
        status,
        label,
        iconSrc: Api.getAppIconSrc ? Api.getAppIconSrc(pkg) : "",
        isSystem: info?.isSystem ?? isSystem,
      };
      (app.isSystem ? systemApps : userApps).push(app);
      if (i > 0 && i % 120 === 0) await yieldToUi();
    }
    return { userApps, systemApps };
  }

  async function loadAppList(force, options) {
    const pullRefresh = !!options?.pullRefresh;
    // 新增：如果刚从配置页退出，且列表已经有内容，则直接跳过整个重新加载和渲染的过程！
    if (State.isReturningFromConfig && State.appsLoaded && $("#appList")?.children.length > 0) {
      State.isReturningFromConfig = false;
      renderAppUserSwitcher();
      if (State.appListDirty) {
        State.appListDirty = false;
        applyFilters();
      } else {
        restoreAppListScrollIfNeeded();
        State.shouldRestoreAppListScroll = false;
      }
      return;
    }
    State.isReturningFromConfig = false;
    State.appListDirty = false;

    const listEl = $("#appList");
    const requestId = ++State.appListRequestId;
    const userKey = String(State.currentUserId || "0");
    const hasCache =
      appListCacheUser === userKey &&
      ((appListCache.user && appListCache.user.length) ||
        (appListCache.system && appListCache.system.length));
    if (!force && State.appsLoaded && hasCache) {
      renderAppUserSwitcher();
      applyFilters();
      return;
    }
    if (!pullRefresh) listEl.classList.add("is-loading");
    listEl.setAttribute("aria-busy", "true");
    if (hasCache) {
      applyFilters();
    } else if (!pullRefresh) {
      listEl.innerHTML = renderAppLoadingState();
    }
    try {
      const [configuredConfigs, users] = await Promise.all([
        Api.readConfiguredAppConfigs(),
        Api.listUsers(),
      ]);
      if (requestId !== State.appListRequestId) return;
      const configured = configuredConfigs.map((item) => item.packageName);
      const configuredState = buildConfiguredState(configuredConfigs, State.currentUserId);
      State.availableUsers = Array.from(
        new Set([...(users || ["0"]), ...State.availableUsers, State.currentUserId]),
      ).sort((a, b) => Number(a) - Number(b));
      if (!State.availableUsers.includes(State.currentUserId))
        State.currentUserId = State.availableUsers[0] || "0";
      const installed = await Api.getInstalledApps(State.currentUserId);
      if (requestId !== State.appListRequestId) return;
      State.allApps = installed;
      State.configuredApps = configured;
      const buckets = await buildAppListBuckets(installed, configuredState, requestId);
      if (!buckets || requestId !== State.appListRequestId) return;
      appListCache = { user: buckets.userApps, system: buckets.systemApps };
      sortAppCache();
      appListCacheUser = String(State.currentUserId || "0");
      State.appsLoaded = true;
      renderAppUserSwitcher();
      applyFilters();
      mergeConfiguredUsers(configuredConfigs).then(() => {
        if (requestId !== State.appListRequestId || State.currentPage !== "apps") return;
        renderAppUserSwitcher();
      });
    } catch {
      if (pullRefresh) {
        Theme.showToast("刷新应用列表失败", "error");
        throw new Error("refresh failed");
      } else {
        listEl.innerHTML = '<div class="app-empty">加载失败，请重试</div>';
      }
    } finally {
      listEl.classList.remove("is-loading");
      listEl.setAttribute("aria-busy", "false");
    }
  }

  function renderAppLoadingState() {
    return (
      '<div class="app-loading-list">' +
      Array.from(
        { length: 6 },
        (_, i) =>
          '<div class="app-item skeleton" aria-hidden="true" style="animation-delay:' +
          i * 28 +
          'ms">' +
          '<div class="app-icon skeleton-block"></div>' +
          '<div class="app-info"><div class="skeleton-line skeleton-title"></div><div class="skeleton-line skeleton-subtitle"></div></div>' +
          '<div class="app-status skeleton-pill"></div>' +
          "</div>",
      ).join("") +
      "</div>"
    );
  }

  async function mergeConfiguredUsers(configured) {
    const ids = new Set(State.availableUsers || ["0"]);
    (configured || []).forEach((item) => {
      const cfg = item && item.config ? item.config : item;
      Object.keys((cfg && cfg.users) || {}).forEach((id) => ids.add(id));
    });
    State.availableUsers = Array.from(ids).sort((a, b) => Number(a) - Number(b));
  }

  function buildConfiguredState(configured, userId) {
    const map = new Map();
    (configured || []).forEach((item) => {
      if (!item || !item.packageName) return;
      const cfg = item.config || {};
      const users = cfg.users || {};
      const profile = users[String(userId || "0")];
      const legacyEnabled = Object.prototype.hasOwnProperty.call(cfg, "enabled")
        ? cfg.enabled === true
        : false;
      map.set(
        item.packageName,
        (profile ? profile.enabled === true : legacyEnabled) ? "enabled" : "disabled",
      );
    });
    return map;
  }

  function renderAppUserSwitcher() {
    const switcher = $("#appUserSwitcher");
    if (!switcher) return;
    const users = State.availableUsers || ["0"];
    switcher.classList.toggle("visible", users.length > 1);
    switcher.classList.remove("open");
    if (users.length <= 1) {
      switcher.innerHTML = "";
      return;
    }
    switcher.innerHTML =
      '<button class="app-user-trigger" id="appUserTrigger" aria-haspopup="menu" aria-expanded="false">U' +
      escapeHtml(State.currentUserId) +
      "</button>" +
      '<div class="app-user-menu" role="menu">' +
      users
        .map(
          (id) =>
            '<button class="app-user-chip' +
            (id === State.currentUserId ? " active" : "") +
            '" data-user="' +
            escapeHtml(id) +
            '" role="menuitem">用户 ' +
            escapeHtml(id) +
            "</button>",
        )
        .join("") +
      "</div>";
    switcher.querySelector("#appUserTrigger")?.addEventListener("click", (e) => {
      e.stopPropagation();
      switcher.classList.toggle("open");
      switcher
        .querySelector("#appUserTrigger")
        ?.setAttribute("aria-expanded", switcher.classList.contains("open") ? "true" : "false");
    });
    switcher.querySelectorAll(".app-user-chip").forEach((chip) => {
      chip.addEventListener("click", () => {
        State.currentUserId = chip.dataset.user;
        switcher.classList.remove("open");
        renderAppUserSwitcher();
        Theme.showToast("已切换到用户 " + State.currentUserId);
        loadAppList(true);
      });
    });
  }

  function applyFilters() {
    const query = ($("#appSearchInput")?.value || "").toLowerCase();
    let apps = [];
    if (currentFilter === "configured")
      apps = [...appListCache.user, ...appListCache.system]
        .filter((a) => a.configured)
        .sort(compareAppsByStatus);
    else if (currentFilter === "system") apps = appListCache.system || [];
    else apps = appListCache.user || [];
    if (query)
      apps = apps.filter(
        (a) =>
          a.package.toLowerCase().includes(query) ||
          (a.label && a.label.toLowerCase().includes(query)),
      );
    renderAppList(apps);
  }

  function renderAppList(apps) {
    const listEl = $("#appList");
    const renderId = ++State.renderedAppListId;
    listEl.innerHTML = "";
    if (apps.length === 0) {
      listEl.innerHTML = '<div class="app-empty">没有找到应用</div>';
      State.shouldRestoreAppListScroll = false;
      exitAppSelectionMode();
      return;
    }
    const frag = document.createDocumentFragment();
    const appendChunk = (start) => {
      if (renderId !== State.renderedAppListId) return;
      const end = Math.min(start + 36, apps.length);
      for (let i = start; i < end; i += 1) {
        frag.appendChild(createAppItem(apps[i]));
      }
      listEl.appendChild(frag);
      restoreAppListScrollIfNeeded();
      if (end < apps.length) requestAnimationFrame(() => appendChunk(end));
      else if (State.shouldRestoreAppListScroll)
        requestAnimationFrame(() => {
          restoreAppListScrollIfNeeded();
          State.shouldRestoreAppListScroll = false;
        });
    };
    appendChunk(0);
  }

  function restoreAppListScrollIfNeeded() {
    if (!State.shouldRestoreAppListScroll) return;
    const scroller = getAppListScroller();
    if (!scroller) return;
    setScrollerTopInstant(scroller, State.appListScrollTop || 0);
  }

  function createAppItem(app) {
    const item = document.createElement("div");
    const selected = State.selectedApps?.has(app.package);
    item.className =
      "app-item" +
      (State.appSelectionMode ? " selection-mode" : "") +
      (selected ? " selected" : "");
    item.dataset.pkg = app.package;
    const initial = (app.label || app.package)[0].toUpperCase();
    const iconHtml = app.iconSrc
      ? '<div class="app-icon has-image" data-initial="' +
        escapeHtml(initial) +
        '"><img class="app-icon-img" src="' +
        escapeHtml(app.iconSrc) +
        '" alt="" loading="lazy" onerror="const icon=this.closest(\'.app-icon\'); if (icon) icon.classList.add(\'fallback\'); this.remove();"></div>'
      : '<div class="app-icon fallback" data-initial="' + escapeHtml(initial) + '"></div>';
    item.innerHTML =
      iconHtml +
      '<div class="app-info"><div class="app-name">' +
      escapeHtml(app.label || app.package) +
      '</div><div class="app-package">' +
      app.package +
      "</div></div>" +
      (State.appSelectionMode
        ? '<span class="app-select-mark"><span class="icon icon-check" aria-hidden="true"></span></span>'
        : '<span class="app-status ' +
          statusClass(app.status) +
          '">' +
          statusLabel(app.status) +
          "</span>");
    let longPressTimer = null;
    let longPressed = false;
    let longPressStartX = 0;
    let longPressStartY = 0;
    const startLongPress = () => {
      longPressed = false;
      clearTimeout(longPressTimer);
      longPressTimer = setTimeout(() => {
        longPressed = true;
        enterAppSelectionMode(app.package);
      }, 480);
    };
    const clearLongPress = () => clearTimeout(longPressTimer);
    item.addEventListener("pointerdown", (event) => {
      longPressStartX = event.clientX;
      longPressStartY = event.clientY;
      startLongPress();
    });
    item.addEventListener("pointermove", (event) => {
      if (
        Math.abs(event.clientX - longPressStartX) > 8 ||
        Math.abs(event.clientY - longPressStartY) > 8
      ) {
        clearLongPress();
      }
    });
    item.addEventListener("pointerup", clearLongPress);
    item.addEventListener("pointercancel", clearLongPress);
    item.addEventListener("pointerleave", clearLongPress);
    item.addEventListener("click", (event) => {
      if (longPressed) {
        event.preventDefault();
        return;
      }
      if (State.appSelectionMode) toggleAppSelection(app.package);
      else openAppConfig(app.package);
    });
    return item;
  }

  function enterAppSelectionMode(packageName) {
    State.appSelectionMode = true;
    State.selectedApps = new Set([packageName]);
    document.body.classList.add("app-selection-active");
    applyFilters();
    renderBatchActionBar();
  }

  function toggleAppSelection(packageName) {
    if (!State.selectedApps) State.selectedApps = new Set();
    if (State.selectedApps.has(packageName)) State.selectedApps.delete(packageName);
    else State.selectedApps.add(packageName);
    if (!State.selectedApps.size) exitAppSelectionMode({ refreshList: true });
    else {
      applyFilters();
      renderBatchActionBar();
    }
  }

  function exitAppSelectionMode(options) {
    State.appSelectionMode = false;
    State.selectedApps = new Set();
    document.body.classList.remove("app-selection-active");
    document.getElementById("appBatchBar")?.remove();
    if (options?.refreshList) applyFilters();
  }

  function currentFilteredAppPackages() {
    const query = ($("#appSearchInput")?.value || "").toLowerCase();
    let apps = [];
    if (currentFilter === "configured")
      apps = [...appListCache.user, ...appListCache.system]
        .filter((a) => a.configured)
        .sort(compareAppsByStatus);
    else if (currentFilter === "system") apps = appListCache.system || [];
    else apps = appListCache.user || [];
    if (query)
      apps = apps.filter(
        (a) =>
          a.package.toLowerCase().includes(query) ||
          (a.label && a.label.toLowerCase().includes(query)),
      );
    return apps.map((app) => app.package);
  }

  function renderBatchActionBar() {
    let bar = document.getElementById("appBatchBar");
    if (!State.appSelectionMode || !State.selectedApps?.size) {
      bar?.remove();
      return;
    }
    if (!bar) {
      bar = document.createElement("div");
      bar.id = "appBatchBar";
      bar.className = "app-batch-bar visible miuix-floating-container miuix-blur";
      bar.dataset.miuixUi = "floating-container";
      bar.dataset.miuixBlur = "realtime";
      document.body.appendChild(bar);
    }
    bar.innerHTML =
      '<div class="app-batch-status" aria-label="\u5df2\u9009 ' +
      State.selectedApps.size +
      ' \u4e2a\u5e94\u7528"><span class="app-batch-status-count">' +
      State.selectedApps.size +
      '</span><span class="app-batch-status-label">\u5df2\u9009</span></div>' +
      '<button class="app-batch-item app-batch-primary" id="batchApplyTemplate" type="button"><span class="nav-icon icon icon-template" aria-hidden="true"></span><span class="nav-label">\u6a21\u677f</span></button>' +
      '<button class="app-batch-item" id="batchSelectAll" type="button"><span class="nav-icon icon icon-select-all" aria-hidden="true"></span><span class="nav-label">\u5168\u9009</span></button>' +
      '<button class="app-batch-item app-batch-danger" id="batchCancel" type="button"><span class="nav-icon icon icon-x" aria-hidden="true"></span><span class="nav-label">\u53d6\u6d88</span></button>';
    bar.querySelector("#batchApplyTemplate")?.addEventListener("click", () =>
      showTemplatePickerDialog("\u9009\u62e9\u914d\u7f6e\u6a21\u677f", async (template) => {
        const count = State.selectedApps.size;
        const confirmed = await confirmAction(
          "\u5c06\u6a21\u677f\u201c" +
            template.name +
            "\u201d\u5e94\u7528\u5230 " +
            count +
            " \u4e2a\u5e94\u7528\uff0c\u73b0\u6709\u914d\u7f6e\u4f1a\u88ab\u8986\u76d6\u3002\u662f\u5426\u7ee7\u7eed\uff1f",
        );
        if (!confirmed) return;
        await applyTemplateToPackages(template, Array.from(State.selectedApps));
        exitAppSelectionMode({ refreshList: true });
      }),
    );
    bar.querySelector("#batchSelectAll")?.addEventListener("click", () => {
      State.selectedApps = new Set(currentFilteredAppPackages());
      applyFilters();
      renderBatchActionBar();
    });
    bar
      .querySelector("#batchCancel")
      ?.addEventListener("click", () => exitAppSelectionMode({ refreshList: true }));
  }

  function statusClass(status) {
    return status === "enabled" ? "enabled" : status === "disabled" ? "disabled" : "unconfigured";
  }
  function statusLabel(status) {
    return status === "enabled" ? "已启用" : status === "disabled" ? "未启用" : "未配置";
  }

  function updateCachedAppConfigured(packageName, configured, enabled) {
    const status = configured ? (enabled ? "enabled" : "disabled") : "unconfigured";
    State.appListDirty = true;
    ["user", "system"].forEach((group) => {
      (appListCache[group] || []).forEach((app) => {
        if (app.package === packageName) {
          app.configured = configured;
          app.enabled = enabled === true;
          app.status = status;
        }
      });
    });
    if (configured && !State.configuredApps.includes(packageName))
      State.configuredApps.push(packageName);
    if (!configured)
      State.configuredApps = State.configuredApps.filter((pkg) => pkg !== packageName);
    sortAppCache();

    // 新增：局部更新 DOM，这样返回时不用重绘列表也能体现刚才开启/关闭的状态
    const itemDom = document.querySelector('.app-item[data-pkg="' + packageName + '"]');
    if (itemDom) {
      const statusDom = itemDom.querySelector(".app-status");
      if (statusDom) {
        statusDom.className = "app-status " + statusClass(status);
        statusDom.textContent = statusLabel(status);
      }
    }
  }

  function initSearch() {
    const input = $("#appSearchInput"),
      clearBtn = $("#appSearchClear");
    let timer = null;
    document.addEventListener("click", (e) => {
      const switcher = $("#appUserSwitcher");
      if (switcher && !switcher.contains(e.target)) switcher.classList.remove("open");
    });

    input?.addEventListener("input", () => {
      clearBtn.classList.toggle("hidden", !input.value);
      clearTimeout(timer);
      // 优化：将搜索防抖延迟从 200ms 提高到 400ms，增强应用列表渲染时的操作流畅度
      timer = setTimeout(() => applyFilters(), 400);
    });

    clearBtn?.addEventListener("click", () => {
      input.value = "";
      clearBtn.classList.add("hidden");
      applyFilters();
      input.focus();
    });

    $$(".filter-chip").forEach((chip) => {
      chip.addEventListener("click", () => {
        $$(".filter-chip").forEach((c) => c.classList.remove("active"));
        chip.classList.add("active");
        currentFilter = chip.dataset.filter;
        Theme.updateFilterIndicator();
        applyFilters();
      });
    });
  }

  function initLogSearch() {
    const input = $("#logSearchInput"),
      clearBtn = $("#logSearchClear");
    if (!input || input._srxBound) return;
    input._srxBound = true;
    let timer = null;
    input.addEventListener("input", () => {
      State.logSearchQuery = input.value.trim().toLowerCase();
      clearBtn?.classList.toggle("hidden", !input.value);
      clearTimeout(timer);
      timer = setTimeout(renderLogCards, 120);
    });
    clearBtn?.addEventListener("click", () => {
      input.value = "";
      State.logSearchQuery = "";
      clearBtn.classList.add("hidden");
      renderLogCards();
      input.focus();
    });
  }

  // ═══ App Config ═══
  async function openAppConfig(packageName, options) {
    const opts = options || {};
    const originPage = opts.originPage || (State.currentPage === "logs" ? "logs" : "apps");
    State.templateEditor = null;
    State.appConfigOriginPage = originPage;
    State.shouldRestoreAppListScroll = false;
    State.shouldRestoreLogListScroll = false;
    if (originPage === "logs") {
      const scroller = getLogListScroller();
      if (scroller) State.logListScrollTop = scroller.scrollTop;
    } else {
      const scroller = getAppListScroller();
      if (scroller) State.appListScrollTop = scroller.scrollTop;
    }
    State.currentApp = packageName;
    State.currentPage = "app-config";
    Theme.navigateTo("app-config");
    if (!opts.skipHistory) pushHistory("app-config", { packageName, originPage });
    State.configUserId = State.currentUserId;
    bindAppConfigActions(packageName, null, true);
    $("#configAppName").textContent = packageName.split(".").pop();
    const content = $("#appConfigContent");
    content.innerHTML =
      '<div class="loading-state"><div class="spinner"></div><span>加载配置...</span></div>';
    try {
      const [label, config, globalConfig] = await Promise.all([
        Api.getAppLabel(packageName).catch(() => ""),
        Api.readAppConfig(packageName),
        Api.readGlobalConfig().catch(() => null),
      ]);
      if (globalConfig) State.globalConfig = globalConfig;
      if (label && label !== packageName) $("#configAppName").textContent = label;
      renderAppConfig(packageName, config);
    } catch {
      bindAppConfigActions(packageName, null, true);
      content.innerHTML = '<div class="app-empty">加载配置失败</div>';
    }
  }

  function renderAppConfig(packageName, config, options) {
    const content = $("#appConfigContent");
    const isTemplateMode = options?.mode === "template" || isTemplateEditorActive();
    const users = config && config.users ? normalizeUsersConfig(config.users) : {};
    const savedUserId = State.configUserId || State.currentUserId;
    if (!users[savedUserId]) users[savedUserId] = createDefaultProfile();
    const profile = users[savedUserId];
    const userIds = Object.keys(users).sort();
    let html = "";
    if (isTemplateMode) {
      html +=
        '<div class="config-group template-edit-meta"><div class="switch-row"><div class="switch-label-group"><div class="switch-label">\u6a21\u677f\u540d\u79f0</div><input type="text" class="modal-input template-name-edit" id="templateNameEdit" maxlength="48" autocomplete="off" value="' +
        escapeHtml(State.templateEditor?.name || "") +
        '"></div></div></div>';
    }
    html += '<div class="user-tabs">';
    userIds.forEach((id) => {
      html +=
        '<button class="user-tab' +
        (id === savedUserId ? " active" : "") +
        '" data-user="' +
        id +
        '">用户 ' +
        id +
        "</button>";
    });
    html += "</div>";

    html += '<div class="config-group">';
    html += switchRow(
      "启用重定向",
      "enable",
      profile.enabled === true,
      "开启后将对此应用执行存储重定向",
    );
    html += switchRow(
      "仅映射模式",
      "mappingOnly",
      !!profile.mapping_mode_only,
      "仅应用显式配置的路径映射，不执行完整隔离重定向",
    );
    const readOnlyEditorKey = appReadOnlyEditorKey(packageName);
    const readOnlyEditorEnabled =
      (profile.read_only_paths || []).length > 0 || State.readOnlyEditorKeys.has(readOnlyEditorKey);
    html += switchRow(
      "只读模式",
      "readOnly",
      readOnlyEditorEnabled,
      "禁止写入指定真实目录；默认方案会退化通配规则，FUSE daemon 可精确匹配",
    );
    html += "</div>";

    // Allowed paths, including ! exclude rules.
    const allowRules = getAllowedRules(profile);
    html +=
      '<div class="config-group"><div class="config-group-header"><span class="config-group-title">允许路径</span><button class="icon-btn icon-btn-sm icon-btn-add add-allow-btn" type="button" aria-label="添加允许路径" title="添加允许路径">' +
      iconHtml("plus") +
      '</button></div><div class="path-list">';
    allowRules.forEach((p) => {
      html += pathItem(p, p.startsWith("!"), "allow");
    });
    if (!allowRules.length)
      html +=
        '<div class="app-empty" style="padding:16px;font-size:12px">允许路径可直接访问；! 可排除子路径，* 和 ? 在默认方案下会退化匹配</div>';
    html += "</div></div>";

    if (readOnlyEditorEnabled) {
      html +=
        '<div class="config-group"><div class="config-group-header"><span class="config-group-title">只读路径</span><button class="icon-btn icon-btn-sm icon-btn-add add-readonly-btn" type="button" aria-label="添加只读路径" title="添加只读路径">' +
        iconHtml("plus") +
        '</button></div><div class="path-list">';
      (profile.read_only_paths || []).forEach((p) => {
        html += pathItem(p, p.startsWith("!"), "readonly");
      });
      if (!(profile.read_only_paths || []).length)
        html +=
          '<div class="app-empty" style="padding:16px;font-size:12px">只读路径保持可读但禁止写入；可用 ! 排除子路径，默认方案会退化通配</div>';
      html += "</div></div>";
    }

    // Sandbox paths (mapping only)
    if (!!profile.mapping_mode_only) {
      html +=
        '<div class="config-group config-sandbox-section"><div class="config-group-header"><span class="config-group-title">沙盒路径</span><button class="icon-btn icon-btn-sm icon-btn-add add-sandbox-btn" type="button" aria-label="添加沙盒路径" title="添加沙盒路径">' +
        iconHtml("plus") +
        '</button></div><div class="path-list">';
      (profile.sandboxed_paths || []).forEach((p) => {
        html += pathItem(p, false, "sandbox");
      });
      if (!(profile.sandboxed_paths || []).length)
        html +=
          '<div class="app-empty" style="padding:16px;font-size:12px">仅映射模式下，未命中映射且匹配沙盒路径时将进入应用沙盒</div>';
      html += "</div></div>";
    }

    // Mappings
    html +=
      '<div class="config-group"><div class="config-group-header"><span class="config-group-title">路径映射</span><button class="icon-btn icon-btn-sm icon-btn-add add-mapping-btn" type="button" aria-label="添加路径映射" title="添加路径映射">' +
      iconHtml("plus") +
      '</button></div><div class="path-list">';
    const mappings = profile.path_mappings || {};
    const entries =
      typeof mappings === "object" && !Array.isArray(mappings)
        ? Object.entries(mappings)
        : Array.isArray(mappings)
          ? mappings.map((m) => [m.request_path, m.final_path])
          : [];
    entries.forEach(([req, target]) => {
      html += mappingItem(req, target);
    });
    if (!entries.length)
      html +=
        '<div class="app-empty" style="padding:16px;font-size:12px">将请求路径映射到目标路径</div>';
    html += "</div></div>";

    content.innerHTML = html;
    bindConfigEvents(packageName, users);
  }

  function switchRow(label, key, checked, hint) {
    return (
      '<div class="switch-row"><div class="switch-label-group"><div class="switch-label">' +
      escapeHtml(label) +
      "</div>" +
      (hint ? '<div class="switch-hint">' + escapeHtml(hint) + "</div>" : "") +
      '</div><button class="toggle ' +
      (checked ? "on" : "") +
      '" data-key="' +
      key +
      '" type="button" role="switch" aria-checked="' +
      String(checked) +
      '" aria-label="' +
      escapeHtml(label) +
      '"></button></div>'
    );
  }
  function setToggleState(toggle, enabled) {
    if (!toggle) return;
    toggle.classList.toggle("on", !!enabled);
    toggle.setAttribute("aria-checked", String(!!enabled));
  }

  function setToggleBusy(toggle, busy) {
    if (!toggle) return;
    toggle.disabled = !!busy;
    toggle.setAttribute("aria-busy", String(!!busy));
  }
  function optionLabel(options, value, fallback) {
    const match = options.find((item) => item.value === value);
    return match ? match.label : fallback || String(value ?? "");
  }
  function accentColorLabel(value) {
    return optionLabel(WEBUI_ACCENT_COLOR_OPTIONS, Number(value) || 0, "自定义");
  }
  function accentColorHex(value) {
    const item = WEBUI_ACCENT_COLOR_OPTIONS.find((option) => option.value === (Number(value) || 0));
    return item?.color || "";
  }
  function hexToRgba(hex, alpha) {
    const normalized = String(hex || "")
      .replace("#", "")
      .trim();
    if (!/^[0-9a-fA-F]{6}$/.test(normalized)) return "";
    const value = Number.parseInt(normalized, 16);
    return (
      "rgba(" +
      ((value >> 16) & 255) +
      "," +
      ((value >> 8) & 255) +
      "," +
      (value & 255) +
      "," +
      alpha +
      ")"
    );
  }
  function accentSwatchHtml(value) {
    const color = accentColorHex(value);
    const style = color
      ? ' style="--accent-icon:' + color + ";--accent-icon-bg:" + hexToRgba(color, 0.14) + '"'
      : "";
    return (
      '<span class="setting-accent-pen' +
      (color ? "" : " default") +
      '"' +
      style +
      '><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4.5 19.5l3.8-.9L18.9 8l-2.9-2.9L5.4 15.7l-.9 3.8z"></path><path d="M14.9 6.2l2.9 2.9"></path><path d="M5.4 15.7l2.9 2.9"></path><path d="M17 4.1l1.1-1.1a1.4 1.4 0 0 1 2 0l.9.9a1.4 1.4 0 0 1 0 2l-1.1 1.1"></path></svg></span>'
    );
  }
  function clampWebUiPageScale(value) {
    const scale = Number(value);
    return Number.isFinite(scale)
      ? Math.max(WEBUI_PAGE_SCALE_MIN, Math.min(WEBUI_PAGE_SCALE_MAX, scale))
      : 1;
  }
  function webUiPageScalePercent(value) {
    return Math.round(clampWebUiPageScale(value) * 100);
  }
  function webUiPageScaleLabel(value) {
    return webUiPageScalePercent(value) + "%";
  }
  function settingSelectRow(label, key, value, hint, leadingHtml) {
    return (
      '<button class="setting-select-row" type="button" data-key="' +
      key +
      '">' +
      (leadingHtml || "") +
      '<span class="switch-label-group"><span class="switch-label">' +
      label +
      "</span>" +
      (hint ? '<span class="switch-hint">' + hint + "</span>" : "") +
      "</span>" +
      '<span class="setting-select-value">' +
      escapeHtml(value) +
      "</span>" +
      '<span class="auto-template-chevron" aria-hidden="true"><span class="icon icon-chevron-down"></span></span>' +
      "</button>"
    );
  }
  function showSettingOptionDialog(title, options, selected, onPick) {
    const rows = options
      .map((option) => {
        const value = String(option.value);
        const selectedClass = option.value === selected ? " selected" : "";
        const swatch = Object.prototype.hasOwnProperty.call(option, "color")
          ? accentSwatchHtml(option.value)
          : "";
        return (
          '<button class="setting-option-row' +
          selectedClass +
          '" type="button" data-value="' +
          escapeHtml(value) +
          '">' +
          swatch +
          '<span class="setting-option-label">' +
          escapeHtml(option.label) +
          "</span>" +
          (option.value === selected ? '<span class="auto-template-selected">✓</span>' : "") +
          "</button>"
        );
      })
      .join("");
    showModalWithHistory(
      '<div class="modal-title">' +
        escapeHtml(title) +
        "</div>" +
        '<div class="setting-option-list">' +
        rows +
        "</div>",
      { backdropClose: true },
    );
    document.querySelectorAll("#modalContent .setting-option-row").forEach((row) => {
      row.addEventListener("click", () => {
        const match = options.find((option) => String(option.value) === row.dataset.value);
        if (!match) return;
        closeActiveModal();
        onPick(match.value);
      });
    });
  }

  function showPageScaleDialog() {
    const currentPercent = webUiPageScalePercent(Theme.getUiOption("pageScale") || 1);
    showModalWithHistory(
      '<div class="page-scale-dialog">' +
        '<div class="modal-title">界面缩放</div>' +
        '<div class="page-scale-dialog-range">80% - 110%</div>' +
        '<label class="page-scale-input-wrap" for="pageScaleInput">' +
        '<input class="modal-input page-scale-input" id="pageScaleInput" type="text" inputmode="numeric" maxlength="3" autocomplete="off" value="' +
        currentPercent +
        '"><span aria-hidden="true">%</span></label>' +
        '<div class="modal-hint page-scale-dialog-hint" id="pageScaleHint">输入 80 到 110 之间的整数</div>' +
        '<div class="modal-actions"><button class="btn btn-secondary modal-close" type="button">取消</button><button class="btn btn-primary" id="pageScaleConfirm" type="button">确定</button></div>' +
        "</div>",
      { backdropClose: true },
    );
    const overlay = document.getElementById("modalOverlay");
    const input = document.getElementById("pageScaleInput");
    const hint = document.getElementById("pageScaleHint");
    document.body.classList.add("scale-modal-open");
    overlay?.classList.add("scale-modal-overlay");

    const cleanupScaleModal = () => {
      document.body.classList.remove("scale-modal-open");
      overlay?.classList.remove("scale-modal-overlay");
    };
    const closeScaleModal = () => {
      cleanupScaleModal();
      closeActiveModal();
    };
    const readPercent = () => {
      const value = Number.parseInt(input?.value || "", 10);
      const valid = Number.isFinite(value) && value >= 80 && value <= 110;
      input?.classList.toggle("invalid", !valid);
      hint?.classList.toggle("error", !valid);
      if (hint) hint.textContent = valid ? "输入 80 到 110 之间的整数" : "请输入 80 - 110";
      return valid ? value : null;
    };

    input?.addEventListener("input", () => {
      input.value = input.value.replace(/[^\d]/g, "").slice(0, 3);
      readPercent();
    });
    input?.addEventListener("keydown", (event) => {
      if (event.key === "Enter") document.getElementById("pageScaleConfirm")?.click();
    });
    document.querySelector("#pageScaleConfirm")?.addEventListener("click", () => {
      const percent = readPercent();
      if (percent == null) {
        input?.focus();
        return;
      }
      Theme.setUiOption("pageScale", percent / 100);
      closeScaleModal();
      if (State.currentPage === "theme") renderThemePage();
    });
    document.querySelector(".modal-close")?.addEventListener("click", closeScaleModal);

    const previousCleanup = State.modalCleanup;
    State.modalCleanup = () => {
      previousCleanup?.();
      cleanupScaleModal();
    };
    focusWithoutViewportJump(input);
    input?.select();
  }
  function normalizeGlobalRuntimeConfig(raw) {
    const source = raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
    return {
      file_monitor_enabled: source.file_monitor_enabled === true,
      fuse_fix_enabled: source.fuse_fix_enabled !== false,
      fuse_daemon_redirect_enabled: source.fuse_daemon_redirect_enabled === true,
      verbose_logging_enabled: source.verbose_logging_enabled === true,
      auto_enable_redirect_for_new_apps: source.auto_enable_redirect_for_new_apps === true,
      auto_enable_new_apps_template_id: isSafeTemplateId(source.auto_enable_new_apps_template_id)
        ? String(source.auto_enable_new_apps_template_id)
        : "",
      app_config_auto_save: source.app_config_auto_save === true,
    };
  }
  function autoRedirectTemplateStatusHtml(globalConfig, templates, fallbackNoticeId) {
    if (globalConfig?.auto_enable_redirect_for_new_apps !== true) return "";
    const templateId = isSafeTemplateId(globalConfig.auto_enable_new_apps_template_id)
      ? String(globalConfig.auto_enable_new_apps_template_id)
      : "";
    const template = templateId ? (templates || []).find((item) => item.id === templateId) : null;
    const missingTemplate = !!fallbackNoticeId || (!!templateId && !template);
    const hint = template
      ? "使用模板：" + template.name
      : missingTemplate
        ? "模板已失效，已回退为仅开启重定向"
        : "仅开启重定向，无其它规则配置";
    return (
      '<button class="auto-template-row' +
      (missingTemplate ? " is-missing" : "") +
      '" id="autoTemplateRow" type="button">' +
      '<span class="auto-template-icon"><span class="icon icon-template" aria-hidden="true"></span></span>' +
      '<span class="auto-template-main"><span class="auto-template-title">自动配置模板</span><span class="auto-template-hint">' +
      escapeHtml(hint) +
      "</span></span>" +
      '<span class="auto-template-chevron" aria-hidden="true"><span class="icon icon-chevron-down"></span></span>' +
      "</button>"
    );
  }
  function refreshAutoTemplateStatus(content) {
    const existing = content.querySelector("#autoTemplateRow");
    const html = autoRedirectTemplateStatusHtml(
      State.globalConfig,
      State.templates,
      State.autoTemplateFallbackNoticeId,
    );
    if (existing && !html) {
      existing.remove();
      return;
    }
    if (existing) {
      existing.outerHTML = html;
    } else if (html) {
      content
        .querySelector('.toggle[data-key="autoEnableNewApps"]')
        ?.closest(".switch-row")
        ?.insertAdjacentHTML("afterend", html);
    }
    bindAutoTemplateStatus(content);
  }

  function bindAutoTemplateStatus(content) {
    content.querySelector("#autoTemplateRow")?.addEventListener("click", () => {
      showAutoTemplatePickerDialog(
        State.globalConfig?.auto_enable_new_apps_template_id || "",
        async (templateId) => {
          try {
            await saveGlobalConfigFromSettings(
              content,
              {
                auto_enable_redirect_for_new_apps: true,
                auto_enable_new_apps_template_id: templateId,
              },
              { silent: true },
            );
            refreshAutoTemplateStatus(content);
          } catch {
            Theme.showToast("保存失败，已恢复原状态", "error");
          }
        },
      ).catch(() => Theme.showToast("加载模板失败", "error"));
    });
  }
  function autoTemplateDefaultRowHtml(selected) {
    return (
      '<div class="template-row auto-template-default' +
      (selected ? " selected" : "") +
      '" data-template-id=""><span class="template-row-icon"><span class="icon icon-check" aria-hidden="true"></span></span><div class="template-row-main"><div class="template-row-title">仅开启重定向</div><div class="template-row-subtitle">不附加允许路径、沙盒路径或映射规则</div></div>' +
      (selected ? '<span class="auto-template-selected">✓</span>' : "") +
      "</div>"
    );
  }
  function autoTemplateChoiceRowHtml(template, selected) {
    return (
      '<div class="template-row' +
      (selected ? " selected" : "") +
      '" data-template-id="' +
      escapeHtml(template.id) +
      '"><span class="template-row-icon"><span class="icon icon-template" aria-hidden="true"></span></span><div class="template-row-main"><div class="template-row-title">' +
      escapeHtml(template.name) +
      '</div><div class="template-row-subtitle">' +
      escapeHtml(templateSummary(template.config)) +
      "</div></div>" +
      (selected ? '<span class="auto-template-selected">✓</span>' : "") +
      "</div>"
    );
  }
  async function showAutoTemplatePickerDialog(currentTemplateId, onPick, onDismiss) {
    const templates = await loadTemplates(true);
    const safeCurrent = isSafeTemplateId(currentTemplateId) ? String(currentTemplateId) : "";
    const currentExists =
      !!safeCurrent && templates.some((template) => template.id === safeCurrent);
    const rows = [
      autoTemplateDefaultRowHtml(!safeCurrent || !currentExists),
      ...templates.map((template) =>
        autoTemplateChoiceRowHtml(template, template.id === safeCurrent),
      ),
    ].join("");
    const emptyHint = templates.length
      ? ""
      : '<div class="modal-hint">模板库为空，当前只能选择仅开启重定向。</div>';
    showModalWithHistory(
      '<div class="modal-title">新应用默认配置</div>' +
        '<div class="modal-hint">选择一个模板后，新安装应用会自动使用该模板生成配置。</div>' +
        '<div class="template-card template-list-scroll template-picker-list auto-template-picker">' +
        rows +
        "</div>" +
        emptyHint,
      { backdropClose: true },
    );
    let picked = false;
    document.querySelectorAll("#modalContent .template-row").forEach((row) => {
      row.addEventListener("click", async () => {
        picked = true;
        closeActiveModal();
        await onPick(row.dataset.templateId || row.dataset.id || "");
      });
    });
    const previousCleanup = State.modalCleanup;
    State.modalCleanup = () => {
      previousCleanup?.();
      if (!picked) onDismiss?.();
    };
  }
  function showAutoTemplateEmptyEnableDialog(onConfirm, onDismiss) {
    showModalWithHistory(
      '<div class="modal-title">没有配置模板</div>' +
        '<div class="modal-hint">当前还没有配置模板。继续开启后，新安装应用会默认只开启重定向，不会附加允许路径、沙盒路径或映射规则。</div>' +
        '<div class="modal-actions"><button class="btn btn-secondary modal-close" type="button">取消</button><button class="btn btn-primary" id="autoTemplateEmptyEnable" type="button">继续开启</button></div>',
    );
    let confirmed = false;
    document.querySelector(".modal-close")?.addEventListener("click", () => closeActiveModal());
    document.getElementById("autoTemplateEmptyEnable")?.addEventListener("click", async () => {
      confirmed = true;
      closeActiveModal();
      await onConfirm();
    });
    const previousCleanup = State.modalCleanup;
    State.modalCleanup = () => {
      previousCleanup?.();
      if (!confirmed) onDismiss?.();
    };
  }
  async function isAutoTemplateReferenced(templateId) {
    if (!isSafeTemplateId(templateId)) return false;
    try {
      const global = normalizeGlobalRuntimeConfig(await Api.readGlobalConfig({ force: true }));
      State.globalConfig = global;
      return global.auto_enable_new_apps_template_id === templateId;
    } catch {
      return State.globalConfig?.auto_enable_new_apps_template_id === templateId;
    }
  }
  function showAutoTemplateDeleteBlocked() {
    Theme.showToast("该模板正用于新应用自动配置，不能删除", "error");
  }
  async function guardTemplateDelete(templateId) {
    if (await isAutoTemplateReferenced(templateId)) {
      showAutoTemplateDeleteBlocked();
      return false;
    }
    return true;
  }
  function iconHtml(name) {
    return '<span class="icon icon-' + name + '" aria-hidden="true"></span>';
  }

  function updatePrefs() {
    try {
      const saved = JSON.parse(localStorage.getItem(UPDATE_PREFS_KEY) || "{}");
      return {
        autoCheckUpdates: saved.autoCheckUpdates !== false,
        updateChannel: updateChannelValue(saved.updateChannel),
      };
    } catch {
      return { autoCheckUpdates: true, updateChannel: "Stable" };
    }
  }

  function saveUpdatePrefs(prefs) {
    const normalized = {
      autoCheckUpdates: prefs?.autoCheckUpdates !== false,
      updateChannel: updateChannelValue(prefs?.updateChannel),
    };
    try {
      localStorage.setItem(UPDATE_PREFS_KEY, JSON.stringify(normalized));
    } catch {}
    return normalized;
  }

  function updateChannelValue(value) {
    return UPDATE_CHANNEL_OPTIONS.some((item) => item.value === value) ? value : "Stable";
  }

  function updateChannelLabel(value) {
    return UPDATE_CHANNEL_OPTIONS.find((item) => item.value === value)?.label || "正式版";
  }

  function updateChannelBadge(channel, prerelease) {
    if (channel === "Beta" || prerelease) return "测试版";
    return "正式版";
  }

  function updateVersionBadge(update) {
    return String(update?.versionName || update?.tagName || "").trim();
  }

  function sanitizeReleaseNotes(markdown) {
    let normalized = String(markdown || "")
      .replace(/\r\n?/g, "\n")
      .trim();
    const commitHeading = normalized.search(/^#{1,6}\s*提交列表\s*$/im);
    if (commitHeading >= 0) normalized = normalized.slice(0, commitHeading).trimEnd();
    return normalized.replace(/^\*\*完整变更对比\*\*\s*:\s*https?:\/\/\S+\s*$/gim, "").trim();
  }

  function releaseNoteSections(markdown) {
    const notes = sanitizeReleaseNotes(markdown);
    if (!notes) return [];
    const heading = /^##\s*(模块更新|App\s*更新|其它更新|其他更新)\s*$/gim;
    const matches = Array.from(notes.matchAll(heading));
    if (!matches.length) return [{ title: "其它更新", markdown: notes }];
    return matches
      .map((match, index) => ({
        title: /^模块/.test(match[1])
          ? "模块更新"
          : /^app/i.test(match[1])
            ? "App 更新"
            : "其它更新",
        markdown: notes.slice(match.index + match[0].length, matches[index + 1]?.index).trim(),
      }))
      .filter((section) => section.markdown);
  }

  function markdownInlineHtml(text) {
    const codeTokens = [];
    let html = escapeHtml(text).replace(/`([^`]+)`/g, (_, code) => {
      const token = `@@SRX_CODE_${codeTokens.length}@@`;
      codeTokens.push("<code>" + code + "</code>");
      return token;
    });
    html = html
      .replace(
        /\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
        '<a href="$2" target="_blank" rel="noopener">$1</a>',
      )
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/__([^_]+)__/g, "<strong>$1</strong>")
      .replace(/(^|[^*])\*([^*]+)\*/g, "$1<em>$2</em>");
    return html.replace(/@@SRX_CODE_(\d+)@@/g, (_, index) => codeTokens[Number(index)] || "");
  }

  function markdownToHtml(markdown) {
    const lines = String(markdown || "")
      .replace(/\r\n?/g, "\n")
      .split("\n");
    const html = [];
    let listType = "";
    let inCode = false;
    let codeLines = [];
    const closeList = () => {
      if (listType) html.push(`</${listType}>`);
      listType = "";
    };
    lines.forEach((line) => {
      if (/^```/.test(line.trim())) {
        closeList();
        if (inCode) {
          html.push("<pre><code>" + escapeHtml(codeLines.join("\n")) + "</code></pre>");
          codeLines = [];
        }
        inCode = !inCode;
        return;
      }
      if (inCode) {
        codeLines.push(line);
        return;
      }
      const heading = /^(#{1,6})\s+(.+)$/.exec(line);
      const unordered = /^\s*[-*+]\s+(.+)$/.exec(line);
      const ordered = /^\s*\d+[.)]\s+(.+)$/.exec(line);
      if (heading) {
        closeList();
        const level = Math.min(6, heading[1].length + 1);
        html.push(`<h${level}>${markdownInlineHtml(heading[2])}</h${level}>`);
      } else if (unordered || ordered) {
        const nextType = unordered ? "ul" : "ol";
        if (listType !== nextType) {
          closeList();
          listType = nextType;
          html.push(`<${listType}>`);
        }
        html.push("<li>" + markdownInlineHtml((unordered || ordered)[1]) + "</li>");
      } else if (line.trim()) {
        closeList();
        html.push("<p>" + markdownInlineHtml(line.trim()) + "</p>");
      } else {
        closeList();
      }
    });
    closeList();
    if (inCode) html.push("<pre><code>" + escapeHtml(codeLines.join("\n")) + "</code></pre>");
    return html.join("");
  }

  function openExternalUrl(url) {
    const target = String(url || "").trim();
    if (!/^https?:\/\//i.test(target)) return false;
    try {
      if (window.open(target, "_blank", "noopener")) return true;
    } catch {}
    try {
      const anchor = document.createElement("a");
      anchor.href = target;
      anchor.target = "_blank";
      anchor.rel = "noopener";
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      return true;
    } catch {}
    try {
      window.location.href = target;
      return true;
    } catch {}
    return false;
  }

  function repositoryCardHtml(title, url, id) {
    return (
      '<button class="repository-card" id="' +
      id +
      '" type="button">' +
      '<span class="repository-card-title">' +
      escapeHtml(title) +
      "</span>" +
      '<span class="repository-card-url">' +
      escapeHtml(url) +
      "</span>" +
      "</button>"
    );
  }

  async function loadUpdate() {
    const content = $("#updateContent");
    if (!content) return;
    const prefs = updatePrefs();
    let moduleVersion = $("#moduleVersionDisplay")?.textContent || "";
    if (!moduleVersion || moduleVersion === "--" || moduleVersion === "...") {
      moduleVersion = await Api.getModuleVersion().catch(() => "");
    }
    const releaseUrl = Api.getReleaseRepositoryUrl
      ? Api.getReleaseRepositoryUrl()
      : "https://github.com/z1298808165/Storage-redirection-X-Public";
    const officialUrl = Api.getOfficialReleaseRepositoryUrl
      ? Api.getOfficialReleaseRepositoryUrl()
      : "https://github.com/Kindness-Kismet/Storage-redirection-X-Public";
    content.innerHTML =
      '<div class="section update-settings-section"><h2 class="section-title">更新</h2>' +
      '<div class="config-group update-settings-card">' +
      switchRow(
        "启动时检查更新",
        "autoCheckUpdates",
        prefs.autoCheckUpdates,
        "打开 WebUI 后自动检查所选通道的新版本",
      ) +
      settingSelectRow(
        "更新通道",
        "updateChannel",
        updateChannelLabel(prefs.updateChannel),
        "选择正式版、测试版或所有通道中的最新版本",
      ) +
      "</div></div>" +
      '<div class="update-current-card">' +
      '<div class="update-current-version">当前模块版本 ' +
      escapeHtml(moduleVersion || "--") +
      "</div>" +
      '<button class="btn btn-primary btn-block" id="updateCheckNow" type="button"' +
      (State.updateCheckRunning ? " disabled" : "") +
      ">" +
      (State.updateCheckRunning ? "正在检查" : "立即检查") +
      "</button>" +
      "</div>" +
      '<div class="section update-repository-section"><h2 class="section-title">发布仓库</h2>' +
      repositoryCardHtml("当前检查仓库", releaseUrl, "releaseRepositoryBtn") +
      repositoryCardHtml("官方发布仓库", officialUrl, "officialRepositoryBtn") +
      "</div>";

    content
      .querySelector('.toggle[data-key="autoCheckUpdates"]')
      ?.addEventListener("click", (event) => {
        const toggle = event.currentTarget;
        toggle.classList.toggle("on");
        saveUpdatePrefs(
          Object.assign({}, updatePrefs(), { autoCheckUpdates: toggle.classList.contains("on") }),
        );
        Theme.showToast("设置已保存", "success");
      });
    content
      .querySelector('.setting-select-row[data-key="updateChannel"]')
      ?.addEventListener("click", () => {
        showSettingOptionDialog(
          "更新通道",
          UPDATE_CHANNEL_OPTIONS,
          updatePrefs().updateChannel,
          (value) => {
            saveUpdatePrefs(Object.assign({}, updatePrefs(), { updateChannel: value }));
            loadUpdate();
          },
        );
      });
    content
      .querySelector("#updateCheckNow")
      ?.addEventListener("click", () =>
        handleUpdateCheck(true, { currentVersionName: moduleVersion }),
      );
    content
      .querySelector("#releaseRepositoryBtn")
      ?.addEventListener("click", () => openExternalUrl(releaseUrl));
    content
      .querySelector("#officialRepositoryBtn")
      ?.addEventListener("click", () => openExternalUrl(officialUrl));
  }

  function maybeRunStartupUpdateCheck(moduleVersion) {
    const prefs = updatePrefs();
    if (
      State.startupUpdateCheckDone ||
      !prefs.autoCheckUpdates ||
      !String(moduleVersion || "").trim()
    )
      return;
    State.startupUpdateCheckDone = true;
    handleUpdateCheck(false, { currentVersionName: moduleVersion });
  }

  async function handleUpdateCheck(manual, options) {
    if (State.updateCheckRunning) return;
    let currentVersionName = String(options?.currentVersionName || "").trim();
    if (!currentVersionName || currentVersionName === "--" || currentVersionName === "...") {
      currentVersionName = await Api.getModuleVersion().catch(() => "");
    }
    if (!currentVersionName) {
      if (manual) Theme.showToast("尚未读取模块版本", "error");
      return;
    }
    State.updateCheckRunning = true;
    if (State.currentPage === "update") loadUpdate();
    try {
      const prefs = updatePrefs();
      const update = await Api.checkForUpdates({
        currentVersionName,
        channel: prefs.updateChannel,
        repository: Api.getReleaseRepository ? Api.getReleaseRepository() : undefined,
      });
      if (update) showUpdateFoundDialog(update, currentVersionName);
      else if (manual) Theme.showToast("已是最新版本", "success");
    } catch (error) {
      if (manual) Theme.showToast("检查更新失败：" + (error?.message || "未知错误"), "error");
    } finally {
      State.updateCheckRunning = false;
      if (State.currentPage === "update") loadUpdate();
    }
  }

  function showUpdateFoundDialog(update, currentVersionName) {
    const noteSections = releaseNoteSections(update.releaseNotes);
    const notesHtml = noteSections.length
      ? '<div class="update-notes-scroll">' +
        noteSections
          .map(
            (section) =>
              '<section class="update-note-section"><h3>' +
              escapeHtml(section.title) +
              '</h3><div class="update-markdown">' +
              markdownToHtml(section.markdown) +
              "</div></section>",
          )
          .join("") +
        "</div>"
      : "";
    showModalWithHistory(
      '<div class="modal-title">发现新版本</div>' +
        '<div class="update-dialog-summary">当前模块版本 ' +
        escapeHtml(currentVersionName || "--") +
        "，有新版本可用。</div>" +
        '<div class="update-dialog-meta"><span>' +
        escapeHtml(updateChannelBadge(update.channel, update.prerelease)) +
        "</span>" +
        (updateVersionBadge(update)
          ? "<span>" + escapeHtml(updateVersionBadge(update)) + "</span>"
          : "") +
        "</div>" +
        notesHtml +
        '<div class="modal-actions"><button class="btn btn-secondary modal-close" type="button">取消</button><button class="btn btn-primary" id="openUpdateRelease" type="button">打开</button></div>',
      { backdropClose: true },
    );
    document.querySelector(".modal-close")?.addEventListener("click", () => closeActiveModal());
    document.getElementById("openUpdateRelease")?.addEventListener("click", () => {
      const opened = openExternalUrl(update.htmlUrl);
      closeActiveModal();
      if (!opened) Theme.showToast("未找到可用浏览器", "error");
    });
  }

  function hiddenFileInputHtml(id, accept) {
    return (
      '<input class="hidden-file-input" id="' +
      id +
      '" type="file" accept="' +
      escapeHtml(accept || "") +
      '" aria-hidden="true" tabindex="-1">'
    );
  }
  function pathItem(path, isExcluded, type) {
    const text = isExcluded && !String(path).startsWith("!") ? "!" + path : path;
    return (
      '<div class="path-item' +
      (isExcluded ? " excluded" : "") +
      '" data-path="' +
      escapeHtml(text) +
      '" data-type="' +
      escapeHtml(type || "allow") +
      '"><span class="path-item-text">' +
      escapeHtml(text) +
      '</span><button class="icon-btn icon-btn-sm icon-btn-danger path-item-delete" type="button" data-path="' +
      escapeHtml(text) +
      '" aria-label="移除路径" title="移除路径">' +
      iconHtml("x") +
      "</button></div>"
    );
  }
  function mappingItem(req, target) {
    return (
      '<div class="mapping-item" data-request="' +
      escapeHtml(req) +
      '" data-target="' +
      escapeHtml(target) +
      '"><span class="mapping-request">' +
      escapeHtml(req) +
      '</span><span class="mapping-arrow">→</span><span class="mapping-target">' +
      escapeHtml(target) +
      '</span><button class="icon-btn icon-btn-sm icon-btn-danger path-item-delete mapping-delete" type="button" data-request="' +
      escapeHtml(req) +
      '" aria-label="移除映射" title="移除映射">' +
      iconHtml("x") +
      "</button></div>"
    );
  }

  function bindConfigEvents(packageName, users) {
    const content = $("#appConfigContent");
    content.querySelectorAll(".user-tab[data-user]").forEach((tab) => {
      tab.addEventListener("click", () => {
        collectCurrentProfile(users);
        saveAppConfigIfAuto(packageName, users);
        State.configUserId = tab.dataset.user;
        renderAppConfig(
          packageName,
          { users },
          isTemplateEditorActive() ? { mode: "template" } : undefined,
        );
      });
    });
    content.querySelectorAll(".toggle").forEach((toggle) => {
      toggle.addEventListener("click", () => {
        toggle.classList.toggle("on");
        collectCurrentProfile(users);
        if (toggle.dataset.key === "readOnly") {
          const key = appReadOnlyEditorKey(packageName);
          const profile = getProfile(users);
          if (toggle.classList.contains("on")) {
            State.readOnlyEditorKeys.add(key);
          } else {
            State.readOnlyEditorKeys.delete(key);
            profile.read_only_paths = [];
          }
          renderAppConfig(
            packageName,
            { users },
            isTemplateEditorActive() ? { mode: "template" } : undefined,
          );
        } else if (toggle.dataset.key === "mappingOnly") {
          renderAppConfig(
            packageName,
            { users },
            isTemplateEditorActive() ? { mode: "template" } : undefined,
          );
        }
        saveAppConfigIfAuto(packageName, users);
      });
    });
    content
      .querySelector(".add-allow-btn")
      ?.addEventListener("click", () => showAddPathDialog(packageName, users, "allow"));
    content
      .querySelector(".add-readonly-btn")
      ?.addEventListener("click", () => showAddPathDialog(packageName, users, "readonly"));
    content
      .querySelector(".add-sandbox-btn")
      ?.addEventListener("click", () => showAddPathDialog(packageName, users, "sandbox"));
    content
      .querySelector(".add-mapping-btn")
      ?.addEventListener("click", () => showAddMappingDialog(packageName, users));
    content.querySelectorAll(".path-item-delete:not(.mapping-delete)").forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const row = btn.closest(".path-item");
        Theme.confirmDelete('删除路径 "' + btn.dataset.path + '"？', () => {
          deletePath(users, btn.dataset.path, row?.dataset.type || "allow");
          renderAppConfig(
            packageName,
            { users },
            isTemplateEditorActive() ? { mode: "template" } : undefined,
          );
          saveAppConfigIfAuto(packageName, users);
        });
      });
    });
    content.querySelectorAll(".mapping-delete").forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        Theme.confirmDelete('删除映射 "' + btn.dataset.request + '"？', () => {
          deleteMapping(users, btn.dataset.request);
          renderAppConfig(
            packageName,
            { users },
            isTemplateEditorActive() ? { mode: "template" } : undefined,
          );
          saveAppConfigIfAuto(packageName, users);
        });
      });
    });
    content.querySelectorAll(".path-item").forEach((item) => {
      item.addEventListener("click", () =>
        showAddPathDialog(packageName, users, item.dataset.type || "allow", {
          originalPath: item.dataset.path || "",
        }),
      );
    });
    content.querySelectorAll(".mapping-item").forEach((item) => {
      item.addEventListener("click", () =>
        showAddMappingDialog(packageName, users, {
          originalRequest: item.dataset.request || "",
          originalTarget: item.dataset.target || "",
        }),
      );
    });
    bindAppConfigActions(packageName, users, false);
  }

  function collectTemplateEditorName() {
    const input = document.getElementById("templateNameEdit");
    const name = normalizeTemplateName(input?.value || State.templateEditor?.name || "");
    input?.classList.toggle("invalid", !name);
    return name;
  }

  async function saveTemplateEditor(users, options) {
    if (!State.templateEditor || !users) return false;
    collectCurrentProfile(users);
    const name = collectTemplateEditorName();
    if (!name) {
      Theme.showToast("\u8bf7\u8f93\u5165\u6a21\u677f\u540d\u79f0", "error");
      return false;
    }
    const snapshot = JSON.parse(JSON.stringify(users || {}));
    const ok = await upsertTemplate({
      id: State.templateEditor.id,
      name,
      config: { users: snapshot },
    });
    if (ok) {
      State.templateEditor.name = name;
      $("#configAppName").textContent = name;
      Theme.showToast(
        options?.auto
          ? "\u6a21\u677f\u5df2\u81ea\u52a8\u4fdd\u5b58"
          : "\u6a21\u677f\u5df2\u4fdd\u5b58",
        "success",
      );
      await loadTemplates(true);
    } else {
      Theme.showToast("\u4fdd\u5b58\u6a21\u677f\u5931\u8d25", "error");
    }
    return ok;
  }

  function bindAppConfigActions(packageName, users, disabled) {
    const saveBtn = $("#appConfigSave");
    const deleteBtn = $("#appConfigDelete");
    const templateBtn = $("#appConfigTemplate");
    const isTemplateMode = isTemplateEditorActive();
    const autoSaveEnabled = !isTemplateMode && isAppConfigAutoSaveEnabled();
    if (saveBtn) {
      saveBtn.hidden = autoSaveEnabled;
      saveBtn.classList.toggle("is-hidden", autoSaveEnabled);
      saveBtn.style.display = autoSaveEnabled ? "none" : "";
      saveBtn.disabled = !!disabled || !users;
      saveBtn.onclick =
        users && !autoSaveEnabled
          ? async () => {
              if (isTemplateMode) await saveTemplateEditor(users);
              else await saveCurrentConfig(packageName, users);
            }
          : null;
    }
    if (deleteBtn) {
      deleteBtn.disabled = !!disabled || !users;
      if (users && isTemplateMode) {
        deleteBtn.onclick = async () => {
          if (!(await guardTemplateDelete(State.templateEditor?.id || ""))) return;
          Theme.confirmDelete(
            "\u5220\u9664\u6a21\u677f\u201c" + (State.templateEditor?.name || "") + "\u201d\uff1f",
            async () => {
              const ok = await Api.deleteTemplate(State.templateEditor.id);
              if (ok) {
                State.templates = State.templates.filter(
                  (item) => item.id !== State.templateEditor.id,
                );
                Theme.showToast("\u6a21\u677f\u5df2\u5220\u9664", "success");
                closeTemplateEditor({ replaceHistory: true });
              } else Theme.showToast("\u5220\u9664\u6a21\u677f\u5931\u8d25", "error");
            },
          );
        };
      } else {
        deleteBtn.onclick = users
          ? () => {
              Theme.confirmDelete(
                '\u786e\u5b9a\u5220\u9664 \"' +
                  packageName +
                  '\" \u7684\u5168\u90e8\u914d\u7f6e\uff1f',
                async () => {
                  const ok = await Api.deleteAppConfig(packageName);
                  if (ok) {
                    updateCachedAppConfigured(packageName, false, false);
                    Theme.showToast("\u914d\u7f6e\u5df2\u5220\u9664", "success");
                    State.currentPage = "apps";
                    State.shouldRestoreAppListScroll = true;
                    Theme.navigateTo("apps", { preserveScroll: true });
                    loadAppList(false);
                  } else Theme.showToast("\u5220\u9664\u5931\u8d25", "error");
                },
              );
            }
          : null;
      }
    }
    if (templateBtn) {
      templateBtn.hidden = isTemplateMode;
      templateBtn.classList.toggle("is-hidden", isTemplateMode);
      templateBtn.disabled = !!disabled || !users || isTemplateMode;
      templateBtn.onclick =
        users && !isTemplateMode ? () => showAppTemplateActions(packageName, users) : null;
    }
  }

  function showAppTemplateActions(packageName, users) {
    showModalWithHistory(
      '<div class="modal-title">配置模板</div>' +
        '<div class="modal-hint">将当前应用配置保存为模板，或用已有模板覆盖当前应用配置。</div>' +
        '<div class="modal-actions" style="flex-direction:column"><button class="btn btn-secondary btn-block" id="applyTemplateBtn" type="button">应用已有模板</button><button class="btn btn-primary btn-block" id="saveAsTemplateBtn" type="button">保存为模板</button></div>',
      { backdropClose: true },
    );
    document.getElementById("saveAsTemplateBtn")?.addEventListener("click", () => {
      closeActiveModal();
      saveCurrentConfigAsTemplate(packageName, users);
    });
    document.getElementById("applyTemplateBtn")?.addEventListener("click", () => {
      closeActiveModal();
      showTemplatePickerDialog("应用配置模板", async (template) => {
        const confirmed = await confirmAction(
          "将模板“" + template.name + "”应用到当前应用，现有配置会被覆盖。是否继续？",
        );
        if (!confirmed) return;
        const ok = await applyTemplateToPackages(template, [packageName]);
        if (ok) {
          State.configUserId = State.currentUserId;
          renderAppConfig(packageName, template.config);
        }
      });
    });
  }

  function collectCurrentProfile(users) {
    const content = $("#appConfigContent");
    const profile = users[State.configUserId || State.currentUserId] || createDefaultProfile();
    const enableToggle = content.querySelector('.toggle[data-key="enable"]');
    const mappingToggle = content.querySelector('.toggle[data-key="mappingOnly"]');
    profile.enabled = enableToggle ? enableToggle.classList.contains("on") : false;
    profile.mapping_mode_only = mappingToggle ? mappingToggle.classList.contains("on") : false;
    users[State.configUserId || State.currentUserId] = profile;
  }

  function isAppConfigAutoSaveEnabled() {
    return State.globalConfig?.app_config_auto_save === true;
  }

  async function saveAppConfigIfAuto(packageName, users) {
    if (isTemplateEditorActive()) return false;
    if (!isAppConfigAutoSaveEnabled() || !users) return false;
    return await saveCurrentConfig(packageName, users, { auto: true });
  }

  async function saveCurrentConfig(packageName, users, options) {
    collectCurrentProfile(users);
    const snapshot = JSON.parse(JSON.stringify(users || {}));
    const writeTask = async () => {
      const ok = await Api.writeAppConfig(packageName, { users: snapshot });
      if (ok) await Api.touchConfig();
      return ok;
    };
    State.appConfigSaveQueue = State.appConfigSaveQueue.catch(() => false).then(writeTask);
    const ok = await State.appConfigSaveQueue;
    if (ok) {
      const listProfile = snapshot[String(State.currentUserId || "0")];
      updateCachedAppConfigured(packageName, true, listProfile?.enabled === true);
      Theme.showToast(options?.auto ? "已自动保存" : "配置已保存", "success");
    } else Theme.showToast("保存失败", "error");
    return ok;
  }

  function createDefaultProfile(options) {
    const enabledDefault =
      options && Object.prototype.hasOwnProperty.call(options, "enabledDefault")
        ? options.enabledDefault
        : false;
    return {
      enabled: enabledDefault,
      mapping_mode_only: false,
      allowed_real_paths: [],
      excluded_real_paths: [],
      sandboxed_paths: [],
      read_only_paths: [],
      path_mappings: {},
    };
  }
  function normalizeUsersConfig(users) {
    const out = {};
    Object.keys(users || {}).forEach((id) => {
      const raw = users[id] || {};
      const profile = Object.assign(createDefaultProfile({ enabledDefault: true }), raw);
      profile.allowed_real_paths = Array.isArray(profile.allowed_real_paths)
        ? profile.allowed_real_paths
        : [];
      profile.excluded_real_paths = Array.isArray(profile.excluded_real_paths)
        ? profile.excluded_real_paths
        : [];
      const readOnlyPaths = normalizeReadOnlyRules(profile.read_only_paths);
      profile.allowed_real_paths = sortPathRules(
        removeConflictingAllowedRules(
          mergeAllowedRules(profile.allowed_real_paths, profile.excluded_real_paths),
        ),
      );
      profile.excluded_real_paths = [];
      profile.read_only_paths = normalizeReadOnlyForRules(
        readOnlyPaths,
        profile.allowed_real_paths,
      );
      profile.sandboxed_paths = sortPathRules(
        Array.isArray(profile.sandboxed_paths) ? profile.sandboxed_paths : [],
      );
      if (!profile.path_mappings || typeof profile.path_mappings !== "object")
        profile.path_mappings = {};
      profile.path_mappings = sortPathMappings(profile.path_mappings);
      out[id] = profile;
    });
    return out;
  }
  function getActiveConfigUserId() {
    return State.configUserId || State.currentUserId;
  }
  function getProfile(users) {
    return users[getActiveConfigUserId()] || createDefaultProfile();
  }
  function appReadOnlyEditorKey(packageName) {
    const owner = isTemplateEditorActive()
      ? "template:" + (State.templateEditor?.id || packageName || "")
      : "app:" + (packageName || "");
    return owner + ":user:" + getActiveConfigUserId();
  }

  function getAllowedRules(profile) {
    return sortPathRules(
      removeConflictingAllowedRules(
        mergeAllowedRules(profile.allowed_real_paths || [], profile.excluded_real_paths || []),
      ),
    );
  }
  function mergeAllowedRules(allowed, excluded) {
    const seen = new Set();
    const rules = [];
    [
      ...(allowed || []),
      ...(excluded || []).map((path) => (String(path).startsWith("!") ? path : "!" + path)),
    ].forEach((path) => {
      const rule = String(path || "").trim();
      if (!rule || seen.has(rule)) return;
      seen.add(rule);
      rules.push(rule);
    });
    return rules;
  }
  function removeConflictingAllowedRules(rules) {
    const allowed = new Set();
    const excluded = new Set();
    (rules || []).forEach((rule) => {
      const text = String(rule || "").trim();
      if (!text) return;
      if (text.startsWith("!")) excluded.add(text.slice(1));
      else allowed.add(text);
    });
    const conflicts = new Set([...allowed].filter((path) => excluded.has(path)));
    if (!conflicts.size) return rules || [];
    return (rules || []).filter(
      (rule) => String(rule || "").startsWith("!") || !conflicts.has(String(rule || "")),
    );
  }
  function normalizeReadOnlyRules(paths) {
    const source = Array.isArray(paths) ? paths : typeof paths === "string" ? [paths] : [];
    const out = [];
    source.forEach((raw) => {
      const path = String(raw || "").trim();
      if (!path || out.includes(path)) return;
      out.push(path);
    });
    return sortPathRules(out);
  }
  function normalizeReadOnlyForRules(readOnlyPaths, allowedRules) {
    const excluded = new Set(
      (allowedRules || [])
        .filter((rule) => String(rule || "").startsWith("!"))
        .map((rule) => String(rule).slice(1)),
    );
    return sortPathRules(
      (readOnlyPaths || []).filter((path) => {
        const text = String(path || "");
        return text.startsWith("!") || !excluded.has(text);
      }),
    );
  }
  function normalizePathSortKey(path) {
    return String(path || "")
      .replace(/^!/, "")
      .replace(/^\/+/, "")
      .toLowerCase();
  }
  function comparePathNames(a, b) {
    const left = normalizePathSortKey(a);
    const right = normalizePathSortKey(b);
    return (
      left.localeCompare(right, "en", { numeric: true, sensitivity: "base" }) ||
      String(a).localeCompare(String(b), "en")
    );
  }
  function sortPathRules(paths) {
    return [...(paths || [])].sort(comparePathNames);
  }
  function sortPathMappings(mappings) {
    const entries = Array.isArray(mappings)
      ? mappings.map((m) => [m.request_path, m.final_path])
      : Object.entries(mappings || {});
    return entries
      .filter(
        ([req, target]) =>
          typeof req === "string" &&
          req &&
          typeof target === "string" &&
          target &&
          !isAndroidDataOrObbPath(target),
      )
      .sort(([a], [b]) => comparePathNames(a, b))
      .reduce((out, [req, target]) => {
        out[req] = target;
        return out;
      }, {});
  }

  function deletePath(users, path, type) {
    const p = getProfile(users);
    if ((type || "allow") === "allow") {
      p.allowed_real_paths = (p.allowed_real_paths || []).filter((x) => x !== path);
    }
    p.excluded_real_paths = [];
    if (type === "sandbox") {
      p.sandboxed_paths = (p.sandboxed_paths || []).filter((x) => x !== path);
    }
    if (type === "readonly") {
      p.read_only_paths = (p.read_only_paths || []).filter((x) => x !== path);
    }
    users[getActiveConfigUserId()] = p;
  }
  function deleteMapping(users, req) {
    const p = getProfile(users);
    if (p.path_mappings) delete p.path_mappings[req];
    users[getActiveConfigUserId()] = p;
  }

  // ═══ Path Validation ═══
  const PATH_RULES = {
    noAbsolute: { re: /^\//, msg: "不能使用绝对路径，请输入相对路径" },
    noDots: { re: /\.\./, msg: "路径不能包含 .." },
    noUnsafe: { re: /[<>:"|?*\x00-\x1f]/, msg: "路径包含非法字符" },
    noEmpty: { re: /^\s*$/, msg: "路径不能为空" },
  };

  function validatePath(raw, options) {
    if (!raw || !raw.trim()) return { valid: false, msg: "路径不能为空" };
    const allowRuleSyntax = !!(options && options.allowRuleSyntax);
    const allowWildcards = allowRuleSyntax || !!(options && options.allowWildcards);
    const trimmed = raw.trim();
    const pathPart = allowRuleSyntax && trimmed.startsWith("!") ? trimmed.slice(1) : trimmed;
    if (!pathPart.trim()) return { valid: false, msg: "路径不能为空" };
    if (!allowRuleSyntax && trimmed.startsWith("!"))
      return { valid: false, msg: "该路径不支持 ! 前缀" };
    if (trimmed.startsWith("/")) return { valid: false, msg: "不能使用绝对路径，请输入相对路径" };
    if (pathPart.startsWith("/")) return { valid: false, msg: "不能使用绝对路径，请输入相对路径" };
    if (pathPart.split("/").some((part) => part === "." || part === ".."))
      return { valid: false, msg: "路径不能包含 . 或 .." };
    const unsafeRe = allowWildcards ? /[<>:"|\x00-\x1f]/ : /[<>:"|?*\x00-\x1f]/;
    if (unsafeRe.test(pathPart)) return { valid: false, msg: "路径包含非法字符" };
    if (options?.disallowPrivateAndroidTarget && isAndroidDataOrObbPath(pathPart)) {
      return { valid: false, msg: "映射目标不能位于 Android/data 或 Android/obb" };
    }
    return { valid: true, msg: "路径格式正确" };
  }

  function isAndroidDataOrObbPath(path) {
    const normalized = String(path || "")
      .trim()
      .replace(/\\/g, "/")
      .replace(/^\/+/, "")
      .replace(/^storage\/emulated\/\d+\/?/i, "")
      .replace(/^storage\/self\/primary\/?/i, "")
      .replace(/^data\/media\/\d+\/?/i, "")
      .replace(/^sdcard\/?/i, "")
      .replace(/\/+$/g, "");
    const parts = normalized.split("/").filter(Boolean);
    if (parts.length < 2 || parts[0].toLowerCase() !== "android") return false;
    const section = parts[1].toLowerCase();
    return section === "data" || section === "obb";
  }

  function hasMonitorFilterStorageRootPrefix(path) {
    const lower = String(path || "")
      .replace(/\\/g, "/")
      .replace(/^\/+/, "")
      .toLowerCase();
    return (
      lower === "sdcard" ||
      lower.startsWith("sdcard/") ||
      lower === "storage/emulated" ||
      lower.startsWith("storage/emulated/") ||
      lower === "storage/self/primary" ||
      lower.startsWith("storage/self/primary/") ||
      lower === "data/media" ||
      lower.startsWith("data/media/")
    );
  }

  function validateMonitorFilterPath(raw, options) {
    const allowLegacyAbsolute = !!(options && options.allowLegacyAbsolute);
    let value = String(raw || "")
      .trim()
      .replace(/\\/g, "/")
      .replace(/\/+/g, "/");
    if (!value) return { valid: false, value: "", msg: "路径不能为空" };
    if (hasMonitorFilterStorageRootPrefix(value))
      return { valid: false, value: "", msg: "不能带存储根目录，请输入相对路径" };
    if (value.startsWith("/")) {
      if (!allowLegacyAbsolute)
        return { valid: false, value: "", msg: "不能使用绝对路径，请输入相对路径" };
      value = value.replace(/^\/+/, "");
    }
    value = value.replace(/^\/+|\/+$/g, "");
    if (!value) return { valid: false, value: "", msg: "路径不能为空" };
    if (value.startsWith("!")) return { valid: false, value: "", msg: "过滤路径不支持排除前缀" };
    if (value.includes("\0") || value.length > 512)
      return { valid: false, value: "", msg: "路径格式不正确" };
    if (value.split("/").some((part) => part === "." || part === ".."))
      return { valid: false, value: "", msg: "路径不能包含 . 或 .." };
    if (hasMonitorFilterStorageRootPrefix(value))
      return { valid: false, value: "", msg: "不能带存储根目录，请输入相对路径" };
    if (/[<>:"|\x00-\x1f]/.test(value)) return { valid: false, value: "", msg: "路径包含非法字符" };
    return { valid: true, value, msg: "路径格式正确" };
  }

  function normalizeMonitorFilterPathList(list) {
    const seen = new Set();
    const out = [];
    (Array.isArray(list) ? list : []).forEach((item) => {
      const result = validateMonitorFilterPath(item, { allowLegacyAbsolute: true });
      if (!result.valid || seen.has(result.value)) return;
      seen.add(result.value);
      out.push(result.value);
    });
    return out.sort(compareMonitorFilterValues);
  }

  function normalizeMonitorFilterOperationList(list) {
    const seen = new Set();
    const out = [];
    (Array.isArray(list) ? list : []).forEach((item) => {
      const value = String(item || "").trim();
      if (!value || value.includes("\0") || value.length > 512 || seen.has(value)) return;
      seen.add(value);
      out.push(value);
    });
    return isLegacyDefaultMonitorOperations(out)
      ? DEFAULT_MONITOR_FILTER_OPERATIONS.slice()
      : out.sort(compareMonitorFilterValues);
  }

  function compareMonitorFilterValues(left, right) {
    const leftText = String(left);
    const rightText = String(right);
    const leftLower = leftText.toLowerCase();
    const rightLower = rightText.toLowerCase();
    if (leftLower < rightLower) return -1;
    if (leftLower > rightLower) return 1;
    return leftText < rightText ? -1 : leftText > rightText ? 1 : 0;
  }

  function isLegacyDefaultMonitorOperations(list) {
    return (
      isSameStringSet(list, LEGACY_DEFAULT_MONITOR_FILTER_OPERATIONS) ||
      isSameStringSet(list, LEGACY_FULL_DEFAULT_MONITOR_FILTER_OPERATIONS)
    );
  }

  function showMonitorFilterPathValidation(inputEl, hintEl) {
    if (!inputEl || !hintEl) return false;
    if (!inputEl.value.trim()) {
      hintEl.textContent = "";
      hintEl.className = "path-validation";
      inputEl.classList.remove("invalid");
      return false;
    }
    const result = validateMonitorFilterPath(inputEl.value, { allowLegacyAbsolute: false });
    hintEl.textContent = result.msg;
    hintEl.className = "path-validation " + (result.valid ? "valid" : "invalid");
    inputEl.classList.toggle("invalid", !result.valid);
    return result.valid;
  }

  function showPathValidation(inputEl, hintEl, options) {
    const result = validatePath(inputEl.value, options);
    if (!inputEl.value.trim()) {
      hintEl.textContent = "";
      inputEl.classList.remove("invalid");
      return;
    }
    hintEl.textContent = result.msg;
    hintEl.className = "path-validation " + (result.valid ? "valid" : "invalid");
    inputEl.classList.toggle("invalid", !result.valid);
    return result.valid;
  }

  // ═══ Path Browser (auto-complete from filesystem) ═══
  function buildPathBrowser(basePath, inputEl, hintEl, onSelect, options) {
    const container = document.createElement("div");
    container.className = "path-browser";
    container.innerHTML = "";
    let currentDir = basePath;
    let currentRulePrefix = "";
    let currentPrefix = "";
    let currentQuery = "";
    let requestId = 0;
    let loadTimer = null;
    let cachedDir = "";
    let cachedContents = null;
    let renderedSignature = "";
    const validateOptions = (options && options.validateOptions) || null;
    const directoriesOnly = !!(options && options.directoriesOnly);

    const normalizeRelative = (value) =>
      value.trim().replace(/^!/, "").replace(/^\/+/, "").replace(/\/+/g, "/");
    const moveInputCursorToEnd = () => {
      const end = inputEl.value.length;
      try {
        inputEl.focus({ preventScroll: true });
        inputEl.setSelectionRange(end, end);
      } catch {
        try {
          inputEl.setSelectionRange(end, end);
        } catch {}
      }
    };
    const splitInputPath = (value) => {
      const normalized = normalizeRelative(value);
      if (!normalized || normalized.endsWith("/")) {
        return { dirRel: normalized.replace(/\/$/, ""), query: "", prefix: normalized };
      }
      const slash = normalized.lastIndexOf("/");
      if (slash === -1) return { dirRel: "", query: normalized.toLowerCase(), prefix: "" };
      return {
        dirRel: normalized.slice(0, slash),
        query: normalized.slice(slash + 1).toLowerCase(),
        prefix: normalized.slice(0, slash + 1),
      };
    };

    const choosePath = (relative, isDir) => {
      const selectedPath = isDir ? relative.replace(/\/?$/, "/") : relative;
      inputEl.value = currentRulePrefix + selectedPath;
      moveInputCursorToEnd();
      showPathValidation(inputEl, hintEl, validateOptions);
      if (onSelect) onSelect(inputEl.value);
      if (isDir) loadForInput(inputEl.value);
    };

    const renderContents = (dir, contents) => {
      const signature = [
        dir,
        currentPrefix,
        currentQuery,
        contents.dirs.join("\0"),
        contents.files.join("\0"),
      ].join("\n");
      if (signature === renderedSignature) return;
      renderedSignature = signature;
      container.innerHTML = "";
      const fragment = document.createDocumentFragment();
      let itemCount = 0;
      if (dir !== basePath) {
        const up = document.createElement("div");
        up.className = "path-browser-item dir";
        up.textContent = "..";
        up.addEventListener("click", () => {
          const parent = dir.split("/").slice(0, -1).join("/") || basePath;
          const relative =
            parent === basePath ? "" : parent.substring(basePath.length + 1).replace(/\/?$/, "/");
          inputEl.value = currentRulePrefix + relative;
          moveInputCursorToEnd();
          showPathValidation(inputEl, hintEl, validateOptions);
          loadForInput(inputEl.value);
        });
        fragment.appendChild(up);
        itemCount += 1;
      }
      contents.dirs.forEach((name) => {
        if (currentQuery && !name.toLowerCase().includes(currentQuery)) return;
        const item = document.createElement("div");
        item.className = "path-browser-item dir";
        item.textContent = name;
        const relative = currentPrefix ? currentPrefix + name : name;
        item.addEventListener("click", () => choosePath(relative, true));
        fragment.appendChild(item);
        itemCount += 1;
      });
      if (!directoriesOnly)
        contents.files.forEach((name) => {
          if (currentQuery && !name.toLowerCase().includes(currentQuery)) return;
          const item = document.createElement("div");
          item.className = "path-browser-item file";
          item.textContent = name;
          item.addEventListener("click", () => {
            const relative = currentPrefix ? currentPrefix + name : name;
            choosePath(relative, false);
          });
          fragment.appendChild(item);
          itemCount += 1;
        });
      if (!itemCount) {
        const empty = document.createElement("div");
        empty.className = "path-browser-loading";
        empty.textContent = "没有匹配的路径";
        fragment.appendChild(empty);
      }
      container.appendChild(fragment);
    };

    const loadDir = async (dir) => {
      const thisRequest = ++requestId;
      if (cachedDir === dir && cachedContents) {
        renderContents(dir, cachedContents);
        return;
      }
      renderedSignature = "";
      if (!container.children.length)
        container.innerHTML = '<div class="path-browser-loading">加载目录...</div>';
      try {
        const contents = await Api.listDir(dir);
        if (thisRequest !== requestId) return;
        cachedDir = dir;
        cachedContents = contents;
        renderContents(dir, contents);
      } catch {
        container.innerHTML = '<div class="path-browser-loading">无法读取目录</div>';
      }
    };

    const loadForInput = (value) => {
      clearTimeout(loadTimer);
      currentRulePrefix =
        validateOptions?.allowRuleSyntax && value.trim().startsWith("!") ? "!" : "";
      const parsed = splitInputPath(value);
      currentPrefix = parsed.prefix;
      currentQuery = parsed.query;
      currentDir = parsed.dirRel ? basePath + "/" + parsed.dirRel : basePath;
      loadTimer = setTimeout(() => loadDir(currentDir), 220);
    };

    container.loadForInput = loadForInput;
    if (options && options.autoLoad) loadForInput(inputEl.value || "");
    return container;
  }

  // ═══ Dialogs ═══
  function showAddPathDialog(packageName, users, type, editOptions) {
    const originalPath = editOptions?.originalPath || "";
    const isEdit = !!originalPath;
    const titles = { allow: "添加允许路径", sandbox: "添加沙盒路径", readonly: "添加只读路径" };
    const hints = {
      allow: "直接输入路径可放行；加 ! 前缀可排除子路径。",
      sandbox: "仅映射模式下，未命中映射时进入沙盒。",
      readonly: "只读路径保持可读但禁止写入；加 ! 前缀可排除子路径。",
    };
    const storageBase = "/storage/emulated/" + getActiveConfigUserId();
    const validateOptions =
      type === "allow"
        ? { allowRuleSyntax: true }
        : type === "readonly"
          ? { allowRuleSyntax: true, allowWildcards: true }
          : null;
    const actionButton =
      '<button class="btn btn-primary" id="modalAddBtn" type="button">' +
      (isEdit ? "保存修改" : "添加") +
      "</button>";

    showModalWithHistory(
      '<div class="modal-title">' +
        (isEdit ? "修改路径" : titles[type]) +
        "</div>" +
        '<input type="text" class="modal-input" id="pathInput" placeholder="相对路径，如 Pictures/Screenshots" autocomplete="off" value="' +
        escapeHtml(originalPath) +
        '">' +
        '<div class="path-validation" id="pathValidation"></div>' +
        '<div id="pathBrowserContainer"></div>' +
        '<div class="modal-hint">' +
        hints[type] +
        "相对路径以 " +
        storageBase +
        " 为根。</div>" +
        '<div class="modal-actions"><button class="btn btn-secondary modal-close" type="button">取消</button>' +
        actionButton +
        "</div>",
    );

    const modal = $("#modalOverlay");
    const input = $("#pathInput");
    const hintEl = $("#pathValidation");
    const browserContainer = $("#pathBrowserContainer");

    // Path browser
    const browser = buildPathBrowser(storageBase, input, hintEl, null, {
      validateOptions,
      autoLoad: true,
    });
    browserContainer.appendChild(browser);

    // Validation on input
    let validateTimer;
    input.addEventListener("input", () => {
      clearTimeout(validateTimer);
      validateTimer = setTimeout(() => showPathValidation(input, hintEl, validateOptions), 180);
      browser.loadForInput(input.value);
    });

    $("#modalAddBtn")?.addEventListener("click", () => {
      const rawPath = input.value.trim();
      if (!validatePath(rawPath, validateOptions).valid) {
        Theme.showToast("路径格式不正确", "error");
        return;
      }
      const p = getProfile(users);
      const targetKey =
        type === "sandbox"
          ? "sandboxed_paths"
          : type === "readonly"
            ? "read_only_paths"
            : "allowed_real_paths";
      p[targetKey] = p[targetKey] || [];
      if (isEdit) p[targetKey] = p[targetKey].filter((item) => item !== originalPath);
      if (p[targetKey].includes(rawPath)) {
        Theme.showToast("路径已存在", "error");
        return;
      }
      p[targetKey].push(rawPath);
      p[targetKey] = sortPathRules(p[targetKey]);
      if (type === "readonly") p.read_only_paths = normalizeReadOnlyRules(p.read_only_paths);
      users[getActiveConfigUserId()] = p;
      closeActiveModal();
      renderAppConfig(
        packageName,
        { users },
        isTemplateEditorActive() ? { mode: "template" } : undefined,
      );
      saveAppConfigIfAuto(packageName, users);
    });

    document.querySelector(".modal-close")?.addEventListener("click", () => closeActiveModal());
    setTimeout(() => focusWithoutViewportJump(input), 260);
  }

  function showAddMappingDialog(packageName, users, editOptions) {
    const originalRequest = editOptions?.originalRequest || "";
    const originalTarget = editOptions?.originalTarget || "";
    const isEdit = !!originalRequest;
    const storageBase = "/storage/emulated/" + getActiveConfigUserId();
    const actionButton =
      '<button class="btn btn-primary" id="modalAddMapping" type="button">' +
      (isEdit ? "保存修改" : "添加") +
      "</button>";
    const targetValidateOptions = { disallowPrivateAndroidTarget: true };

    showModalWithHistory(
      '<div class="modal-title">' +
        (isEdit ? "修改路径映射" : "添加路径映射") +
        "</div>" +
        '<input type="text" class="modal-input" id="mappingRequest" placeholder="请求路径，如 DCIM/Camera" autocomplete="off" style="margin-bottom:8px" value="' +
        escapeHtml(originalRequest) +
        '">' +
        '<div class="path-validation" id="reqValidation"></div>' +
        '<div id="reqBrowserContainer"></div>' +
        '<input type="text" class="modal-input" id="mappingTarget" placeholder="目标路径，如 Pictures/CameraBackup" autocomplete="off" style="margin-top:12px" value="' +
        escapeHtml(originalTarget) +
        '">' +
        '<div class="path-validation" id="targetValidation"></div>' +
        '<div id="targetBrowserContainer"></div>' +
        '<div class="modal-hint">路径匹配时重定向。相对于 ' +
        storageBase +
        "。</div>" +
        '<div class="modal-actions"><button class="btn btn-secondary modal-close" type="button">取消</button>' +
        actionButton +
        "</div>",
    );

    const modal = $("#modalOverlay");
    const reqInput = $("#mappingRequest");
    const targetInput = $("#mappingTarget");
    const reqHint = $("#reqValidation");
    const targetHint = $("#targetValidation");

    // Browsers
    const reqBrowser = buildPathBrowser(storageBase, reqInput, reqHint);
    const targetBrowser = buildPathBrowser(storageBase, targetInput, targetHint, null, {
      validateOptions: targetValidateOptions,
    });
    reqBrowser.hidden = true;
    targetBrowser.hidden = true;
    $("#reqBrowserContainer")?.appendChild(reqBrowser);
    $("#targetBrowserContainer")?.appendChild(targetBrowser);

    const activateBrowser = (type) => {
      const activeBrowser = type === "target" ? targetBrowser : reqBrowser;
      const inactiveBrowser = type === "target" ? reqBrowser : targetBrowser;
      const activeInput = type === "target" ? targetInput : reqInput;
      inactiveBrowser.hidden = true;
      activeBrowser.hidden = false;
      activeBrowser.loadForInput(activeInput?.value || "");
    };
    const hideBrowsers = () => {
      reqBrowser.hidden = true;
      targetBrowser.hidden = true;
    };
    const isBrowserControl = (target) =>
      target === reqInput ||
      target === targetInput ||
      reqBrowser.contains(target) ||
      targetBrowser.contains(target);
    const modalContent = document.getElementById("modalContent");
    const hideBrowsersOnOutsideInteraction = (event) => {
      if (!isBrowserControl(event.target)) hideBrowsers();
    };
    modalContent?.addEventListener("pointerdown", hideBrowsersOnOutsideInteraction);
    modalContent?.addEventListener("focusin", hideBrowsersOnOutsideInteraction);
    State.modalCleanup = () => {
      modalContent?.removeEventListener("pointerdown", hideBrowsersOnOutsideInteraction);
      modalContent?.removeEventListener("focusin", hideBrowsersOnOutsideInteraction);
    };
    reqInput?.addEventListener("focus", () => activateBrowser("request"));
    targetInput?.addEventListener("focus", () => activateBrowser("target"));

    // Validation
    let reqTimer = null,
      targetTimer = null;
    reqInput?.addEventListener("input", () => {
      clearTimeout(reqTimer);
      reqTimer = setTimeout(() => showPathValidation(reqInput, reqHint), 180);
      activateBrowser("request");
    });
    targetInput?.addEventListener("input", () => {
      clearTimeout(targetTimer);
      targetTimer = setTimeout(
        () => showPathValidation(targetInput, targetHint, targetValidateOptions),
        180,
      );
      activateBrowser("target");
    });

    $("#modalAddMapping")?.addEventListener("click", () => {
      const req = reqInput.value.trim(),
        target = targetInput.value.trim();
      if (!validatePath(req).valid) {
        Theme.showToast("请求路径格式不正确", "error");
        return;
      }
      const targetValidation = validatePath(target, targetValidateOptions);
      if (!targetValidation.valid) {
        Theme.showToast(targetValidation.msg || "目标路径格式不正确", "error");
        return;
      }
      if (req === target) {
        Theme.showToast("路径不能相同", "error");
        return;
      }
      const p = getProfile(users);
      p.path_mappings = p.path_mappings || {};
      if (isEdit && originalRequest !== req) delete p.path_mappings[originalRequest];
      if (
        (!isEdit || originalRequest !== req) &&
        Object.prototype.hasOwnProperty.call(p.path_mappings, req)
      ) {
        Theme.showToast("请求路径已存在", "error");
        return;
      }
      p.path_mappings[req] = target;
      p.path_mappings = sortPathMappings(p.path_mappings);
      users[getActiveConfigUserId()] = p;
      closeActiveModal();
      renderAppConfig(
        packageName,
        { users },
        isTemplateEditorActive() ? { mode: "template" } : undefined,
      );
      saveAppConfigIfAuto(packageName, users);
    });

    document.querySelector(".modal-close")?.addEventListener("click", () => closeActiveModal());
  }

  // ═══ Settings ═══
  function themeModeLabel(mode) {
    return { light: "浅色", dark: "深色", system: "跟随系统" }[mode] || "跟随系统";
  }

  function themePreferenceSummary() {
    const uiOptions = Theme.getUiOptions();
    const color = uiOptions.dynamicColor
      ? accentColorLabel(Number(uiOptions.accentColor) || 0)
      : "默认配色";
    return (
      themeModeLabel(Theme.get()) + " · " + color + " · " + webUiPageScaleLabel(uiOptions.pageScale)
    );
  }

  function themeModeSelectorHtml(currentTheme) {
    const options = [
      { value: "system", label: "跟随系统" },
      { value: "light", label: "浅色" },
      { value: "dark", label: "深色" },
    ];
    return (
      '<div class="theme-selector" role="radiogroup" aria-label="主题模式">' +
      options
        .map(
          (option) =>
            '<label class="theme-option' +
            (currentTheme === option.value ? " active" : "") +
            '"><input type="radio" name="theme" value="' +
            option.value +
            '" ' +
            (currentTheme === option.value ? "checked" : "") +
            '><span class="theme-option-label">' +
            option.label +
            "</span></label>",
        )
        .join("") +
      "</div>"
    );
  }

  function accentPaletteHtml(selected) {
    return (
      '<div class="theme-color-palette" role="radiogroup" aria-label="强调色">' +
      WEBUI_ACCENT_COLOR_OPTIONS.map((option) => {
        const active = option.value === selected;
        const color = option.color || "var(--system-accent-color, var(--color-primary))";
        return (
          '<button class="theme-color-swatch' +
          (active ? " active" : "") +
          (option.value === 0 ? " system" : "") +
          '" type="button" data-value="' +
          option.value +
          '" role="radio" aria-checked="' +
          String(active) +
          '" aria-label="' +
          option.label +
          '" title="' +
          option.label +
          '" style="--swatch-color:' +
          color +
          '"><span aria-hidden="true"></span></button>'
        );
      }).join("") +
      "</div>"
    );
  }

  function pageScaleControlHtml(scale) {
    const percent = webUiPageScalePercent(scale);
    const progress = ((percent - 80) / 30) * 100;
    return (
      '<div class="theme-settings-card theme-scale-card" id="pageScaleCard" role="button" tabindex="0" aria-label="手动输入界面缩放，当前 ' +
      percent +
      '%">' +
      '<div class="theme-scale-header"><div class="switch-label-group">' +
      '<div class="switch-label">界面缩放</div>' +
      '<div class="switch-hint">调整整体界面缩放比例</div></div>' +
      '<output class="theme-scale-value" id="pageScaleValue" for="pageScaleSlider">' +
      percent +
      "%</output></div>" +
      '<div class="theme-scale-control">' +
      '<div class="theme-scale-keypoints" aria-hidden="true"><span style="--key-point:0%"></span><span style="--key-point:33.333%"></span><span style="--key-point:66.667%"></span><span style="--key-point:100%"></span></div>' +
      '<input class="theme-scale-slider" id="pageScaleSlider" type="range" min="80" max="110" step="1" value="' +
      percent +
      '" aria-label="界面缩放" aria-valuetext="' +
      percent +
      '%" style="--scale-progress:' +
      progress.toFixed(2) +
      '%">' +
      "</div></div>"
    );
  }

  function renderThemePage() {
    const content = $("#themeContent");
    if (!content) return;
    const currentTheme = Theme.get();
    const uiOptions = Theme.getUiOptions();
    const accentColor = Number(uiOptions.accentColor) || 0;
    const dynamicColor = uiOptions.dynamicColor === true;
    const viewportRatio =
      Math.max(0.42, Math.min(0.9, window.innerWidth / Math.max(window.innerHeight, 1))) || 0.5625;
    content.innerHTML =
      '<section class="theme-preview" aria-label="主题预览" style="--theme-preview-ratio:' +
      viewportRatio.toFixed(4) +
      '">' +
      '<div class="theme-preview-bar"><span></span><span></span></div>' +
      '<div class="theme-preview-body"><div class="theme-preview-primary"></div><div class="theme-preview-lines"><span></span><span></span><span></span></div></div>' +
      '<div class="theme-preview-nav"><span class="active"></span><span></span><span></span><span></span></div>' +
      "</section>" +
      '<div class="section theme-page-section"><h2 class="section-title">主题模式</h2>' +
      '<div class="theme-settings-card theme-mode-card">' +
      themeModeSelectorHtml(currentTheme) +
      "</div></div>" +
      '<div class="section theme-page-section"><h2 class="section-title">颜色</h2>' +
      '<div class="theme-settings-card"><div class="theme-settings-rows">' +
      switchRow(
        "动态取色",
        "dynamicColor",
        dynamicColor,
        "跟随系统壁纸生成界面配色，关闭后使用固定主题色",
      ) +
      (dynamicColor
        ? settingSelectRow(
            "强调色",
            "accentColor",
            accentColorLabel(accentColor),
            "选择系统取色，或指定应用主题色",
          )
        : "") +
      (dynamicColor && accentColor !== 0
        ? settingSelectRow(
            "色彩风格",
            "colorStyle",
            optionLabel(WEBUI_COLOR_STYLE_OPTIONS, uiOptions.colorStyle, uiOptions.colorStyle),
            "调整主题色板的明度与饱和度倾向",
          ) +
          settingSelectRow(
            "色彩标准",
            "colorSpec",
            optionLabel(WEBUI_COLOR_SPEC_OPTIONS, uiOptions.colorSpec, uiOptions.colorSpec),
            "选择主题色生成算法版本",
          )
        : "") +
      "</div></div></div>" +
      '<div class="section theme-page-section"><h2 class="section-title">显示</h2>' +
      pageScaleControlHtml(uiOptions.pageScale) +
      "</div>" +
      '<div class="section theme-page-section"><h2 class="section-title">视觉效果</h2>' +
      '<div class="theme-settings-card"><div class="theme-settings-rows">' +
      switchRow(
        "悬浮底栏",
        "floatingNav",
        uiOptions.floatingNav !== false,
        "让底部导航悬浮于页面内容之上",
      ) +
      switchRow(
        "液态玻璃",
        "liquidGlass",
        uiOptions.liquidGlass !== false,
        "启用玻璃高光、透镜放大与底栏跟随形变",
      ) +
      switchRow(
        "材质模糊",
        "blurEffect",
        uiOptions.blurEffect !== false,
        "为浮动面板和弹窗保留背景模糊",
      ) +
      "</div></div></div>";
    bindThemePageEvents(content);
  }

  function bindThemePageEvents(content) {
    content.querySelectorAll('input[name="theme"]').forEach((radio) => {
      radio.addEventListener("change", () => {
        Theme.apply(radio.value);
        renderThemePage();
      });
    });
    content.querySelectorAll(".toggle").forEach((toggle) => {
      toggle.addEventListener("click", () => {
        const key = toggle.dataset.key;
        const enabled = !toggle.classList.contains("on");
        Theme.setUiOption(key, enabled);
        if (key === "dynamicColor" && enabled) Theme.refreshSystemAccent(true);
        renderThemePage();
      });
    });
    content
      .querySelector('.setting-select-row[data-key="accentColor"]')
      ?.addEventListener("click", () => {
        showSettingOptionDialog(
          "强调色",
          WEBUI_ACCENT_COLOR_OPTIONS,
          Number(Theme.getUiOption("accentColor")) || 0,
          (value) => {
            Theme.setUiOption("accentColor", Number(value) || 0);
            renderThemePage();
          },
        );
      });
    content
      .querySelector('.setting-select-row[data-key="colorStyle"]')
      ?.addEventListener("click", () => {
        showSettingOptionDialog(
          "色彩风格",
          WEBUI_COLOR_STYLE_OPTIONS,
          Theme.getUiOption("colorStyle") || "TonalSpot",
          (value) => {
            Theme.setUiOption("colorStyle", value);
            renderThemePage();
          },
        );
      });
    content
      .querySelector('.setting-select-row[data-key="colorSpec"]')
      ?.addEventListener("click", () => {
        showSettingOptionDialog(
          "色彩标准",
          WEBUI_COLOR_SPEC_OPTIONS,
          Theme.getUiOption("colorSpec") || "Spec2025",
          (value) => {
            Theme.setUiOption("colorSpec", value);
            renderThemePage();
          },
        );
      });
    const scaleSlider = content.querySelector("#pageScaleSlider");
    const scaleValue = content.querySelector("#pageScaleValue");
    const scaleCard = content.querySelector("#pageScaleCard");
    const readScalePercent = () => {
      let percent = Math.max(80, Math.min(110, Number(scaleSlider?.value) || 100));
      const keyPoint = [80, 90, 100, 110].find((point) => Math.abs(point - percent) <= 1);
      if (keyPoint !== undefined) {
        percent = keyPoint;
        if (scaleSlider) scaleSlider.value = String(keyPoint);
      }
      return percent;
    };
    const applyScale = () => {
      const percent = readScalePercent();
      Theme.setUiOption("pageScale", percent / 100);
    };
    const updateScale = () => {
      if (!scaleSlider) return;
      const percent = readScalePercent();
      const progress = ((percent - 80) / 30) * 100;
      if (scaleValue) scaleValue.textContent = percent + "%";
      scaleSlider.style.setProperty("--scale-progress", progress.toFixed(2) + "%");
      scaleSlider.setAttribute("aria-valuetext", percent + "%");
      scaleCard?.setAttribute("aria-label", "手动输入界面缩放，当前 " + percent + "%");
    };
    const openScaleDialog = () => showPageScaleDialog();
    scaleCard?.addEventListener("click", openScaleDialog);
    scaleCard?.addEventListener("keydown", (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      openScaleDialog();
    });
    ["pointerdown", "click", "keydown"].forEach((eventName) => {
      scaleSlider?.addEventListener(eventName, (event) => event.stopPropagation());
    });
    scaleSlider?.addEventListener("input", updateScale);
    scaleSlider?.addEventListener("change", applyScale);
  }

  async function loadSettings() {
    const content = $("#settingsContent");
    try {
      const [globalConfig, templates] = await Promise.all([
        Api.readGlobalConfig({ force: true }),
        loadTemplates(true),
      ]);
      State.autoTemplateFallbackNoticeId = "";
      let normalizedGlobal = normalizeGlobalRuntimeConfig(globalConfig);
      const selectedTemplateId = normalizedGlobal.auto_enable_redirect_for_new_apps
        ? normalizedGlobal.auto_enable_new_apps_template_id
        : "";
      if (selectedTemplateId && !templates.some((template) => template.id === selectedTemplateId)) {
        const fallbackGlobal = normalizeGlobalRuntimeConfig(
          Object.assign({}, normalizedGlobal, {
            auto_enable_new_apps_template_id: "",
          }),
        );
        if (await Api.writeGlobalConfig(fallbackGlobal)) {
          await Api.notifyRuntimeConfigChanged().catch(() => {});
          normalizedGlobal = fallbackGlobal;
          State.autoTemplateFallbackNoticeId = selectedTemplateId;
          Theme.showToast("自动配置模板已失效，已回退", "error");
        }
      }
      State.globalConfig = normalizedGlobal;
      const templateSection =
        '<div class="section template-section settings-section"><h2 class="section-title">配置模板</h2>' +
        '<div class="template-card"><div class="config-group-header"><span class="config-group-title">模板库</span><button class="icon-btn icon-btn-sm icon-btn-add" id="templateAddBtn" type="button" aria-label="添加配置模板" title="添加配置模板">' +
        iconHtml("plus") +
        "</button></div>" +
        '<div class="template-list-scroll">' +
        (templates.length
          ? templates.map((t) => templateRowHtml(t, { manage: true })).join("")
          : '<div class="app-empty" style="padding:18px;font-size:12px">还没有配置模板，可从应用配置页保存当前配置为模板</div>') +
        "</div></div></div>";
      content.innerHTML =
        '<div class="section settings-section"><h2 class="section-title">模块设置</h2><div class="config-group">' +
        switchRow(
          "文件监视",
          "fileMonitor",
          !!State.globalConfig.file_monitor_enabled,
          "记录已配置普通应用和系统代写进程的文件创建操作，普通应用由 daemon 外部监控",
        ) +
        switchRow(
          "Fuse Fixer",
          "fuseFix",
          State.globalConfig.fuse_fix_enabled !== false,
          "SRX 内置 Fuse Fixer 兼容保护，处理特殊 Unicode 字符",
        ) +
        switchRow(
          "详细日志",
          "verboseLogging",
          State.globalConfig.verbose_logging_enabled === true,
          "开启后立即记录 Rust、Java 和诊断采集日志",
        ) +
        switchRow(
          "新应用自动重定向",
          "autoEnableNewApps",
          State.globalConfig.auto_enable_redirect_for_new_apps === true,
          "收到系统新安装事件后，自动写入默认配置",
        ) +
        autoRedirectTemplateStatusHtml(
          State.globalConfig,
          templates,
          State.autoTemplateFallbackNoticeId,
        ) +
        switchRow(
          "配置操作即时保存",
          "appConfigAutoSave",
          State.globalConfig.app_config_auto_save === true,
          "开启后应用配置页的修改会在每个操作结束后自动保存",
        ) +
        "</div></div>" +
        '<div class="section settings-section appearance-settings-section"><h2 class="section-title">外观</h2><div class="config-group">' +
        settingSelectRow(
          "主题与外观",
          "themePage",
          themePreferenceSummary(),
          "主题模式、颜色、缩放和视觉效果",
        ) +
        "</div></div>" +
        '<div class="section experiment-settings-section settings-section"><h2 class="section-title">实验区</h2>' +
        '<div class="theme-settings-card"><div class="theme-settings-rows">' +
        switchRow(
          "Fuse daemon",
          "fuseDaemonRedirect",
          State.globalConfig.fuse_daemon_redirect_enabled === true,
          "仅在普通应用的通配规则前缀启用 scoped FUSE，精确处理 !、*、?；普通路径继续使用 mount namespace。可提升复杂规则准确性，但通配前缀内的高频读写会多一层用户态转发。",
        ) +
        "</div></div></div>" +
        templateSection +
        '<div class="section backup-restore-section settings-section"><h2 class="section-title">备份还原</h2>' +
        '<div class="backup-restore-card">' +
        '<div class="backup-restore-actions">' +
        '<button class="backup-action backup-export-btn" id="backupExportBtn" type="button"><span class="icon icon-download" aria-hidden="true"></span><span>备份</span></button>' +
        '<button class="backup-action backup-import-btn" id="backupImportBtn" type="button"><span class="icon icon-upload" aria-hidden="true"></span><span>还原</span></button>' +
        "</div>" +
        '<div class="backup-restore-hint">备份会导出一个可传播的单文件，包含全局设置、所有应用配置、配置模板、文件监视过滤、外观偏好和检查更新设置；还原前会校验格式、模块标识和配置字段。</div>' +
        hiddenFileInputHtml(
          "backupRestoreFile",
          ".srxbak.zip,.srxbak.json,application/zip,application/json",
        ) +
        "</div></div>" +
        '<div class="config-group settings-path-card"><div class="switch-row"><div class="switch-label-group"><div class="switch-label">配置文件路径</div><div class="switch-hint settings-path-value">/data/adb/modules/storage.redirect.x/config/</div></div></div></div>';

      content
        .querySelector('.setting-select-row[data-key="themePage"]')
        ?.addEventListener("click", () => routeTo("theme"));
      content.querySelectorAll(".toggle").forEach((toggle) => {
        toggle.addEventListener("click", async () => {
          if (toggle.disabled) return;
          setToggleState(toggle, !toggle.classList.contains("on"));
          const key = toggle.dataset.key;
          if (key === "autoEnableNewApps") {
            await handleAutoEnableNewAppsToggle(content, toggle);
            return;
          }
          await saveGlobalSettingImmediate(content, toggle, key);
        });
      });
      bindAutoTemplateStatus(content);
      content.querySelector("#backupExportBtn")?.addEventListener("click", handleBackupExport);
      content
        .querySelector("#backupImportBtn")
        ?.addEventListener("click", () => content.querySelector("#backupRestoreFile")?.click());
      content
        .querySelector("#backupRestoreFile")
        ?.addEventListener("change", handleBackupFileSelected);
      content.querySelector("#templateAddBtn")?.addEventListener("click", () => {
        openTemplateEditor(
          {
            id: createTemplateId(),
            name: "\u65b0\u914d\u7f6e\u6a21\u677f",
            config: { users: { [String(State.currentUserId || "0")]: createDefaultProfile() } },
          },
          { originPage: "settings" },
        );
      });
      content.querySelectorAll(".template-row[data-id]").forEach((row) => {
        row.addEventListener("click", () => {
          const template = State.templates.find((item) => item.id === row.dataset.id);
          if (template) openTemplateEditor(template, { originPage: "settings" });
        });
      });
      content.querySelectorAll(".template-delete[data-id]").forEach((btn) => {
        btn.addEventListener("click", async (e) => {
          e.stopPropagation();
          const template = State.templates.find((item) => item.id === btn.dataset.id);
          if (!template) return;
          if (!(await guardTemplateDelete(template.id))) return;
          Theme.confirmDelete("删除模板“" + template.name + "”？", async () => {
            const ok = await Api.deleteTemplate(template.id);
            if (ok) State.templates = State.templates.filter((item) => item.id !== template.id);
            Theme.showToast(ok ? "模板已删除" : "删除模板失败", ok ? "success" : "error");
            if (ok) loadSettings();
          });
        });
      });
    } catch {
      content.innerHTML = '<div class="app-empty">加载设置失败</div>';
    }
  }

  function buildGlobalConfigFromSettings(content, overrides) {
    return normalizeGlobalRuntimeConfig(
      Object.assign(
        {},
        State.globalConfig || {},
        {
          file_monitor_enabled:
            content.querySelector('.toggle[data-key="fileMonitor"]')?.classList.contains("on") ??
            true,
          fuse_fix_enabled:
            content.querySelector('.toggle[data-key="fuseFix"]')?.classList.contains("on") ?? true,
          fuse_daemon_redirect_enabled:
            content
              .querySelector('.toggle[data-key="fuseDaemonRedirect"]')
              ?.classList.contains("on") === true,
          verbose_logging_enabled:
            content
              .querySelector('.toggle[data-key="verboseLogging"]')
              ?.classList.contains("on") === true,
          auto_enable_redirect_for_new_apps:
            content
              .querySelector('.toggle[data-key="autoEnableNewApps"]')
              ?.classList.contains("on") === true,
          auto_enable_new_apps_template_id:
            State.globalConfig?.auto_enable_new_apps_template_id || "",
          app_config_auto_save:
            content
              .querySelector('.toggle[data-key="appConfigAutoSave"]')
              ?.classList.contains("on") === true,
        },
        overrides || {},
      ),
    );
  }

  async function saveGlobalConfigFromSettings(content, overrides, options) {
    const config = buildGlobalConfigFromSettings(content, overrides);
    const ok = await Api.writeGlobalConfig(config);
    if (!ok) throw new Error("write failed");
    await Api.notifyRuntimeConfigChanged();
    State.globalConfig = config;
    setFeatureChipState("monitorStatusChip", !!config.file_monitor_enabled, "文件监控");
    setFeatureChipState("fuseFixStatusChip", config.fuse_fix_enabled !== false, "FuseFixer");
    setFeatureChipState(
      "verboseLogStatusChip",
      config.verbose_logging_enabled === true,
      "详细日志",
    );
    if (!options?.silent) Theme.showToast("设置已保存", "success");
    if (options?.reloadSettings) loadSettings();
    return config;
  }

  async function handleAutoEnableNewAppsToggle(content, toggle) {
    if (!toggle.classList.contains("on")) {
      await saveGlobalSettingImmediate(content, toggle, "autoEnableNewApps", {
        silent: true,
      });
      refreshAutoTemplateStatus(content);
      return;
    }
    if (State.globalConfig?.auto_enable_new_apps_template_id) {
      await saveGlobalSettingImmediate(content, toggle, "autoEnableNewApps", {
        silent: true,
      });
      refreshAutoTemplateStatus(content);
      return;
    }
    setToggleBusy(toggle, true);
    let templates = [];
    try {
      templates = await loadTemplates(true);
    } catch {
      setToggleState(toggle, false);
      setToggleBusy(toggle, false);
      Theme.showToast("加载模板失败，已恢复原状态", "error");
      return;
    }
    setToggleBusy(toggle, false);
    const restoreDisabledState = () => setToggleState(toggle, false);
    if (!templates.length) {
      showAutoTemplateEmptyEnableDialog(async () => {
        try {
          await saveGlobalConfigFromSettings(
            content,
            {
              auto_enable_redirect_for_new_apps: true,
              auto_enable_new_apps_template_id: "",
            },
            { silent: true },
          );
          refreshAutoTemplateStatus(content);
        } catch {
          setToggleState(toggle, false);
          Theme.showToast("保存失败，已恢复原状态", "error");
        }
      }, restoreDisabledState);
      return;
    }
    showAutoTemplatePickerDialog(
      "",
      async (templateId) => {
        try {
          await saveGlobalConfigFromSettings(
            content,
            {
              auto_enable_redirect_for_new_apps: true,
              auto_enable_new_apps_template_id: templateId,
            },
            { silent: true },
          );
          refreshAutoTemplateStatus(content);
        } catch {
          setToggleState(toggle, false);
          Theme.showToast("保存失败，已恢复原状态", "error");
        }
      },
      restoreDisabledState,
    );
  }

  async function saveGlobalSettingImmediate(content, toggle, key) {
    const options = arguments[3] || {};
    const previous = !toggle.classList.contains("on");
    setToggleBusy(toggle, true);
    try {
      await saveGlobalConfigFromSettings(content, null, Object.assign({ silent: true }, options));
    } catch {
      setToggleState(toggle, previous);
      Theme.showToast("保存失败，已恢复原状态", "error");
      setToggleBusy(toggle, false);
      return;
    }
    setToggleBusy(toggle, false);
  }

  function stableJson(value) {
    if (Array.isArray(value)) return "[" + value.map(stableJson).join(",") + "]";
    if (value && typeof value === "object") {
      return (
        "{" +
        Object.keys(value)
          .sort()
          .map((key) => JSON.stringify(key) + ":" + stableJson(value[key]))
          .join(",") +
        "}"
      );
    }
    const json = JSON.stringify(value);
    return typeof json === "undefined" ? "null" : json;
  }

  function isSameStringSet(left, right) {
    if (!Array.isArray(left) || !Array.isArray(right)) return false;
    return (
      left
        .map((item) => String(item || "").toLowerCase())
        .sort()
        .join("\n") ===
      right
        .map((item) => String(item || "").toLowerCase())
        .sort()
        .join("\n")
    );
  }

  function cloneForBackupDigest(value, options) {
    const copy = JSON.parse(JSON.stringify(value || {}));
    if (options?.includeAutoEnableNewApps === false && copy.global) {
      delete copy.global.auto_enable_redirect_for_new_apps;
      delete copy.global.auto_enable_new_apps_template_id;
    }
    if (options?.includeAutoEnableNewAppsTemplateId === false && copy.global) {
      delete copy.global.auto_enable_new_apps_template_id;
    }
    if (options?.includeVerboseLogging === false && copy.global) {
      delete copy.global.verbose_logging_enabled;
    }
    if (options?.includeUiPreferences === false) {
      delete copy.ui;
    }
    if (options?.includeTemplates === false) {
      delete copy.templates;
    }
    if (options?.includeMonitorFilters === false) {
      delete copy.monitor_filters;
    }
    if (options?.legacyDefaultMonitorOperations === true && copy.monitor_filters) {
      const ops = copy.monitor_filters.excluded_operations;
      const isCurrentDefault = isSameStringSet(ops, DEFAULT_MONITOR_FILTER_OPERATIONS);
      if (isCurrentDefault)
        copy.monitor_filters.excluded_operations = LEGACY_DEFAULT_MONITOR_FILTER_OPERATIONS.slice();
    }
    if (options?.legacyFullDefaultMonitorOperations === true && copy.monitor_filters) {
      const ops = copy.monitor_filters.excluded_operations;
      const isCurrentDefault = isSameStringSet(ops, DEFAULT_MONITOR_FILTER_OPERATIONS);
      if (isCurrentDefault)
        copy.monitor_filters.excluded_operations =
          LEGACY_FULL_DEFAULT_MONITOR_FILTER_OPERATIONS.slice();
    }
    return copy;
  }

  function utf8Bytes(value) {
    if (typeof TextEncoder !== "undefined") return new TextEncoder().encode(String(value));
    const encoded = unescape(encodeURIComponent(String(value)));
    const bytes = new Uint8Array(encoded.length);
    for (let i = 0; i < encoded.length; i++) bytes[i] = encoded.charCodeAt(i);
    return bytes;
  }

  function fnv32Hex(value) {
    const bytes = utf8Bytes(value);
    let hash = 0x811c9dc5;
    bytes.forEach((byte) => {
      hash ^= byte;
      hash = Math.imul(hash, 0x01000193) >>> 0;
    });
    return "fnv32-" + hash.toString(16).padStart(8, "0");
  }

  async function sha256Hex(value) {
    const bytes = utf8Bytes(value);
    const bitLength = bytes.length * 8;
    const paddedLength = ((bytes.length + 9 + 63) >> 6) << 6;
    const padded = new Uint8Array(paddedLength);
    const words = new Uint32Array(64);
    const h = new Uint32Array([
      0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
      0x5be0cd19,
    ]);
    const k = new Uint32Array([
      0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
      0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
      0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
      0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
      0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
      0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
      0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
      0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
      0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
      0xc67178f2,
    ]);
    const rotr = (value, bits) => (value >>> bits) | (value << (32 - bits));
    padded.set(bytes);
    padded[bytes.length] = 0x80;
    const high = Math.floor(bitLength / 0x100000000);
    const low = bitLength >>> 0;
    padded[paddedLength - 8] = (high >>> 24) & 0xff;
    padded[paddedLength - 7] = (high >>> 16) & 0xff;
    padded[paddedLength - 6] = (high >>> 8) & 0xff;
    padded[paddedLength - 5] = high & 0xff;
    padded[paddedLength - 4] = (low >>> 24) & 0xff;
    padded[paddedLength - 3] = (low >>> 16) & 0xff;
    padded[paddedLength - 2] = (low >>> 8) & 0xff;
    padded[paddedLength - 1] = low & 0xff;
    for (let offset = 0; offset < paddedLength; offset += 64) {
      for (let i = 0; i < 16; i++) {
        const j = offset + i * 4;
        words[i] =
          ((padded[j] << 24) | (padded[j + 1] << 16) | (padded[j + 2] << 8) | padded[j + 3]) >>> 0;
      }
      for (let i = 16; i < 64; i++) {
        const s0 = (rotr(words[i - 15], 7) ^ rotr(words[i - 15], 18) ^ (words[i - 15] >>> 3)) >>> 0;
        const s1 = (rotr(words[i - 2], 17) ^ rotr(words[i - 2], 19) ^ (words[i - 2] >>> 10)) >>> 0;
        words[i] = (words[i - 16] + s0 + words[i - 7] + s1) >>> 0;
      }
      let [a, b, c, d, e, f, g, hh] = h;
      for (let i = 0; i < 64; i++) {
        const s1 = (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) >>> 0;
        const ch = ((e & f) ^ (~e & g)) >>> 0;
        const temp1 = (hh + s1 + ch + k[i] + words[i]) >>> 0;
        const s0 = (rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) >>> 0;
        const maj = ((a & b) ^ (a & c) ^ (b & c)) >>> 0;
        const temp2 = (s0 + maj) >>> 0;
        hh = g;
        g = f;
        f = e;
        e = (d + temp1) >>> 0;
        d = c;
        c = b;
        b = a;
        a = (temp1 + temp2) >>> 0;
      }
      h[0] = (h[0] + a) >>> 0;
      h[1] = (h[1] + b) >>> 0;
      h[2] = (h[2] + c) >>> 0;
      h[3] = (h[3] + d) >>> 0;
      h[4] = (h[4] + e) >>> 0;
      h[5] = (h[5] + f) >>> 0;
      h[6] = (h[6] + g) >>> 0;
      h[7] = (h[7] + hh) >>> 0;
    }
    return Array.from(h)
      .map((word) => word.toString(16).padStart(8, "0"))
      .join("");
  }

  async function createBackupIntegrity(canonical) {
    return { algorithm: "SHA-256", value: await sha256Hex(canonical) };
  }

  async function verifyBackupDigest(canonical, integrity) {
    const algorithm = String(integrity?.algorithm || "")
      .trim()
      .toUpperCase();
    if (algorithm === "SHA-256") return await sha256Hex(canonical);
    if (algorithm === "FNV-1A-32") return fnv32Hex(canonical);
    throw new Error("备份校验算法不支持");
  }

  function normalizeBackupGlobalConfig(raw) {
    const source = raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
    return {
      file_monitor_enabled: source.file_monitor_enabled === true,
      fuse_fix_enabled: source.fuse_fix_enabled !== false,
      fuse_daemon_redirect_enabled: source.fuse_daemon_redirect_enabled === true,
      verbose_logging_enabled: source.verbose_logging_enabled === true,
      auto_enable_redirect_for_new_apps: source.auto_enable_redirect_for_new_apps === true,
      auto_enable_new_apps_template_id: isSafeTemplateId(source.auto_enable_new_apps_template_id)
        ? String(source.auto_enable_new_apps_template_id)
        : "",
      app_config_auto_save: source.app_config_auto_save === true,
    };
  }

  function normalizeBackupUiPreferences(raw) {
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
    const ui = {};
    const has = (key) => Object.prototype.hasOwnProperty.call(raw, key);
    if (Object.prototype.hasOwnProperty.call(raw, "predictive_back")) {
      ui.predictive_back = raw.predictive_back === true;
    }
    if (has("floating_bottom_bar")) ui.floating_bottom_bar = raw.floating_bottom_bar !== false;
    if (has("liquid_glass")) ui.liquid_glass = raw.liquid_glass !== false;
    if (has("blur_effect")) ui.blur_effect = raw.blur_effect !== false;
    if (has("dynamic_color")) ui.dynamic_color = raw.dynamic_color === true;
    if (has("accent_color")) {
      const color = normalizeBackupAccentColor(raw.accent_color);
      if (color !== null) ui.accent_color = color;
    }
    if (has("color_style")) {
      const style = normalizeBackupColorStyle(raw.color_style);
      if (style) ui.color_style = style;
    }
    if (has("color_spec")) {
      const spec = normalizeBackupColorSpec(raw.color_spec);
      if (spec) ui.color_spec = spec;
    }
    if (has("theme_mode")) {
      const mode = normalizeBackupThemeMode(raw.theme_mode);
      if (mode) ui.theme_mode = mode;
    }
    if (has("page_scale")) {
      const scale = normalizeBackupPageScale(raw.page_scale);
      if (scale !== null) ui.page_scale = scale;
    }
    if (has("auto_check_updates")) {
      ui.auto_check_updates = raw.auto_check_updates !== false;
    }
    if (has("update_channel")) {
      const channel = String(raw.update_channel || "");
      if (UPDATE_CHANNEL_OPTIONS.some((item) => item.value === channel))
        ui.update_channel = channel;
    }
    return Object.keys(ui).length ? ui : null;
  }

  function normalizeBackupThemeMode(value) {
    const raw = String(value || "").trim();
    const lower = raw.toLowerCase();
    return { light: "Light", dark: "Dark", system: "System" }[lower] || null;
  }

  function backupThemeModeToWebValue(value) {
    const mode = normalizeBackupThemeMode(value);
    return mode ? mode.toLowerCase() : null;
  }

  function normalizeBackupColorStyle(value) {
    const raw = String(value || "").trim();
    const lower = raw.toLowerCase();
    return (
      WEBUI_COLOR_STYLE_OPTIONS.find((item) => item.value.toLowerCase() === lower)?.value || null
    );
  }

  function normalizeBackupColorSpec(value) {
    const raw = String(value || "").trim();
    const lower = raw.toLowerCase();
    return (
      WEBUI_COLOR_SPEC_OPTIONS.find((item) => item.value.toLowerCase() === lower)?.value || null
    );
  }

  function normalizeBackupAccentColor(value) {
    const number = Number(value);
    if (!Number.isFinite(number) || !Number.isInteger(number)) return null;
    if (number < -2147483648 || number > 4294967295) return null;
    const unsigned = number >>> 0;
    if (unsigned === 0) return 0;
    return unsigned > 0x7fffffff ? unsigned - 0x100000000 : unsigned;
  }

  function backupAccentColorToWebValue(value) {
    const signed = normalizeBackupAccentColor(value);
    return signed === null ? null : signed >>> 0;
  }

  function normalizeBackupPageScale(value) {
    const scale = Number(value);
    if (!Number.isFinite(scale)) return null;
    return Math.max(WEBUI_PAGE_SCALE_MIN, Math.min(WEBUI_PAGE_SCALE_MAX, scale));
  }

  function buildBackupUiPreferences() {
    const prefs = updatePrefs();
    const themeMode = normalizeBackupThemeMode(Theme.get());
    const uiOptions = Theme.getUiOptions();
    return {
      floating_bottom_bar: uiOptions.floatingNav !== false,
      liquid_glass: uiOptions.liquidGlass !== false,
      blur_effect: uiOptions.blurEffect !== false,
      dynamic_color: uiOptions.dynamicColor === true,
      accent_color: normalizeBackupAccentColor(uiOptions.accentColor) || 0,
      color_style: normalizeBackupColorStyle(uiOptions.colorStyle) || "TonalSpot",
      color_spec: normalizeBackupColorSpec(uiOptions.colorSpec) || "Spec2025",
      theme_mode: themeMode || "Light",
      page_scale: normalizeBackupPageScale(uiOptions.pageScale) || 1,
      auto_check_updates: prefs.autoCheckUpdates,
      update_channel: prefs.updateChannel,
    };
  }

  function restoreBackupUiPreferences(ui) {
    if (!ui || typeof ui !== "object") return;
    const has = (key) => Object.prototype.hasOwnProperty.call(ui, key);
    if (has("theme_mode")) {
      const mode = backupThemeModeToWebValue(ui.theme_mode);
      if (mode) Theme.apply(mode);
    }
    const currentUiOptions = Theme.getUiOptions();
    const nextUiOptions = Object.assign({}, currentUiOptions);
    let uiOptionsChanged = false;
    if (has("floating_bottom_bar")) {
      nextUiOptions.floatingNav = ui.floating_bottom_bar !== false;
      uiOptionsChanged = true;
    }
    if (has("liquid_glass")) {
      nextUiOptions.liquidGlass = ui.liquid_glass !== false;
      uiOptionsChanged = true;
    }
    if (has("blur_effect")) {
      nextUiOptions.blurEffect = ui.blur_effect !== false;
      uiOptionsChanged = true;
    }
    if (has("dynamic_color")) {
      nextUiOptions.dynamicColor = ui.dynamic_color === true;
      uiOptionsChanged = true;
    }
    if (has("accent_color")) {
      const color = backupAccentColorToWebValue(ui.accent_color);
      if (color !== null) {
        nextUiOptions.accentColor = color;
        uiOptionsChanged = true;
      }
    }
    if (has("color_style")) {
      const style = normalizeBackupColorStyle(ui.color_style);
      if (style) {
        nextUiOptions.colorStyle = style;
        uiOptionsChanged = true;
      }
    }
    if (has("color_spec")) {
      const spec = normalizeBackupColorSpec(ui.color_spec);
      if (spec) {
        nextUiOptions.colorSpec = spec;
        uiOptionsChanged = true;
      }
    }
    if (has("page_scale")) {
      const scale = normalizeBackupPageScale(ui.page_scale);
      if (scale !== null) {
        nextUiOptions.pageScale = scale;
        uiOptionsChanged = true;
      }
    }
    if (uiOptionsChanged) Theme.setUiOptions(nextUiOptions);
    const current = updatePrefs();
    const next = Object.assign({}, current);
    let changed = false;
    if (has("auto_check_updates")) {
      next.autoCheckUpdates = ui.auto_check_updates !== false;
      changed = true;
    }
    if (has("update_channel")) {
      next.updateChannel = updateChannelValue(ui.update_channel);
      changed = true;
    }
    if (changed) saveUpdatePrefs(next);
  }

  function isBackupPackageName(value) {
    return /^[A-Za-z0-9_.-]+$/.test(String(value || ""));
  }

  function isBackupUserId(value) {
    return /^[0-9]+$/.test(String(value || ""));
  }

  function sanitizeBackupPath(raw, options) {
    const allowRuleSyntax = !!options?.allowRuleSyntax;
    const allowWildcards = allowRuleSyntax || !!options?.allowWildcards;
    const text = String(raw ?? "")
      .trim()
      .replace(/\\/g, "/")
      .replace(/\/+/g, "/");
    const excluded = allowRuleSyntax && text.startsWith("!");
    let path = excluded ? text.slice(1).trim() : text;
    if (!path || path.length > 512 || path.includes("\0")) return "";
    path = path
      .replace(/^\/+/, "")
      .replace(/^storage\/emulated\/\d+\/?/i, "")
      .replace(/^data\/media\/\d+\/?/i, "")
      .replace(/^sdcard\//i, "")
      .replace(/^\/+|\/+$/g, "");
    if (!path || path.length > 512 || hasMonitorFilterStorageRootPrefix(path)) return "";
    path = path.replace(/\/+$/g, "");
    const validation = validatePath((excluded ? "!" : "") + path, {
      allowRuleSyntax,
      allowWildcards,
    });
    if (!validation.valid) return "";
    return (excluded ? "!" : "") + path;
  }

  function pushUniquePath(target, value) {
    if (!value || target.includes(value)) return;
    target.push(value);
  }

  function normalizeBackupMappings(raw) {
    const mappings = {};
    const entries = Array.isArray(raw)
      ? raw.map((item) => [item?.request_path, item?.final_path])
      : Object.entries(raw && typeof raw === "object" ? raw : {});
    entries.forEach(([reqRaw, targetRaw]) => {
      const req = sanitizeBackupPath(reqRaw);
      const target = sanitizeBackupPath(targetRaw);
      if (!req || !target || req === target) return;
      if (isAndroidDataOrObbPath(target)) return;
      mappings[req] = target;
    });
    return sortPathMappings(filterValidBackupMappingChains(mappings));
  }

  const MAX_PATH_MAPPING_DEPTH = 10;

  function filterValidBackupMappingChains(mappings) {
    const cycles = detectBackupMappingCycles(mappings);
    const depths = detectBackupMappingDepths(mappings);
    const invalid = new Set(cycles);
    Object.entries(depths).forEach(([source, depth]) => {
      if (depth > MAX_PATH_MAPPING_DEPTH) invalid.add(source);
    });
    if (!invalid.size) return mappings;
    return Object.entries(mappings).reduce((out, [source, target]) => {
      if (!invalid.has(source)) out[source] = target;
      return out;
    }, {});
  }

  function detectBackupMappingCycles(mappings) {
    const cycles = new Set();
    const visitState = new Map();
    const stack = [];
    const visit = (source) => {
      const state = visitState.get(source);
      if (state === 1) {
        const index = stack.indexOf(source);
        if (index >= 0) stack.slice(index).forEach((item) => cycles.add(item));
        return;
      }
      if (state === 2) return;
      visitState.set(source, 1);
      stack.push(source);
      if (Object.prototype.hasOwnProperty.call(mappings, source)) {
        visit(mappings[source]);
      }
      stack.pop();
      visitState.set(source, 2);
    };
    Object.keys(mappings).forEach(visit);
    return cycles;
  }

  function detectBackupMappingDepths(mappings) {
    const depths = {};
    const compute = (source, visiting) => {
      if (visiting.has(source)) return MAX_PATH_MAPPING_DEPTH + 1;
      if (Object.prototype.hasOwnProperty.call(depths, source)) return depths[source];
      if (!Object.prototype.hasOwnProperty.call(mappings, source)) return 0;
      const next = new Set(visiting);
      next.add(source);
      const depth = 1 + compute(mappings[source], next);
      depths[source] = depth;
      return depth;
    };
    Object.keys(mappings).forEach((source) => {
      if (!Object.prototype.hasOwnProperty.call(depths, source)) compute(source, new Set());
    });
    return depths;
  }

  function normalizeBackupUserProfile(raw) {
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
      return { profile: null, warnings: ["用户配置不是对象"] };
    }
    const warnings = [];
    const profile = createDefaultProfile({ enabledDefault: true });
    profile.enabled = raw.enabled !== false;
    profile.mapping_mode_only = raw.mapping_mode_only === true;
    const allowRules = [];
    if (Array.isArray(raw.allowed_real_paths)) {
      raw.allowed_real_paths.forEach((item) =>
        pushUniquePath(allowRules, sanitizeBackupPath(item, { allowRuleSyntax: true })),
      );
    } else if (raw.allowed_real_paths != null) {
      warnings.push("allowed_real_paths 不是数组，已忽略");
    }
    if (Array.isArray(raw.excluded_real_paths)) {
      raw.excluded_real_paths.forEach((item) => {
        const path = sanitizeBackupPath(String(item ?? "").replace(/^!/, ""), {
          allowRuleSyntax: true,
        });
        if (path) pushUniquePath(allowRules, path.startsWith("!") ? path : "!" + path);
      });
    } else if (raw.excluded_real_paths != null) {
      warnings.push("excluded_real_paths 不是数组，已忽略");
    }
    const readOnlyPaths = [];
    const rawReadOnly = Array.isArray(raw.read_only_paths)
      ? raw.read_only_paths
      : typeof raw.read_only_paths === "string"
        ? [raw.read_only_paths]
        : [];
    if (
      raw.read_only_paths != null &&
      !Array.isArray(raw.read_only_paths) &&
      typeof raw.read_only_paths !== "string"
    ) {
      warnings.push("read_only_paths 格式不支持，已忽略");
    }
    rawReadOnly.forEach((item) =>
      pushUniquePath(
        readOnlyPaths,
        sanitizeBackupPath(item, { allowRuleSyntax: true, allowWildcards: true }),
      ),
    );
    profile.allowed_real_paths = sortPathRules(
      removeConflictingAllowedRules(mergeAllowedRules(allowRules, [])),
    );
    profile.excluded_real_paths = [];
    profile.read_only_paths = normalizeReadOnlyForRules(
      normalizeReadOnlyRules(readOnlyPaths),
      profile.allowed_real_paths,
    );
    const sandboxed = [];
    const rawSandboxed = Array.isArray(raw.sandboxed_paths)
      ? raw.sandboxed_paths
      : typeof raw.sandboxed_paths === "string"
        ? [raw.sandboxed_paths]
        : [];
    if (
      raw.sandboxed_paths != null &&
      !Array.isArray(raw.sandboxed_paths) &&
      typeof raw.sandboxed_paths !== "string"
    ) {
      warnings.push("sandboxed_paths 格式不支持，已忽略");
    }
    rawSandboxed.forEach((item) => pushUniquePath(sandboxed, sanitizeBackupPath(item)));
    profile.sandboxed_paths = sortPathRules(sandboxed);
    profile.path_mappings = normalizeBackupMappings(raw.path_mappings);
    return { profile, warnings };
  }

  function normalizeBackupAppConfig(packageName, raw) {
    if (!isBackupPackageName(packageName)) return { config: null, warnings: ["包名非法"] };
    if (
      !raw ||
      typeof raw !== "object" ||
      Array.isArray(raw) ||
      !raw.users ||
      typeof raw.users !== "object" ||
      Array.isArray(raw.users)
    ) {
      return { config: null, warnings: ["缺少 users 对象"] };
    }
    const users = {};
    const warnings = [];
    Object.keys(raw.users)
      .sort((a, b) => Number(a) - Number(b))
      .forEach((userId) => {
        if (!isBackupUserId(userId)) {
          warnings.push("跳过非法用户 " + userId);
          return;
        }
        const normalized = normalizeBackupUserProfile(raw.users[userId]);
        if (normalized.profile) users[userId] = normalized.profile;
        normalized.warnings.forEach((item) => warnings.push("用户 " + userId + "：" + item));
      });
    if (!Object.keys(users).length)
      return { config: null, warnings: warnings.concat("没有有效用户配置") };
    return { config: { users }, warnings };
  }

  function normalizeBackupApps(rawApps) {
    const apps = {};
    const warnings = [];
    const source = rawApps && typeof rawApps === "object" && !Array.isArray(rawApps) ? rawApps : {};
    Object.keys(source)
      .sort((a, b) => a.localeCompare(b))
      .slice(0, BACKUP_MAX_APPS + 1)
      .forEach((packageName) => {
        if (Object.keys(apps).length >= BACKUP_MAX_APPS) {
          warnings.push("应用配置超过 " + BACKUP_MAX_APPS + " 个，已截断");
          return;
        }
        const normalized = normalizeBackupAppConfig(packageName, source[packageName]);
        if (normalized.config) apps[packageName] = normalized.config;
        normalized.warnings.forEach((item) => warnings.push(packageName + "：" + item));
      });
    return { apps, warnings };
  }

  function createTemplateId() {
    const random = Math.random().toString(36).slice(2, 10);
    return "tpl-" + Date.now().toString(36) + "-" + random;
  }

  function isSafeTemplateId(value) {
    return /^[A-Za-z0-9_.-]{1,80}$/.test(String(value || ""));
  }

  function normalizeTemplateName(value) {
    return String(value || "")
      .trim()
      .replace(/\s+/g, " ")
      .slice(0, 48);
  }

  function normalizeTemplate(raw) {
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
    const id = isSafeTemplateId(raw.id) ? String(raw.id) : createTemplateId();
    const name = normalizeTemplateName(raw.name);
    if (!name) return null;
    const normalized = normalizeBackupAppConfig(
      "template.source",
      raw.config || raw.app_config || raw,
    );
    if (!normalized.config) return null;
    return { id, name, config: normalized.config };
  }

  function normalizeTemplates(rawTemplates) {
    const templates = [];
    const seen = new Set();
    (Array.isArray(rawTemplates) ? rawTemplates : []).forEach((raw) => {
      const template = normalizeTemplate(raw);
      if (!template || seen.has(template.id)) return;
      seen.add(template.id);
      templates.push(template);
    });
    templates.sort(
      (a, b) => a.name.localeCompare(b.name, "zh-Hans-CN") || a.id.localeCompare(b.id),
    );
    return templates;
  }

  function normalizeBackupMonitorFilters(raw) {
    const defaults = {
      excluded_paths: ["Android/data"],
      excluded_operations: DEFAULT_MONITOR_FILTER_OPERATIONS.slice(),
    };
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) return defaults;
    return {
      excluded_paths: normalizeMonitorFilterPathList(raw.excluded_paths),
      excluded_operations: normalizeMonitorFilterOperationList(raw.excluded_operations),
    };
  }

  async function loadTemplates(force) {
    if (!force && Array.isArray(State.templates) && State.templates.length) return State.templates;
    State.templates = normalizeTemplates(
      await Api.readTemplates(force ? { force: true } : undefined),
    );
    return State.templates;
  }

  async function saveTemplates(templates) {
    const normalized = normalizeTemplates(templates);
    const ok = await Api.writeTemplates(normalized);
    if (ok) State.templates = normalized;
    return ok;
  }

  async function upsertTemplate(template) {
    const templates = normalizeTemplates(await Api.readTemplates());
    const normalized = normalizeTemplate(template);
    if (!normalized) return false;
    const index = templates.findIndex((item) => item.id === normalized.id);
    if (index >= 0) templates[index] = normalized;
    else templates.push(normalized);
    return await saveTemplates(templates);
  }

  function templateSummary(config) {
    return Object.keys((config && config.users) || {}).length + " 个用户配置";
  }

  function templateRowHtml(template, options) {
    const actions = options?.manage
      ? '<div class="template-row-actions"><button class="icon-btn icon-btn-sm icon-btn-danger template-delete" type="button" data-id="' +
        escapeHtml(template.id) +
        '" aria-label="删除模板" title="删除模板"><span class="icon icon-trash" aria-hidden="true"></span></button></div>'
      : "";
    return (
      '<div class="template-row" data-id="' +
      escapeHtml(template.id) +
      '"><span class="template-row-icon"><span class="icon icon-template" aria-hidden="true"></span></span><div class="template-row-main"><div class="template-row-title">' +
      escapeHtml(template.name) +
      '</div><div class="template-row-subtitle">' +
      escapeHtml(templateSummary(template.config)) +
      "</div></div>" +
      actions +
      "</div>"
    );
  }

  function showTemplateNameDialog(title, initialName, onConfirm) {
    showModalWithHistory(
      '<div class="modal-title">' +
        escapeHtml(title) +
        "</div>" +
        '<input type="text" class="modal-input" id="templateNameInput" placeholder="模板名称" maxlength="48" autocomplete="off" value="' +
        escapeHtml(initialName || "") +
        '">' +
        '<div class="modal-hint">名称用于在应用配置页和批量操作中识别模板。</div>' +
        '<div class="modal-actions"><button class="btn btn-secondary modal-close" type="button">取消</button><button class="btn btn-primary" id="templateNameConfirm" type="button">保存</button></div>',
    );
    const input = document.getElementById("templateNameInput");
    focusWithoutViewportJump(input);
    document.getElementById("templateNameConfirm")?.addEventListener("click", async () => {
      const name = normalizeTemplateName(input?.value);
      if (!name) {
        input?.classList.add("invalid");
        return;
      }
      closeActiveModal();
      await onConfirm(name);
    });
  }

  async function showTemplatePickerDialog(title, onPick) {
    const templates = await loadTemplates(true);
    const body = templates.length
      ? '<div class="template-card template-list-scroll template-picker-list">' +
        templates.map((t) => templateRowHtml(t)).join("") +
        "</div>"
      : '<div class="app-empty" style="padding:24px 8px">还没有配置模板</div>';
    showModalWithHistory('<div class="modal-title">' + escapeHtml(title) + "</div>" + body, {
      backdropClose: true,
    });
    document.querySelectorAll("#modalContent .template-row[data-id]").forEach((row) => {
      row.addEventListener("click", async () => {
        const template = templates.find((item) => item.id === row.dataset.id);
        if (!template) return;
        closeActiveModal();
        await onPick(template);
      });
    });
  }

  async function saveCurrentConfigAsTemplate(packageName, users) {
    collectCurrentProfile(users);
    showTemplateNameDialog("保存为模板", "", async (name) => {
      const ok = await upsertTemplate({
        id: createTemplateId(),
        name,
        config: { users: JSON.parse(JSON.stringify(users || {})) },
      });
      Theme.showToast(ok ? "模板已保存" : "保存模板失败", ok ? "success" : "error");
      if (State.currentPage === "settings") loadSettings();
    });
  }

  async function applyTemplateToPackages(template, packageNames) {
    const targets = (packageNames || []).filter(isBackupPackageName);
    if (!template?.config || !targets.length) return false;
    const loading = Theme.showLoadingDialog("正在应用模板到 " + targets.length + " 个应用...");
    try {
      const configs = targets.reduce((out, packageName) => {
        out[packageName] = template.config;
        return out;
      }, {});
      const ok = await Api.writeAppConfigs(configs, {
        onProgress(done, total) {
          loading.setMessage("正在应用模板 " + Math.min(done + 1, total) + " / " + total);
        },
      });
      if (ok)
        targets.forEach((packageName) => {
          const profile = template.config.users?.[String(State.currentUserId || "0")];
          updateCachedAppConfigured(packageName, true, profile?.enabled === true);
        });
      Theme.showToast(ok ? "模板已应用" : "部分应用写入失败", ok ? "success" : "error");
      return ok;
    } finally {
      loading.close();
    }
  }

  async function buildBackupPayload() {
    const [globalConfig, configuredConfigs, templatesRaw, monitorFilters, version] =
      await Promise.all([
        Api.readGlobalConfig(),
        Api.readConfiguredAppConfigs({ force: true }),
        Api.readTemplates(),
        Api.readMonitorFilters({ force: true }),
        Api.getModuleVersion(),
      ]);
    const apps = {};
    (configuredConfigs || []).forEach((item) => {
      const packageName = item?.packageName || "";
      if (!isBackupPackageName(packageName) || !item?.config) return;
      const normalized = normalizeBackupAppConfig(packageName, item.config);
      if (normalized.config) apps[packageName] = normalized.config;
    });
    const data = {
      global: normalizeBackupGlobalConfig(globalConfig),
      apps,
      templates: normalizeTemplates(templatesRaw),
      monitor_filters: normalizeBackupMonitorFilters(monitorFilters),
      ui: buildBackupUiPreferences(),
    };
    const canonical = stableJson(data);
    const integrity = await createBackupIntegrity(canonical);
    return {
      magic: BACKUP_MAGIC,
      schema: BACKUP_SCHEMA_VERSION,
      module: {
        id: BACKUP_MODULE_ID,
        version: version || "",
      },
      createdAt: new Date().toISOString(),
      summary: {
        appCount: Object.keys(apps).length,
        userCount: Object.values(apps).reduce(
          (sum, cfg) => sum + Object.keys(cfg.users || {}).length,
          0,
        ),
        templateCount: data.templates.length,
      },
      integrity,
      data,
    };
  }

  function buildBackupFileName() {
    const stamp = new Date()
      .toISOString()
      .slice(0, 19)
      .replace(/[-:T]/g, "")
      .replace(/^(\d{8})(\d{6})$/, "$1-$2");
    return "storage-redirect-x-backup-" + stamp + BACKUP_FILE_SUFFIX;
  }

  function buildDiagnosticArchiveFileName() {
    const stamp = new Date()
      .toISOString()
      .slice(0, 19)
      .replace(/[-:T]/g, "")
      .replace(/^(\d{8})(\d{6})$/, "$1-$2");
    return "storage-redirect-x-logs-" + stamp + ".tar.gz";
  }

  function writeZipU16(out, value) {
    out.push(value & 0xff, (value >>> 8) & 0xff);
  }

  function writeZipU32(out, value) {
    out.push(value & 0xff, (value >>> 8) & 0xff, (value >>> 16) & 0xff, (value >>> 24) & 0xff);
  }

  function crc32(bytes) {
    let crc = 0xffffffff;
    for (let i = 0; i < bytes.length; i++) {
      crc ^= bytes[i];
      for (let bit = 0; bit < 8; bit++) {
        crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
      }
    }
    return (crc ^ 0xffffffff) >>> 0;
  }

  function buildSingleFileZip(fileName, contentBytes) {
    const nameBytes = utf8Bytes(fileName);
    const crc = crc32(contentBytes);
    const local = [];
    writeZipU32(local, 0x04034b50);
    writeZipU16(local, 20);
    writeZipU16(local, 0x0800);
    writeZipU16(local, 0);
    writeZipU16(local, 0);
    writeZipU16(local, 0);
    writeZipU32(local, crc);
    writeZipU32(local, contentBytes.length);
    writeZipU32(local, contentBytes.length);
    writeZipU16(local, nameBytes.length);
    writeZipU16(local, 0);
    local.push(...nameBytes, ...contentBytes);

    const centralOffset = local.length;
    const central = [];
    writeZipU32(central, 0x02014b50);
    writeZipU16(central, 20);
    writeZipU16(central, 20);
    writeZipU16(central, 0x0800);
    writeZipU16(central, 0);
    writeZipU16(central, 0);
    writeZipU16(central, 0);
    writeZipU32(central, crc);
    writeZipU32(central, contentBytes.length);
    writeZipU32(central, contentBytes.length);
    writeZipU16(central, nameBytes.length);
    writeZipU16(central, 0);
    writeZipU16(central, 0);
    writeZipU16(central, 0);
    writeZipU16(central, 0);
    writeZipU32(central, 0);
    writeZipU32(central, 0);
    central.push(...nameBytes);

    const end = [];
    writeZipU32(end, 0x06054b50);
    writeZipU16(end, 0);
    writeZipU16(end, 0);
    writeZipU16(end, 1);
    writeZipU16(end, 1);
    writeZipU32(end, central.length);
    writeZipU32(end, centralOffset);
    writeZipU16(end, 0);

    return new Uint8Array([...local, ...central, ...end]);
  }

  function buildBackupZipBytes(text) {
    return buildSingleFileZip(BACKUP_ZIP_ENTRY, utf8Bytes(text));
  }

  function uint8ToBase64(bytes) {
    let binary = "";
    const chunkSize = 0x8000;
    for (let i = 0; i < bytes.length; i += chunkSize) {
      binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunkSize));
    }
    return btoa(binary);
  }

  function downloadBackupBytes(fileName, bytes) {
    const blob = new Blob([bytes], { type: "application/zip" });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = fileName;
    anchor.type = "application/zip";
    anchor.style.display = "none";
    document.body.appendChild(anchor);
    anchor.click();
    setTimeout(() => {
      URL.revokeObjectURL(url);
      anchor.remove();
    }, 30000);
  }

  function createAbortError() {
    try {
      return new DOMException("The user aborted a request.", "AbortError");
    } catch {
      const error = new Error("The user aborted a request.");
      error.name = "AbortError";
      return error;
    }
  }

  function isPublicStoragePath(path) {
    return /^\/(?:storage\/emulated\/[0-9]+|sdcard)(?:\/|$)/.test(String(path || ""));
  }

  function normalizeBackupDirectoryPath(path) {
    const value = String(path || "")
      .trim()
      .replace(/\\/g, "/")
      .replace(/\/+/g, "/")
      .replace(/\/+$/g, "");
    if (!value) return BACKUP_STORAGE_BASE;
    if (!isPublicStoragePath(value)) {
      const relative = value.replace(/^\/+/g, "");
      if (!validatePath(relative).valid) return "";
      return BACKUP_STORAGE_BASE + "/" + relative;
    }
    if (value.split("/").some((part) => part === "." || part === "..")) return "";
    if (/[<>:"|?*\x00-\x1f]/.test(value)) return "";
    return value || BACKUP_DEFAULT_DIR;
  }

  function isKernelSuWebUi() {
    return typeof ksu !== "undefined" && typeof ksu.exec === "function";
  }

  function shouldUseBrowserSavePicker() {
    return !isKernelSuWebUi();
  }

  async function requestBackupSaveHandle(fileName) {
    if (shouldUseBrowserSavePicker() && window.showSaveFilePicker) {
      try {
        return await window.showSaveFilePicker({
          suggestedName: fileName,
          types: [
            { description: "Storage Redirect X Backup", accept: { "application/zip": [".zip"] } },
          ],
        });
      } catch (error) {
        if (error?.name === "AbortError") throw error;
        console.warn("[backup] showSaveFilePicker unavailable:", error);
      }
    }
    return null;
  }

  async function requestBackupDirectoryHandle() {
    if (!shouldUseBrowserSavePicker() || !window.showDirectoryPicker) return null;
    try {
      return await window.showDirectoryPicker({ mode: "readwrite" });
    } catch (error) {
      if (error?.name === "AbortError") throw error;
      console.warn("[backup] showDirectoryPicker unavailable:", error);
      return null;
    }
  }

  async function writeBackupToFileHandle(fileHandle, bytes) {
    if (!fileHandle) return false;
    const writable = await fileHandle.createWritable();
    try {
      await writable.write(bytes);
    } finally {
      await writable.close();
    }
    return true;
  }

  async function saveBackupWithDirectoryHandle(dirHandle, fileName, bytes) {
    if (!dirHandle) return false;
    const fileHandle = await dirHandle.getFileHandle(fileName, { create: true });
    await writeBackupToFileHandle(fileHandle, bytes);
    return true;
  }

  function showBackupDirectoryDialog(fileName) {
    return new Promise((resolve) => {
      showModalWithHistory(
        '<div class="modal-title">选择备份保存目录</div>' +
          '<input type="text" class="modal-input" id="backupDirInput" placeholder="Download" autocomplete="off" value="' +
          escapeHtml(BACKUP_DEFAULT_DIR) +
          '">' +
          '<div class="path-validation" id="backupDirValidation"></div>' +
          '<div id="backupDirBrowserContainer"></div>' +
          '<div class="modal-hint">备份文件名：' +
          escapeHtml(fileName) +
          "。相对路径以 " +
          BACKUP_STORAGE_BASE +
          " 为根。</div>" +
          '<div class="modal-actions"><button class="btn btn-secondary modal-close" type="button">取消</button><button class="btn btn-primary" id="backupDirConfirm" type="button">保存</button></div>',
      );

      const input = document.getElementById("backupDirInput");
      const hint = document.getElementById("backupDirValidation");
      const container = document.getElementById("backupDirBrowserContainer");
      const browser = buildPathBrowser(
        BACKUP_STORAGE_BASE,
        input,
        hint,
        updateBackupDirectoryHint,
        { autoLoad: true, directoriesOnly: true },
      );
      container?.appendChild(browser);

      function updateBackupDirectoryHint() {
        const path = normalizeBackupDirectoryPath(input?.value);
        input?.classList.toggle("invalid", !path && !!input?.value.trim());
        if (hint) {
          hint.textContent = path
            ? "保存到 " + path
            : input?.value.trim()
              ? "请选择公开存储目录"
              : "";
          hint.className = "path-validation " + (path ? "valid" : "invalid");
        }
        return path;
      }

      let settled = false;
      const settle = (value) => {
        if (settled) return;
        settled = true;
        closeActiveModal();
        resolve(value);
      };
      State.modalCleanup = () => {
        if (!settled) {
          settled = true;
          resolve(null);
        }
      };
      document.querySelector(".modal-close")?.addEventListener("click", () => settle(null));
      document.getElementById("backupDirConfirm")?.addEventListener("click", () => {
        const path = updateBackupDirectoryHint();
        if (!path) {
          if (hint) {
            hint.textContent = "请选择公开存储目录";
            hint.className = "path-validation invalid";
          }
          input?.classList.add("invalid");
          return;
        }
        settle(path);
      });
      input?.addEventListener("input", () => {
        updateBackupDirectoryHint();
        browser.loadForInput(input.value);
      });
      updateBackupDirectoryHint();
    });
  }

  async function requestBackupSaveTarget(fileName) {
    const dirHandle = await requestBackupDirectoryHandle();
    if (dirHandle) return { type: "directoryHandle", handle: dirHandle };

    const pickedHandle = await requestBackupSaveHandle(fileName);
    if (pickedHandle) return { type: "fileHandle", handle: pickedHandle };

    const path = await showBackupDirectoryDialog(fileName);
    if (!path) throw createAbortError();
    return { type: "path", path };
  }

  async function saveBackupWithExistingSurface(fileName, bytes, target) {
    if (target?.type === "directoryHandle") {
      await saveBackupWithDirectoryHandle(target.handle, fileName, bytes);
      return "picked-directory";
    }
    if (target?.type === "path") {
      const path = await Api.saveBackupBytesToDirectory?.(
        target.path,
        fileName,
        uint8ToBase64(bytes),
      );
      if (path) return path;
      throw new Error("无法写入所选目录");
    }
    if (target?.type === "fileHandle") {
      await writeBackupToFileHandle(target.handle, bytes);
      return "picked-file";
    }
    if (typeof ksu_download !== "undefined" && typeof ksu_download.save === "function") {
      ksu_download.save(uint8ToBase64(bytes), fileName);
      return "manager";
    }
    if (typeof ksu !== "undefined" && Api.saveBackupToDownloads) {
      const path = await Api.saveBackupBytesToDownloads?.(fileName, uint8ToBase64(bytes));
      if (path) return path;
    }
    try {
      downloadBackupBytes(fileName, bytes);
      return "download";
    } catch {
      const path = await Api.saveBackupBytesToDownloads?.(fileName, uint8ToBase64(bytes));
      if (path) return path;
      throw new Error("当前 WebView 不支持保存备份文件");
    }
  }

  async function handleDiagnosticLogExport() {
    const btn = document.getElementById("logExportBtn");
    if (btn?.disabled) return;
    btn.disabled = true;
    let loading = null;
    try {
      const fileName = buildDiagnosticArchiveFileName();
      loading = Theme.showLoadingDialog("正在准备日志包");
      const progressOptions = {
        onProgress(progress) {
          if (!loading || !progress) return;
          loading.setMessage(progress.message || "正在导出日志");
          loading.setProgress(progress.percent);
        },
      };
      let path = "";
      if (isKernelSuWebUi()) {
        path = await Api.exportDiagnosticArchiveToDownloads(fileName, progressOptions);
      } else {
        const target = await requestBackupSaveTarget(fileName);
        path =
          target?.type === "path"
            ? await Api.exportDiagnosticArchiveToDirectory?.(target.path, fileName, progressOptions)
            : await Api.exportDiagnosticArchiveToDownloads(fileName, progressOptions);
      }
      loading.close();
      loading = null;
      Theme.showToast(path ? "日志包已导出：" + path : "日志导出失败", path ? "success" : "error");
    } catch (error) {
      loading?.close();
      if (error?.name === "AbortError") return;
      Theme.showToast(error?.message || "日志导出失败", "error");
    } finally {
      btn.disabled = false;
    }
  }

  async function handleBackupExport() {
    const btn = document.getElementById("backupExportBtn");
    if (btn?.disabled) return;
    btn.disabled = true;
    let loading = null;
    try {
      const fileName = buildBackupFileName();
      const backupTarget = await requestBackupSaveTarget(fileName);
      loading = Theme.showLoadingDialog("正在生成备份...");
      const payload = await buildBackupPayload();
      const text = JSON.stringify(payload, null, 2) + "\n";
      if (utf8Bytes(text).length > BACKUP_MAX_BYTES) throw new Error("备份文件过大");
      const bytes = buildBackupZipBytes(text);
      if (bytes.length > BACKUP_MAX_BYTES) throw new Error("备份文件过大");
      loading.close();
      loading = null;
      const target = await saveBackupWithExistingSurface(fileName, bytes, backupTarget);
      if (target === "picked-directory") {
        Theme.showToast("备份已保存到所选目录", "success");
      } else if (target === "picked-file") {
        Theme.showToast("备份已保存", "success");
      } else if (target === "manager") {
        Theme.showToast("已交给管理器保存备份", "success");
      } else if (target === "download") {
        Theme.showToast("备份文件已生成", "success");
      } else {
        Theme.showToast("备份已保存到所选目录", "success");
      }
    } catch (error) {
      if (loading) loading.close();
      if (error?.name === "AbortError") return;
      Theme.showToast(error?.message || "备份失败", "error");
    } finally {
      btn.disabled = false;
    }
  }

  function readZipU16(bytes, offset) {
    return bytes[offset] | (bytes[offset + 1] << 8);
  }

  function readZipU32(bytes, offset) {
    return (
      (bytes[offset] |
        (bytes[offset + 1] << 8) |
        (bytes[offset + 2] << 16) |
        (bytes[offset + 3] << 24)) >>>
      0
    );
  }

  async function inflateRawZipEntry(bytes) {
    if (typeof DecompressionStream !== "function")
      throw new Error("当前浏览器不支持读取压缩备份包");
    const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
    const buffer = await new Response(stream).arrayBuffer();
    return new Uint8Array(buffer);
  }

  async function extractBackupJsonFromZip(bytes) {
    let offset = 0;
    while (offset + 30 <= bytes.length) {
      if (readZipU32(bytes, offset) !== 0x04034b50) break;
      const flags = readZipU16(bytes, offset + 6);
      const method = readZipU16(bytes, offset + 8);
      const compressedSize = readZipU32(bytes, offset + 18);
      const fileNameLength = readZipU16(bytes, offset + 26);
      const extraLength = readZipU16(bytes, offset + 28);
      const nameStart = offset + 30;
      const dataStart = nameStart + fileNameLength + extraLength;
      if (flags & 0x08) throw new Error("备份包格式不支持 data descriptor");
      if (dataStart + compressedSize > bytes.length) throw new Error("备份包不完整");
      const name = new TextDecoder().decode(bytes.subarray(nameStart, nameStart + fileNameLength));
      const data = bytes.subarray(dataStart, dataStart + compressedSize);
      if (name === BACKUP_ZIP_ENTRY) {
        const payloadBytes =
          method === 0 ? data : method === 8 ? await inflateRawZipEntry(data) : null;
        if (!payloadBytes) throw new Error("备份包压缩方式不支持");
        if (payloadBytes.length > BACKUP_MAX_BYTES) throw new Error("备份文件过大");
        return new TextDecoder("utf-8").decode(payloadBytes);
      }
      offset = dataStart + compressedSize;
    }
    throw new Error("备份包缺少 " + BACKUP_ZIP_ENTRY);
  }

  async function decodeBackupFileBytes(bytes) {
    if (bytes.length >= 4 && bytes[0] === 0x50 && bytes[1] === 0x4b) {
      return await extractBackupJsonFromZip(bytes);
    }
    return new TextDecoder("utf-8").decode(bytes);
  }

  function readBackupFile(file) {
    return new Promise((resolve, reject) => {
      if (!file) {
        reject(new Error("未选择文件"));
        return;
      }
      if (file.size > BACKUP_MAX_BYTES) {
        reject(new Error("备份文件过大"));
        return;
      }
      const reader = new FileReader();
      reader.onload = async () => {
        try {
          resolve(await decodeBackupFileBytes(new Uint8Array(reader.result || new ArrayBuffer(0))));
        } catch (error) {
          reject(error);
        }
      };
      reader.onerror = () => reject(reader.error || new Error("读取文件失败"));
      reader.readAsArrayBuffer(file);
    });
  }

  async function parseBackupText(text) {
    let parsed;
    try {
      parsed = JSON.parse(text);
    } catch {
      throw new Error("备份文件不是有效 JSON");
    }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
      throw new Error("备份格式错误");
    if (parsed.magic !== BACKUP_MAGIC) throw new Error("不是 Storage Redirect X 备份");
    if (
      !Number.isInteger(parsed.schema) ||
      parsed.schema < 1 ||
      parsed.schema > BACKUP_SCHEMA_VERSION
    )
      throw new Error("备份格式版本不支持");
    if (!parsed.module || parsed.module.id !== BACKUP_MODULE_ID)
      throw new Error("备份属于其它模块");
    if (!parsed.data || typeof parsed.data !== "object" || Array.isArray(parsed.data))
      throw new Error("备份缺少数据区");
    const normalized = normalizeBackupApps(parsed.data.apps);
    const ui = normalizeBackupUiPreferences(parsed.data.ui);
    const data = {
      global: normalizeBackupGlobalConfig(parsed.data.global),
      apps: normalized.apps,
      templates: normalizeTemplates(parsed.data.templates),
      monitor_filters: normalizeBackupMonitorFilters(parsed.data.monitor_filters),
    };
    if (ui) data.ui = ui;
    if (!parsed.integrity?.value) throw new Error("备份缺少校验信息");
    const expected = String(parsed.integrity.value);
    const buildDigestCandidates = async (source) => {
      const optionSets = [];
      const autoOptions = [
        {},
        { includeAutoEnableNewAppsTemplateId: false },
        { includeAutoEnableNewApps: false },
      ];
      const verboseOptions = [{}, { includeVerboseLogging: false }];
      const uiOptions = ui ? [{}, { includeUiPreferences: false }] : [{}];
      const templateOptions = [{}, { includeTemplates: false }];
      autoOptions.forEach((autoOption) => {
        verboseOptions.forEach((verboseOption) => {
          uiOptions.forEach((uiOption) => {
            templateOptions.forEach((templateOption) => {
              optionSets.push({ ...autoOption, ...verboseOption, ...uiOption, ...templateOption });
            });
          });
        });
      });
      return Promise.all(
        optionSets.map((options) => {
          const cloned = Object.keys(options).length
            ? cloneForBackupDigest(source, options)
            : source;
          return verifyBackupDigest(stableJson(cloned), parsed.integrity);
        }),
      );
    };
    const baseDigests = await buildDigestCandidates(data);
    const legacyMonitorData = cloneForBackupDigest(data, { legacyDefaultMonitorOperations: true });
    const legacyMonitorDigests =
      stableJson(legacyMonitorData) === stableJson(data)
        ? []
        : await buildDigestCandidates(legacyMonitorData);
    const legacyFullMonitorData = cloneForBackupDigest(data, {
      legacyFullDefaultMonitorOperations: true,
    });
    const legacyFullMonitorDigests =
      stableJson(legacyFullMonitorData) === stableJson(data)
        ? []
        : await buildDigestCandidates(legacyFullMonitorData);
    const noMonitorData = cloneForBackupDigest(data, { includeMonitorFilters: false });
    const monitorCompatDigests = await buildDigestCandidates(noMonitorData);
    if (
      ![
        ...baseDigests,
        ...legacyMonitorDigests,
        ...legacyFullMonitorDigests,
        ...monitorCompatDigests,
      ].includes(expected)
    )
      throw new Error("备份校验失败，文件可能被改动");
    return { data, warnings: normalized.warnings, meta: parsed };
  }

  async function handleBackupFileSelected(event) {
    const input = event.target;
    const file = input?.files?.[0];
    if (input) input.value = "";
    if (!file) return;
    const loading = Theme.showLoadingDialog("正在校验备份...");
    try {
      const text = await readBackupFile(file);
      const parsed = await parseBackupText(text);
      loading.close();
      const appCount = Object.keys(parsed.data.apps || {}).length;
      const warningHint = parsed.warnings.length
        ? "有 " + parsed.warnings.length + " 条不兼容字段会被忽略。"
        : "";
      const confirmed = await confirmAction(
        "将用备份覆盖当前全局设置、全部应用配置、外观偏好和检查更新设置，共 " +
          appCount +
          " 个应用。" +
          warningHint +
          "还原后建议重启已配置应用或相关媒体进程。是否继续？",
      );
      if (!confirmed) return;
      await restoreBackupData(parsed.data);
    } catch (error) {
      loading.close();
      Theme.showToast(error?.message || "还原失败", "error");
    }
  }

  async function restoreBackupData(data) {
    const loading = Theme.showLoadingDialog("正在还原配置...");
    try {
      const ok = await Api.restoreConfigSnapshot(data);
      if (!ok) throw new Error("写入配置失败");
      State.globalConfig = data.global;
      State.templates = data.templates || [];
      State.monitorFilters = normalizeBackupMonitorFilters(data.monitor_filters);
      restoreBackupUiPreferences(data.ui);
      State.appsLoaded = false;
      State.configuredApps = Object.keys(data.apps || {});
      appListCache = { user: [], system: [] };
      Api.invalidateConfiguredAppsCache?.();
      loading.close();
      Theme.showToast("配置已还原", "success");
      if (State.currentPage === "settings") loadSettings();
      if (State.currentPage === "update") loadUpdate();
      refreshDashboardCountsIfVisible({ force: true });
    } catch (error) {
      loading.close();
      Theme.showToast(error?.message || "还原失败", "error");
    }
  }

  async function restartMediaProviderWithLoading() {
    const confirmed = await confirmAction(
      "快速重启会结束 MediaProvider 进程并触发系统重新拉起，期间媒体访问可能短暂不可用。是否继续？",
    );
    if (!confirmed) return;
    const loading = Theme.showLoadingDialog("正在快速重启 MediaProvider...");
    try {
      const ok = await Api.restartMediaProvider();
      if (!ok) throw new Error("MediaProvider restart timeout");
      Api.showManagerToast("MediaProvider 已重启");
    } catch (error) {
      Theme.showToast("重启 MediaProvider 失败", "error");
    } finally {
      loading.close();
    }
  }

  // ═══ Logs ═══
  async function loadLogs(options) {
    const pullRefresh = !!options?.pullRefresh;
    const viewer = $("#logViewer");
    if (State.isReturningFromConfigToLogs && viewer?.children.length) {
      State.isReturningFromConfigToLogs = false;
      restoreLogListScrollIfNeeded();
      State.shouldRestoreLogListScroll = false;
      return;
    }
    State.isReturningFromConfigToLogs = false;
    if (!pullRefresh)
      viewer.innerHTML =
        '<div class="loading-state"><div class="spinner"></div><span>加载日志...</span></div>';
    try {
      await Api.ensureLogCollectors?.();
      const [content, filters] = await Promise.all([
        Api.readFile(FILE_MONITOR_LOG),
        Api.readMonitorFilters({ force: true }).catch(() => State.monitorFilters || null),
      ]);
      if (filters) State.monitorFilters = filters;
      displayLogs(content, filters);
    } catch {
      if (pullRefresh) {
        Theme.showToast("刷新文件监视失败", "error");
        throw new Error("refresh failed");
      } else {
        viewer.innerHTML = '<div class="log-empty">暂无日志或无法读取</div>';
      }
    }
  }

  function displayLogs(raw, filters) {
    const viewer = $("#logViewer"),
      infoEl = $("#logInfo");
    if (!raw || !raw.trim()) {
      State.logEntries = [];
      viewer.innerHTML = '<div class="log-empty">暂无文件操作记录</div>';
      if (infoEl) infoEl.textContent = "";
      return;
    }
    State.logEntries = parseMonitorLogEntries(raw, { filters }).reverse();
    renderLogCards();
  }

  function parseMonitorLogEntries(raw, options) {
    if (!raw || !raw.trim()) return [];
    const lines = raw.trim().split("\n").filter(Boolean);
    if (options?.hydratePackages !== false) hydrateLogPackageInfo(lines);
    const filters = normalizeLogMonitorFilters(options?.filters || State.monitorFilters);
    const entries = lines
      .slice(-500)
      .map(parseLogLine)
      .filter(Boolean)
      .filter((entry) => !shouldFilterMonitorLogEntry(entry, filters));
    const mappingPaths = collectMappingPaths(entries);
    return coalesceLogEntries(entries, mappingPaths);
  }

  function initDashboardRefreshHooks() {
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) refreshDashboardCountsIfVisible();
    });
    window.addEventListener("pageshow", () => refreshDashboardCountsIfVisible());
    window.addEventListener("focus", () => refreshDashboardCountsIfVisible());
  }

  function hydrateLogPackageInfo(lines) {
    if (!Api.populatePackageInfo) return;
    const packages = new Set();
    lines.slice(-500).forEach((line) => {
      const parts = String(line || "").split("|");
      const extras = parseLogExtras(parts.slice(5));
      [parts[1], parts[2], extras.watch_package].forEach((pkg) => {
        if (pkg && pkg !== "-" && /^[A-Za-z0-9_.-]+$/.test(pkg)) packages.add(pkg);
      });
    });
    Api.populatePackageInfo(Array.from(packages));
  }

  function renderLogCards() {
    const viewer = $("#logViewer"),
      infoEl = $("#logInfo");
    if (!viewer) return;
    const query = State.logSearchQuery || "";
    const all = State.logEntries || [];
    const entries = query ? all.filter((entry) => entry.searchText.includes(query)) : all;
    if (infoEl)
      infoEl.textContent = query
        ? "匹配 " + entries.length + " / " + all.length + " 条"
        : "共 " + all.length + " 条";
    if (!entries.length) {
      viewer.innerHTML =
        '<div class="log-empty">' + (query ? "没有匹配的日志" : "暂无文件操作记录") + "</div>";
      return;
    }
    viewer.innerHTML = entries.map(renderLogCard).join("");
    bindLogCardToggles(viewer);
    if (State.shouldRestoreLogListScroll)
      requestAnimationFrame(() => {
        restoreLogListScrollIfNeeded();
        State.shouldRestoreLogListScroll = false;
      });
  }

  function parseLogLine(line) {
    const parts = String(line || "").split("|");
    const timestamp = parts[0] || "";
    const processPkg = parts[1] || "";
    const callerPkg = parts[2] || "";
    const eventKind = parts[3] || "";
    const path = parts[4] || extractPathFromLog(line);
    const extras = parseLogExtras(parts.slice(5));
    const filterOperation = extras.op_filter || extras.op || eventKind;
    const rawOperation = extras.op || eventKind;
    const operationIntent = monitorOperationIntent(filterOperation);
    if (extras.op === "monitor_watch") return null;
    const ret = Number(extras.ret ?? NaN);
    const errno = Number(extras.errno ?? NaN);
    const ok = Number.isFinite(ret) ? ret >= 0 : !/error|fail|denied/i.test(line);
    const watchPkg = extras.watch_package || "";
    const fromPath = extras.from || "";
    const landingPath = normalizeLogLandingPath(path);
    const sourcePath = normalizeLogLandingPath(fromPath);
    const backendPath = normalizeLogLandingPath(extras.backend || "");
    const displayPath = selectLogPrimaryPath(landingPath || path, backendPath, extras);
    const identifyMethod = extras.identify_method || "";
    const isModuleWebUiExport = isModuleExportLogEntry(extras, processPkg, callerPkg, watchPkg);
    const pkg = isModuleWebUiExport
      ? BACKUP_MODULE_ID
      : selectLogDisplayPackage(processPkg, callerPkg, watchPkg, identifyMethod);
    const appName = getLogPackageLabel(pkg);
    const operationLabel = isModuleWebUiExport
      ? "export"
      : formatLogOperationBadge(rawOperation || filterOperation);
    const action = isModuleWebUiExport
      ? describeModuleExportOperation(extras)
      : describeLogOperation(eventKind, filterOperation, extras);
    const sourceText = isModuleWebUiExport
      ? describeModuleExportSource(extras)
      : describeLogSource(processPkg, callerPkg, extras);
    const status = ok ? "success" : "error";
    const statusText = ok ? "成功" : describeErrno(errno, extras);
    const timeText = timestamp.length >= 16 ? timestamp.slice(11, 16) : "--:--";
    const meta = [];
    if (operationIntent) meta.push(monitorIntentLabel(operationIntent));
    if (sourceText) meta.push(sourceText);
    if (extras.identify_reliability)
      meta.push("可靠性 " + formatReliability(extras.identify_reliability));
    const summaryText = meta.join(" · ");
    const searchText = [
      appName,
      pkg,
      processPkg,
      callerPkg,
      watchPkg,
      operationLabel,
      operationIntent,
      action,
      sourceText,
      statusText,
      displayPath,
      sourcePath,
      backendPath,
      line,
    ]
      .join(" ")
      .toLowerCase();
    return {
      raw: line,
      timestamp,
      timeText,
      processPkg,
      callerPkg,
      watchPkg,
      pkg,
      appName,
      operationLabel,
      operationIntent,
      filterOperation,
      action,
      summaryText,
      path: displayPath,
      originalPath: path,
      fromPath,
      landingPath,
      sourcePath,
      backendPath,
      status,
      statusText,
      meta,
      extras,
      searchText,
      isModuleWebUiExport,
    };
  }

  function normalizeLogMonitorFilters(filters) {
    if (!filters || typeof filters !== "object")
      return { excluded_paths: [], excluded_operations: [] };
    return {
      excluded_paths: normalizeMonitorFilterPathList(filters.excluded_paths || []),
      excluded_operations: normalizeMonitorFilterOperationList(filters.excluded_operations || []),
    };
  }

  function shouldFilterMonitorLogEntry(entry, filters) {
    if (!entry || !filters) return false;
    const op = String(entry.filterOperation || entry.operationLabel || "")
      .trim()
      .toLowerCase();
    if (op && filters.excluded_operations.some((rule) => monitorOperationFilterMatches(rule, op)))
      return true;
    const paths = [
      entry.originalPath,
      entry.landingPath,
      entry.sourcePath,
      entry.fromPath,
      entry.backendPath,
      entry.path,
    ];
    return filters.excluded_paths.some((rule) =>
      paths.some((path) => monitorPathFilterMatches(rule, path)),
    );
  }

  function monitorOperationFilterMatches(rule, operation) {
    const pattern = String(rule || "")
      .trim()
      .toLowerCase();
    const value = String(operation || "")
      .trim()
      .toLowerCase();
    if (!pattern || !value || pattern.includes("/")) return false;
    return wildcardMatches(pattern, value);
  }

  function monitorOperationIntent(operation) {
    const match = String(operation || String())
      .trim()
      .toLowerCase()
      .match(/:(read|write|create)$/);
    return match ? match[1] : String();
  }

  function monitorIntentLabel(intent) {
    return { read: `读取意图`, write: `写入意图`, create: `创建意图` }[intent] || String();
  }

  function monitorPathFilterMatches(rule, path) {
    const result = validateMonitorFilterPath(rule, { allowLegacyAbsolute: true });
    if (!result.valid) return false;
    const pattern = result.value;
    const relative = monitorFilterRelativeLogPath(path);
    if (!relative) return false;
    if (!hasMonitorWildcard(pattern))
      return relative === pattern || relative.startsWith(pattern + "/");
    if (wildcardMatches(pattern, relative)) return true;
    if (pattern.endsWith("/**")) return wildcardMatches(pattern.slice(0, -3), relative);
    return false;
  }

  function monitorFilterRelativeLogPath(path) {
    let value = normalizeLogLandingPath(path)
      .replace(/\\/g, "/")
      .replace(/\/+/g, "/")
      .replace(/\/$/, "");
    if (!value) return "";
    value = value.replace(/^\/sdcard(?=\/|$)/, "/storage/emulated/0");
    value = value.replace(/^\/storage\/self\/primary(?=\/|$)/, "/storage/emulated/0");
    value = value.replace(/^\/data\/media\/(\d+)(?=\/|$)/, "/storage/emulated/$1");
    const match = value.match(/^\/storage\/emulated\/\d+\/(.+)$/);
    if (!match) return "";
    const relative = match[1].replace(/^\/+|\/+$/g, "");
    if (!relative || relative.split("/").some((part) => part === "." || part === "..")) return "";
    return relative;
  }

  function hasMonitorWildcard(pattern) {
    return /[*?]/.test(String(pattern || ""));
  }

  function wildcardMatches(pattern, value) {
    const escaped = String(pattern || "")
      .replace(/[.+^${}()|[\]\\]/g, "\\$&")
      .replace(/\*/g, ".*")
      .replace(/\?/g, ".");
    return new RegExp("^" + escaped + "$").test(String(value || ""));
  }

  function coalesceLogEntries(entries, mappingPaths = emptyMappingPaths()) {
    const groups = new Map();
    const ordered = [];
    entries.forEach((entry) => {
      if (!entry) return;
      if (shouldHideCompanionLogEntry(entry, mappingPaths)) return;
      const key = buildLogCoalesceKey(entry);
      if (!key) {
        ordered.push(entry);
        return;
      }
      const existing = groups.get(key);
      if (!existing) {
        groups.set(key, entry);
        ordered.push(entry);
        return;
      }
      const best = pickPreferredLogEntry(existing, entry);
      if (best !== existing) {
        groups.set(key, best);
        const index = ordered.indexOf(existing);
        if (index >= 0) ordered[index] = best;
      }
    });
    return ordered;
  }

  function emptyMappingPaths() {
    return { sources: new Set(), targets: new Set() };
  }

  function collectMappingPaths(entries) {
    const mappingPaths = emptyMappingPaths();
    entries.forEach((entry) => {
      if (!entry || entry.status !== "success") return;
      if (entry?.extras?.source !== "path_mapping") return;
      const sourcePath = entry?.sourcePath || entry?.fromPath || "";
      const targetPath = entry?.landingPath || entry?.path || "";
      if (sourcePath) mappingPaths.sources.add(sourcePath);
      if (targetPath) mappingPaths.targets.add(targetPath);
    });
    return mappingPaths;
  }

  function shouldHideCompanionLogEntry(entry, mappingPaths = emptyMappingPaths()) {
    const path = entry?.originalPath || entry?.landingPath || entry?.path || "";
    const name = path.split("/").pop() || "";
    if (/^\.[^/]+\.js$/i.test(name)) return true;
    if (entry?.status !== "error") return false;
    const sources = mappingPaths?.sources || new Set();
    const targets = mappingPaths?.targets || new Set();
    if (!sources.size && !targets.size) return false;
    const sourcePath = entry?.sourcePath || entry?.fromPath || "";
    const targetPath = entry?.landingPath || entry?.path || "";
    if (!sourcePath || !targetPath) return false;
    if (!sources.has(sourcePath) || !targets.has(targetPath)) return false;
    const opFilter = String(
      entry?.extras?.op_filter || entry?.extras?.op || entry?.operationLabel || "",
    ).toLowerCase();
    return (
      opFilter.includes("open") ||
      opFilter.includes("provider_open") ||
      opFilter.includes("create") ||
      opFilter.includes("write")
    );
  }

  function buildLogCoalesceKey(entry) {
    const path =
      entry?.extras?.source === "path_mapping"
        ? entry?.sourcePath || entry?.landingPath || entry?.path || ""
        : entry?.landingPath || entry?.path || entry?.sourcePath || "";
    if (!path) return "";
    const finalPath = normalizeCompanionLogPath(path);
    const isDiagnosticArchive = isDiagnosticLogArchivePath(finalPath);
    const coalescePath = diagnosticArchiveCoalescePath(finalPath);
    const op = isDiagnosticArchive
      ? "diagnostic_export"
      : normalizeMonitorCoalesceOperation(
          entry?.operationLabel || entry?.extras?.op_filter || entry?.extras?.op || "",
        );
    return [
      (entry.timestamp || "").slice(0, 16),
      coalescePath.replace(/^\/storage\/emulated\/\d+\//, "/storage/emulated/*/"),
      op,
      getLogResultGroup(entry),
    ].join("|");
  }

  function getLogResultGroup(entry) {
    if (!entry || entry.status === "success") return "ok";
    if (entry.extras?.deny_reason === "read_only_rule") return "deny:read_only_rule";
    return "error:" + (entry.statusText || "unknown");
  }

  function normalizeMonitorCoalesceOperation(value) {
    const op = String(value || "")
      .toLowerCase()
      .replace(/:create$|:read$/g, "");
    return op === "open" ||
      op === "openat" ||
      op === "openat2" ||
      op === "provider_open" ||
      op === "inotify" ||
      op === "close_write" ||
      op === "export"
      ? "create"
      : op;
  }

  function normalizeLogLandingPath(path) {
    const value = String(path || "").trim();
    if (!value) return "";
    return normalizeCompanionLogPath(value.replace(/^file:\/\//i, ""));
  }

  function selectLogPrimaryPath(landingPath, backendPath, extras = {}) {
    const source = String(extras.source || "").toLowerCase();
    if (backendPath && ["sandbox_path", "redirect_root", "fuse_redirect"].includes(source)) {
      return backendPath;
    }
    return landingPath || backendPath;
  }

  function requestPathForLogEntry(entry) {
    if (entry?.sourcePath) return entry.sourcePath;
    if (entry?.backendPath) return entry?.landingPath || entry?.originalPath || "";
    return "";
  }

  function normalizeCompanionLogPath(path) {
    const value = String(path || "");
    const index = value.lastIndexOf("/");
    const parent = index >= 0 ? value.slice(0, index) : "";
    const name = index >= 0 ? value.slice(index + 1) : value;
    if (/^\.[^/]+\.js$/i.test(name)) {
      return (parent ? parent + "/" : "") + name.slice(1, -3);
    }
    return value;
  }

  function diagnosticArchiveCoalescePath(path) {
    if (!isDiagnosticLogArchivePath(path)) return path;
    const name =
      String(path || "")
        .split("/")
        .pop() || "";
    return "/storage/emulated/*/Download/" + name;
  }

  function isModuleExportLogEntry(extras, processPkg, callerPkg, watchPkg) {
    return (
      extras?.identify_method === "module_export" ||
      extras?.source === "webui_export" ||
      extras?.source === "webui_backup" ||
      [processPkg, callerPkg, watchPkg].includes(BACKUP_MODULE_ID)
    );
  }

  function isDiagnosticLogArchivePath(path) {
    const name =
      String(path || "")
        .split("/")
        .pop() || "";
    return /^storage-redirect-x-logs-.+\.tar\.gz$/i.test(name);
  }

  function isManagerAppPackage(value) {
    return value === "org.srx.manager" || value === "org.srx.manager.debug";
  }

  function describeModuleExportOperation(extras) {
    const kind = String(extras?.export_kind || "").toLowerCase();
    if (kind === "backup") return "备份导出";
    if (kind === "diagnostic" || kind === "logs" || kind === "log") return "日志包导出";
    if (extras?.source === "webui_backup") return "备份导出";
    if (extras?.source === "webui_export") return "日志包导出";
    return "模块导出";
  }

  function describeModuleExportSource(extras) {
    return "存储重定向X · " + describeModuleExportOperation(extras);
  }

  function pickPreferredLogEntry(left, right) {
    return logEntryRank(right) > logEntryRank(left) ? right : left;
  }

  function logEntryRank(entry) {
    const method = entry?.extras?.identify_method || "unknown";
    const reliability = entry?.extras?.identify_reliability || "none";
    const ret = Number(entry?.extras?.ret ?? NaN);
    let score = 0;
    if (Number.isFinite(ret) && ret >= 0) score += 1000;
    if (isDiagnosticLogArchivePath(entry?.landingPath || entry?.path || "")) {
      if (isManagerAppPackage(entry?.pkg) || isManagerAppPackage(entry?.callerPkg)) score += 420;
      if (entry?.isModuleWebUiExport) score += 260;
    }
    if (entry?.callerPkg && entry.callerPkg !== "-" && entry.callerPkg !== entry.processPkg) {
      score += isIntermediateLogPackage(entry.callerPkg) ? 30 : 260;
    }
    if (
      method === "watch_package" &&
      entry?.watchPkg &&
      entry.watchPkg !== "-" &&
      entry.watchPkg !== entry.processPkg
    ) {
      score += isIntermediateLogPackage(entry.watchPkg) ? 20 : 140;
    }
    if (method === "caller") score += 220;
    if (method === "module_export") score += 230;
    if (method === "provider_open") score += 210;
    if (entry?.fromPath) score += 80;
    if (entry?.landingPath && entry?.sourcePath && entry.landingPath !== entry.sourcePath)
      score += 100;
    score +=
      {
        module_export: 125,
        caller: 120,
        provider_open: 110,
        recent_private_owner: 105,
        recent_caller: 100,
        path_config: 80,
        media_provider_fallback: 70,
        daemon_inotify: 75,
        java_stack: 70,
        stack: 70,
        path_hint: 55,
        thread_name: 45,
        path_owner: 35,
        owner_uid: 35,
        shared_uid: 10,
        unknown: 0,
      }[method] ?? 20;
    score += { high: 40, medium: 25, fallback: 5, none: 0 }[reliability] ?? 0;
    if (method === "shared_uid") score -= 80;
    if (isIntermediateLogPackage(entry?.pkg)) score -= 80;
    if (
      entry?.extras?.source === "allowed_real_path" &&
      entry?.watchPkg &&
      method !== "watch_package"
    )
      score -= 220;
    if (shouldHideCompanionLogEntry(entry)) score -= 200;
    return score;
  }

  function describeLogOperation(eventKind, syscallOp, extras) {
    const op = formatLogOperationBadge(syscallOp);
    if (op === "provider_open:read") return "Provider 读取请求";
    if (op === "provider_open:create") return "Provider 创建请求";
    if (op === "provider_open:write") return "Provider 写入请求";
    if (op === "open" || op === "openat" || op === "openat2" || op === "provider_open") {
      const hints = [];
      const flags = parseInt(String(extras.flags || "").replace(/^0x/i, ""), 16);
      if (Number.isFinite(flags)) {
        if ((flags & 0x40) !== 0) hints.push("O_CREAT");
        if ((flags & 0x200) !== 0) hints.push("O_TRUNC");
        if ((flags & 0x400) !== 0) hints.push("O_APPEND");
        if ((flags & 0x410000) === 0x410000) hints.push("O_TMPFILE");
      }
      return "带创建意图的文件打开" + (hints.length ? "（" + hints.join(" / ") + "）" : "");
    }
    if (op === "mkdir" || op === "mkdirat") return "目录创建请求";
    if (op === "mknod" || op === "mknodat") return "文件节点创建请求";
    if (String(eventKind || "").toUpperCase() === "CREATE") return "创建类文件操作";
    return op ? "文件操作：" + op : "文件操作记录";
  }

  function describeLogSource(processPkg, callerPkg, extras) {
    const method = extras.identify_method || "unknown";
    if (
      callerPkg &&
      callerPkg !== "-" &&
      callerPkg !== processPkg &&
      !isIntermediateLogPackage(callerPkg)
    ) {
      if (isSinglePackageName(callerPkg))
        return "调用方 " + callerPkg + "（" + formatIdentifyMethod(method) + "）";
      return "候选应用 " + callerPkg + "（" + formatIdentifyMethod(method) + "）";
    }
    if (
      method === "watch_package" &&
      extras.watch_package &&
      extras.watch_package !== "-" &&
      extras.watch_package !== processPkg &&
      !isIntermediateLogPackage(extras.watch_package)
    ) {
      return "监视应用 " + extras.watch_package + "（" + formatIdentifyMethod(method) + "）";
    }
    if (processPkg) return "进程 " + processPkg + "（" + formatIdentifyMethod(method) + "）";
    return formatIdentifyMethod(method);
  }

  function formatIdentifyMethod(method) {
    const map = {
      caller: "直接调用方",
      module_export: "模块导出记录",
      provider_open: "Provider 打开请求",
      media_provider_fallback: "MediaProvider 回退",
      path_owner: "路径归属",
      owner_uid: "文件属主",
      path_config: "路径配置",
      daemon_inotify: "外部 inotify",
      path_hint: "路径推断",
      stack: "堆栈推断",
      java_stack: "Java 栈推断",
      thread_name: "线程名推断",
      recent_caller: "近期调用方",
      recent_private_owner: "近期私有路径归属",
      download_owner: "下载记录",
      query_access: "媒体查询记录",
      shared_uid: "共享 UID 回退",
      unknown: "来源未知",
    };
    return map[method] || method;
  }

  function selectLogDisplayPackage(processPkg, callerPkg, watchPkg, identifyMethod) {
    return (
      [
        callerPkg !== processPkg && !isIntermediateLogPackage(callerPkg) ? callerPkg : "",
        identifyMethod === "watch_package" && !isIntermediateLogPackage(watchPkg) ? watchPkg : "",
        !isIntermediateLogPackage(callerPkg) ? callerPkg : "",
        !isIntermediateLogPackage(processPkg) ? processPkg : "",
        callerPkg,
        processPkg,
      ].find(isSinglePackageName) ||
      processPkg ||
      ""
    );
  }

  function isSinglePackageName(value) {
    return /^[A-Za-z0-9_.-]+$/.test(value || "") && String(value || "").includes(".");
  }

  function isIntermediateLogPackage(value) {
    const pkg = String(value || "");
    return (
      pkg === "com.google.android.providers.media.module" ||
      pkg === "com.android.providers.media.module" ||
      pkg === "com.android.providers.media" ||
      pkg === "com.android.providers.downloads" ||
      pkg === "com.android.providers.downloads.ui" ||
      pkg === "com.android.externalstorage" ||
      pkg === "com.android.mtp" ||
      pkg.includes(".documentsui") ||
      pkg.includes(".photopicker")
    );
  }

  function formatReliability(value) {
    const map = { high: "高", medium: "中", fallback: "回退", none: "未知" };
    return map[value] || value;
  }

  function describeErrno(errno, extras = {}) {
    if (extras.deny_reason === "read_only_rule") return "失败：命中只读模式规则";
    const map = {
      1: "失败：无权限",
      2: "失败：路径不存在",
      13: "失败：权限被拒绝",
      20: "失败：不是目录",
      21: "失败：是目录",
      28: "失败：空间不足",
      30: "失败：只读文件系统",
      107: "失败：传输端未连接",
    };
    return map[errno] || (errno ? "失败 errno=" + errno : "失败");
  }

  function formatLogOperationBadge(op) {
    const value = String(op || "")
      .trim()
      .toLowerCase();
    if (!value) return "unknown";
    if (
      value === "provider_open:read" ||
      value === "provider_open:create" ||
      value === "provider_open:write"
    )
      return value;
    if (value === "inotify") return "create";
    return value.replace(/:create$|:read$/g, "");
  }

  function parseLogExtras(parts) {
    const out = {};
    parts.forEach((part) => {
      const index = part.indexOf("=");
      if (index <= 0) return;
      out[part.slice(0, index)] = part.slice(index + 1);
    });
    return out;
  }

  function extractPathFromLog(line) {
    const match = String(line || "").match(/(?:^|\s)(\/storage\/[^\s|]+|\/data\/media\/[^\s|]+)/);
    return match ? match[1] : "";
  }

  function getLogPackageLabel(packageName) {
    if (!packageName || packageName === "-") return "未知应用";
    if (packageName === BACKUP_MODULE_ID) return "存储重定向X";
    const cached = Api.getCachedAppInfo ? Api.getCachedAppInfo(packageName) : null;
    return cached ? cached.appLabel || cached.label || cached.name || packageName : packageName;
  }

  function formatLogTime(entry) {
    if (!State.logFullTime) return entry.timeText || "--:--";
    const timestamp = String(entry.timestamp || "").replace("T", " ");
    if (timestamp.length >= 16) return timestamp.slice(0, 16);
    return timestamp || entry.timeText || "--:--";
  }

  function logOperationCopyValue(entry) {
    return String(entry?.filterOperation || entry?.operationLabel || `unknown`).trim() || `unknown`;
  }

  async function copyText(text) {
    const value = String(text || String());
    if (!value) return false;
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(value);
        return true;
      }
    } catch {}
    const input = document.createElement(`textarea`);
    input.value = value;
    input.setAttribute(`readonly`, String());
    input.style.position = `fixed`;
    input.style.opacity = `0`;
    document.body.appendChild(input);
    input.select();
    input.setSelectionRange(0, value.length);
    let copied = false;
    try {
      copied = document.execCommand(`copy`);
    } catch {}
    input.remove();
    return copied;
  }

  function renderLogCard(entry) {
    const iconSrc = Api.getAppIconSrc ? Api.getAppIconSrc(entry.pkg) : "";
    const initial = (entry.appName || entry.pkg || "?").trim().charAt(0).toUpperCase() || "?";
    const timeText = formatLogTime(entry);
    const timeTitle = State.logFullTime ? "切换为仅显示时间" : "切换为日期和时间";
    const canOpenApp = isSinglePackageName(entry.pkg) && !entry.isModuleWebUiExport;
    const iconHtml = iconSrc
      ? '<div class="log-card-icon has-image" data-initial="' +
        escapeHtml(initial) +
        '" aria-hidden="true"><img class="log-card-icon-img" src="' +
        escapeHtml(iconSrc) +
        '" alt="" loading="lazy" onerror="const icon=this.closest(\'.log-card-icon\'); if (icon) { icon.classList.remove(\'has-image\'); icon.classList.add(\'fallback\'); } this.remove();"></div>'
      : '<div class="log-card-icon fallback" data-initial="' +
        escapeHtml(initial) +
        '" aria-hidden="true"></div>';
    const identityContent =
      iconHtml + '<div class="log-card-title">' + escapeHtml(entry.appName) + "</div>";
    const identityHtml = canOpenApp
      ? '<button class="log-app-identity" type="button" data-pkg="' +
        escapeHtml(entry.pkg) +
        '" aria-label="打开应用配置">' +
        identityContent +
        "</button>"
      : '<div class="log-app-identity is-disabled">' + identityContent + "</div>";
    const detailParts = [];
    const requestPath = requestPathForLogEntry(entry);
    if (entry.backendPath && entry.backendPath !== entry.path)
      detailParts.push("实际路径：" + escapeHtml(entry.backendPath));
    if (requestPath && requestPath !== entry.path)
      detailParts.push("请求路径：" + escapeHtml(requestPath));
    if (entry.status === "error")
      detailParts.push(
        '<span class="log-detail-error">' + escapeHtml(entry.statusText) + "</span>",
      );
    const detailHtml = detailParts.length
      ? '<div class="log-detail">' + detailParts.join("<br>") + "</div>"
      : "";
    return (
      '<article class="log-card ' +
      entry.status +
      '">' +
      '<div class="log-card-body">' +
      '<div class="log-card-head">' +
      identityHtml +
      '<button class="log-status ' +
      entry.status +
      '" type="button" data-copy-operation="' +
      escapeHtml(logOperationCopyValue(entry)) +
      '" aria-label="复制操作规则" title="复制操作规则：' +
      escapeHtml(logOperationCopyValue(entry)) +
      '">' +
      escapeHtml(entry.operationLabel || "unknown") +
      "</button>" +
      '<button class="log-time" type="button" aria-label="' +
      escapeHtml(timeTitle) +
      '" title="' +
      escapeHtml(timeTitle) +
      '" aria-pressed="' +
      (State.logFullTime ? "true" : "false") +
      '">' +
      escapeHtml(timeText) +
      "</button>" +
      '<button class="log-expand-btn" type="button" aria-label="展开详情" title="展开详情"><span class="icon icon-chevron-down" aria-hidden="true"></span></button>' +
      "</div>" +
      (entry.summaryText
        ? '<div class="log-action">' + escapeHtml(entry.summaryText) + "</div>"
        : "") +
      '<div class="log-path">' +
      escapeHtml(entry.path || "未解析到路径") +
      "</div>" +
      detailHtml +
      "</div>" +
      "</article>"
    );
  }

  function bindLogCardToggles(root) {
    root.querySelectorAll(".log-app-identity[data-pkg]").forEach((btn) => {
      btn.addEventListener("click", (event) => {
        event.stopPropagation();
        const packageName = btn.dataset.pkg || "";
        if (isSinglePackageName(packageName)) openAppConfig(packageName, { originPage: "logs" });
      });
    });
    root.querySelectorAll(".log-status[data-copy-operation]").forEach((btn) => {
      btn.addEventListener("click", async (event) => {
        event.stopPropagation();
        const value = btn.dataset.copyOperation || "";
        const copied = await copyText(value);
        Theme.showToast(
          copied ? "已复制操作规则：" + value : "复制操作规则失败",
          copied ? "success" : "error",
        );
      });
    });
    root.querySelectorAll(".log-time").forEach((btn) => {
      btn.addEventListener("click", (event) => {
        event.stopPropagation();
        State.logFullTime = !State.logFullTime;
        renderLogCards();
      });
    });
    root.querySelectorAll(".log-expand-btn").forEach((btn) => {
      btn.addEventListener("click", (event) => {
        event.stopPropagation();
        const card = btn.closest(".log-card");
        const expanded = !card?.classList.contains("expanded");
        card?.classList.toggle("expanded", expanded);
        btn.setAttribute("aria-label", expanded ? "收起详情" : "展开详情");
        btn.title = expanded ? "收起详情" : "展开详情";
      });
    });
  }

  const MONITOR_INTENTS = [`read`, `write`, `create`];

  function splitMonitorOperationRules(items) {
    const operations = [];
    const intents = [];
    normalizeMonitorFilterOperationList(items).forEach((value) => {
      const match = String(value).match(/^\*:(read|write|create)$/i);
      if (match) intents.push(match[1].toLowerCase());
      else operations.push(value);
    });
    return { operations, intents };
  }

  function renderMonitorFilterList(container, items, type) {
    if (!container) return;
    if (!items.length) {
      container.innerHTML = '<div class="monitor-filter-empty">未添加规则</div>';
      return;
    }
    container.innerHTML = items
      .map(
        (value, index) =>
          '<div class="monitor-filter-item">' +
          '<span class="monitor-filter-value" title="' +
          escapeHtml(value) +
          '">' +
          escapeHtml(type === `intent` ? monitorIntentLabel(value) : value) +
          "</span>" +
          '<button class="monitor-filter-delete" type="button" data-type="' +
          type +
          '" data-index="' +
          index +
          '" aria-label="删除规则">×</button>' +
          "</div>",
      )
      .join("");
  }

  async function showMonitorFilterDialog() {
    const loading = Theme.showLoadingDialog(`正在读取过滤配置...`);
    let filters;
    try {
      const [monitorFilters, globalConfig] = await Promise.all([
        Api.readMonitorFilters({ force: true }),
        Api.readGlobalConfig({ force: true }).catch(() => null),
      ]);
      filters = monitorFilters;
      if (globalConfig) State.globalConfig = normalizeGlobalRuntimeConfig(globalConfig);
      State.monitorFilters = filters;
    } catch {
      loading.close();
      Theme.showToast(`读取过滤配置失败`, `error`);
      return;
    }
    loading.close();

    const autoSave = isAppConfigAutoSaveEnabled();
    const draft = {
      excluded_paths: normalizeMonitorFilterPathList(filters.excluded_paths),
      excluded_operations: normalizeMonitorFilterOperationList(filters.excluded_operations),
    };
    const panels = {
      path: {
        title: `路径`,
        description: `排除目录及其子路径，支持 * 和 ? 通配。`,
        placeholder: `Download 或 Android/cache`,
      },
      operation: {
        title: `操作`,
        description: `按操作名或模式过滤，支持 *、? 和意图后缀，例如 open*、open*:read。`,
        placeholder: `open* 或 open*:read`,
      },
      intent: {
        title: `意图`,
        description: `按访问目的过滤，不受 open、openat 等具体操作名影响。`,
      },
    };
    const actions = autoSave
      ? String()
      : `<div class='modal-actions'><button class='btn btn-secondary modal-close' type='button'>取消</button><button class='btn btn-primary' id='monitorFilterSave' type='button'>保存</button></div>`;
    showModalWithHistory(
      `<div class='modal-title monitor-filter-heading'><span>文件监视过滤</span><span class='monitor-filter-total' id='monitorFilterTotal'></span></div>
       <div class='monitor-filter-tabs' role='tablist' aria-label='过滤规则类型'>
         ${Object.entries(panels)
           .map(
             ([key, panel], index) =>
               `<button class='monitor-filter-tab${index ? String() : ` active`}' type='button' role='tab' aria-selected='${index ? `false` : `true`}' data-panel='${key}'><span>${panel.title}</span><small data-count='${key}'>0</small></button>`,
           )
           .join(String())}
       </div>
       <div class='monitor-filter-workspace'>
         <div class='monitor-filter-description' id='monitorFilterDescription'></div>
         <div class='monitor-filter-input-row' id='monitorFilterInputRow'><input type='text' class='modal-input' id='monitorFilterInput' autocomplete='off'><button class='icon-btn icon-btn-sm icon-btn-add monitor-filter-add' id='monitorFilterAdd' type='button' aria-label='添加规则' title='添加规则'>${iconHtml(`plus`)}</button></div>
         <div class='path-validation' id='monitorFilterValidation'></div>
         <div class='monitor-intent-options' id='monitorFilterIntentOptions'>
           ${MONITOR_INTENTS.map((intent) => `<button class='monitor-intent-option' type='button' data-intent='${intent}' aria-pressed='false'><span>${monitorIntentLabel(intent)}</span><small>${{ read: `仅读取现有内容`, write: `写入或追加内容`, create: `新建、覆盖或临时文件` }[intent]}</small>${iconHtml(`plus`)}</button>`).join(String())}
         </div>
         <div class='monitor-filter-list' id='monitorFilterList'></div>
       </div>${actions}`,
      { backdropClose: true },
    );

    let activeType = `path`;
    const input = $(`#monitorFilterInput`);
    const validation = $(`#monitorFilterValidation`);
    const persist = async (closeAfterSave = false) => {
      const snapshot = {
        excluded_paths: draft.excluded_paths.slice(),
        excluded_operations: draft.excluded_operations.slice(),
      };
      State.monitorFilterSaveQueue = State.monitorFilterSaveQueue
        .catch(() => false)
        .then(() => Api.writeMonitorFilters(snapshot));
      const ok = await State.monitorFilterSaveQueue;
      if (!ok) Theme.showToast(`保存过滤配置失败`, `error`);
      else {
        State.monitorFilters = snapshot;
        if (closeAfterSave) {
          closeActiveModal();
          Theme.showToast(`过滤配置已保存`, `success`);
        }
      }
      return ok;
    };
    const operationRuleValues = (type) => {
      const rules = splitMonitorOperationRules(draft.excluded_operations);
      return type === `intent` ? rules.intents : rules.operations;
    };
    const valuesForType = (type) =>
      type === `path` ? draft.excluded_paths : operationRuleValues(type);
    const render = () => {
      const panel = panels[activeType];
      const values = valuesForType(activeType);
      $(`#monitorFilterDescription`).textContent = panel.description;
      $(`#monitorFilterInputRow`).hidden = activeType === `intent`;
      $(`#monitorFilterIntentOptions`).hidden = activeType !== `intent`;
      $(`#monitorFilterList`).hidden = activeType === `intent`;
      validation.hidden = activeType === `intent`;
      if (activeType !== `path`) {
        validation.textContent = String();
        validation.classList.remove(`valid`, `invalid`);
      }
      input.placeholder = panel.placeholder || String();
      renderMonitorFilterList($(`#monitorFilterList`), values, activeType);
      const counts = {
        path: draft.excluded_paths.length,
        operation: operationRuleValues(`operation`).length,
        intent: operationRuleValues(`intent`).length,
      };
      $(`#monitorFilterTotal`).textContent =
        Object.values(counts).reduce((sum, count) => sum + count, 0) + ` 条规则`;
      Object.entries(counts).forEach(([type, count]) => {
        const node = document.querySelector(`[data-count='${type}']`);
        if (node) node.textContent = count;
      });
      $$(`.monitor-intent-option`).forEach((button) => {
        const selected = draft.excluded_operations.includes(`*:${button.dataset.intent}`);
        button.classList.toggle(`selected`, selected);
        button.setAttribute(`aria-pressed`, String(selected));
        button.title = selected ? `点击移除此过滤规则` : `点击添加此过滤规则`;
        const icon = button.querySelector(`.icon`);
        if (icon) icon.className = `icon icon-${selected ? `check` : `plus`}`;
      });
      $(`#monitorFilterList`)
        ?.querySelectorAll(`.monitor-filter-delete`)
        .forEach((button) => {
          button.addEventListener(`click`, () => {
            const index = Number(button.dataset.index);
            const value = values[index];
            const label = activeType === `intent` ? monitorIntentLabel(value) : value;
            Theme.confirmDelete(`删除过滤规则“${label}”？`, async () => {
              if (activeType === `path`) {
                draft.excluded_paths.splice(index, 1);
              } else {
                const storedValue = activeType === `intent` ? `*:${value}` : value;
                const storedIndex = draft.excluded_operations.indexOf(storedValue);
                if (storedIndex >= 0) draft.excluded_operations.splice(storedIndex, 1);
              }
              render();
              if (autoSave) await persist();
            });
          });
        });
    };
    const addCurrentValue = async () => {
      const result =
        activeType === `path`
          ? validateMonitorFilterPath(input.value, { allowLegacyAbsolute: false })
          : { valid: true, value: String(input.value || String()).trim(), msg: String() };
      if (!result.valid || !result.value) {
        Theme.showToast(result.msg || `规则不能为空`, `error`);
        if (activeType === `path`) showMonitorFilterPathValidation(input, validation);
        return;
      }
      if (result.value.includes(`\0`) || result.value.length > 512)
        return Theme.showToast(`规则格式不正确`, `error`);
      const target = activeType === `path` ? draft.excluded_paths : draft.excluded_operations;
      if (target.includes(result.value)) return Theme.showToast(`规则已存在`, `error`);
      target.push(result.value);
      target.sort(compareMonitorFilterValues);
      input.value = String();
      render();
      if (autoSave) await persist();
    };
    $$(`.monitor-filter-tab`).forEach((tab) =>
      tab.addEventListener(`click`, () => {
        activeType = tab.dataset.panel;
        $$(`.monitor-filter-tab`).forEach((item) => {
          const active = item === tab;
          item.classList.toggle(`active`, active);
          item.setAttribute(`aria-selected`, String(active));
        });
        render();
        if (activeType !== `intent`) setTimeout(() => focusWithoutViewportJump(input), 80);
      }),
    );
    $$(`.monitor-intent-option`).forEach((button) =>
      button.addEventListener(`click`, async () => {
        const intent = button.dataset.intent;
        if (!MONITOR_INTENTS.includes(intent)) return;
        const rule = `*:${intent}`;
        const index = draft.excluded_operations.indexOf(rule);
        if (index >= 0) {
          Theme.confirmDelete(`删除过滤规则“${monitorIntentLabel(intent)}”？`, async () => {
            draft.excluded_operations.splice(index, 1);
            render();
            if (autoSave) await persist();
          });
          return;
        }
        draft.excluded_operations.push(rule);
        draft.excluded_operations.sort(compareMonitorFilterValues);
        render();
        if (autoSave) await persist();
      }),
    );
    $(`#monitorFilterAdd`)?.addEventListener(`click`, addCurrentValue);
    input?.addEventListener(`keydown`, (event) => {
      if (event.key === `Enter`) addCurrentValue();
    });
    input?.addEventListener(`input`, () => {
      if (activeType === `path`) showMonitorFilterPathValidation(input, validation);
    });
    $(`#monitorFilterSave`)?.addEventListener(`click`, () => persist(true));
    document.querySelector(`.modal-close`)?.addEventListener(`click`, closeActiveModal);
    render();
    setTimeout(() => focusWithoutViewportJump(input), 220);
  }

  async function loadAbout() {
    const list = $("#licenseList");
    if (!list) return;
    list.innerHTML = LICENSES.map(
      (item) =>
        '<a class="license-item" href="' +
        item.url +
        '">' +
        '<span class="license-group">' +
        escapeHtml(item.group) +
        "</span>" +
        '<span class="license-name">' +
        escapeHtml(item.name) +
        "</span>" +
        '<span class="license-meta">' +
        escapeHtml(item.license) +
        "</span>" +
        "</a>",
    ).join("");
  }

  $("#logFilter")?.addEventListener("click", () => {
    showMonitorFilterDialog();
  });

  $("#logClear")?.addEventListener("click", () => {
    Theme.confirmDelete("确认清空文件监视记录？", async () => {
      try {
        await Api.clearFileMonitorLog();
        Theme.showToast("日志已清空", "success");
        loadLogs();
      } catch {
        Theme.showToast("清空失败", "error");
      }
    });
  });

  $("#logExportBtn")?.addEventListener("click", handleDiagnosticLogExport);

  function escapeHtml(str) {
    const div = document.createElement("div");
    div.textContent = str;
    return div.innerHTML;
  }

  if (window.__SRX_ENABLE_TEST_EXPORTS__) {
    window.__SRX_WEBUI_TEST__ = {
      normalizeBackupAppConfig,
      normalizeBackupMonitorFilters,
      normalizeBackupUiPreferences,
      normalizeGlobalRuntimeConfig,
      normalizeUsersConfig,
      logOperationCopyValue,
      parseLogLine,
      shouldFilterMonitorLogEntry,
      splitMonitorOperationRules,
      setToggleBusy,
      setToggleState,
      updateChannelBadge,
      updateVersionBadge,
    };
  }

  window.App = Object.assign(window.App || {}, { navigateFromNav });
  function init() {
    Theme.init();
    setupHistory();
    initNav();
    initSearch();
    initLogSearch();
    initPullRefreshControls();
    initDashboardRefreshHooks();
    initRuntimeActivationInteractions();
    loadDashboard();
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
