/**
 * SRX Core WebUI - 主题与界面工具
 * 主题：light（默认）、dark、system
 * 处理导航与筛选器的指示器动画
 */

const THEME_KEY = "srx_theme";
const THEME_UI_KEY = "srx_theme_ui";
const THEME_OPTIONS = ["light", "dark", "system"];
const THEME_UI_DEFAULTS = {
  floatingNav: true,
  liquidGlass: true,
  blurEffect: true,
  dynamicColor: false,
  accentColor: 0,
  colorStyle: "TonalSpot",
  colorSpec: "Spec2025",
  pageScale: 1,
};
const THEME_VIEWPORT_CONTENT =
  "width=device-width, initial-scale=1.0, maximum-scale=1.0, user-scalable=no, viewport-fit=cover, interactive-widget=overlays-content";
const THEME_ACCENT_COLORS = {
  0xfff44336: "#F44336",
  0xffe91e63: "#E91E63",
  0xff9c27b0: "#9C27B0",
  0xff673ab7: "#673AB7",
  0xff3f51b5: "#3F51B5",
  0xff2196f3: "#2196F3",
  0xff00bcd4: "#00BCD4",
  0xff009688: "#009688",
  0xff4faf50: "#4FAF50",
  0xffffeb3b: "#FFEB3B",
  0xffffc107: "#FFC107",
  0xffff9800: "#FF9800",
  0xff795548: "#795548",
  0xff607d8f: "#607D8F",
  0xffff9ca8: "#FF9CA8",
};

const Theme = {
  _current: "light",
  _systemAccentPalette: null,
  _systemAccentRequest: null,
  _navigationSequence: 0,

  init() {
    const stored = localStorage.getItem(THEME_KEY) || "light";
    this.apply(stored);
    this.applyUiOptions();
    this._watchSystem();
    this._bindToggle();
    this._bindResize();
    this._initIndicators();
    this._bindNavDrag();
    this._bindLiquidToggleMotion();
    this._bindLiquidSurfaceLight();
    this.resetNavIndicator();
    this.refreshSystemAccent();
  },

  apply(mode) {
    this._current = mode;
    localStorage.setItem(THEME_KEY, mode);
    let resolved = mode;
    if (mode === "system") {
      resolved = window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
    }
    document.documentElement.setAttribute("data-theme", resolved);
    this.applyAccentOptions(this.getUiOptions());
    const btn = document.getElementById("themeToggle");
    if (btn) {
      const icons = { light: "☀", dark: "☾", system: "◐" };
      btn.textContent = icons[mode] || "◐";
    }
  },

  get() {
    return this._current;
  },

  getUiOptions() {
    try {
      return Object.assign(
        {},
        THEME_UI_DEFAULTS,
        JSON.parse(localStorage.getItem(THEME_UI_KEY) || "{}"),
      );
    } catch {
      return Object.assign({}, THEME_UI_DEFAULTS);
    }
  },

  getUiOption(key) {
    const value = this.getUiOptions()[key];
    return typeof value === "boolean" ? value !== false : value;
  },

  setUiOption(key, enabled) {
    const options = this.getUiOptions();
    options[key] = typeof enabled === "boolean" ? !!enabled : enabled;
    this.setUiOptions(options);
  },

  setUiOptions(options) {
    const next = Object.assign({}, THEME_UI_DEFAULTS, options || {});
    next.pageScale = this.normalizePageScale(next.pageScale);
    localStorage.setItem(THEME_UI_KEY, JSON.stringify(next));
    this.applyUiOptions(next);
  },

  applyUiOptions(options) {
    const prefs = options || this.getUiOptions();
    document.body.classList.toggle("nav-floating-disabled", prefs.floatingNav === false);
    document.body.classList.toggle("liquid-glass-disabled", prefs.liquidGlass === false);
    document.body.classList.toggle("blur-effect-disabled", prefs.blurEffect === false);
    document.body.classList.toggle("liquid-surface-disabled", prefs.blurEffect === false);
    // 折射与材质模糊分别受各自开关控制，关闭液态玻璃会立即释放全部位移滤镜。
    window.LiquidGlass?.setBlurEnabled(prefs.blurEffect !== false);
    window.LiquidGlass?.setEnabled(prefs.liquidGlass !== false);
    this.applyPageScale(prefs);
    this.applyAccentOptions(prefs);
    this.resetNavIndicator();
  },

  normalizePageScale(value) {
    const scale = Number(value);
    return Number.isFinite(scale) ? Math.max(0.8, Math.min(1.1, scale)) : 1;
  },

  applyPageScale(options) {
    const scale = this.normalizePageScale((options || this.getUiOptions()).pageScale);
    document.documentElement.style.setProperty("--srx-page-scale", String(scale));
    document
      .querySelector('meta[name="viewport"]')
      ?.setAttribute("content", THEME_VIEWPORT_CONTENT);
    if (!document.body) return;
    document.body.style.zoom = "";
    document.body.style.width = "";
    document.body.style.height = "";
    this.syncPageScaleLayout(scale);
  },

  syncPageScaleLayout(scaleValue) {
    const scale = this.normalizePageScale(scaleValue ?? this.getUiOptions().pageScale);
    this.syncPageScaleSpacing(scale);
    const app = document.querySelector(".app-container");
    if (!app) return;
    const resetProps = [
      "zoom",
      "width",
      "height",
      "right",
      "bottom",
      "left",
      "transform",
      "transformOrigin",
    ];
    if (scale === 1) {
      resetProps.forEach((prop) => (app.style[prop] = ""));
      return;
    }
    const viewportWidth = window.innerWidth || document.documentElement.clientWidth || 0;
    const wide = viewportWidth >= 700;
    app.style.zoom = "";
    app.style.right = "auto";
    app.style.bottom = "auto";
    app.style.height = 100 / scale + "%";
    if (wide) {
      app.style.left = "50%";
      app.style.width = Math.min(720, viewportWidth) / scale + "px";
      app.style.transform = "translateX(-50%) scale(" + scale + ")";
      app.style.transformOrigin = "50% 0";
    } else {
      app.style.left = "0";
      app.style.width = 100 / scale + "%";
      app.style.transform = "scale(" + scale + ")";
      app.style.transformOrigin = "0 0";
    }
  },

  syncPageScaleSpacing(scaleValue) {
    const scale = this.normalizePageScale(scaleValue ?? this.getUiOptions().pageScale);
    const viewportWidth = window.innerWidth || document.documentElement.clientWidth || 0;
    const basePaddingX = Math.max(14, Math.min(24, viewportWidth * 0.04));
    const root = document.documentElement;
    root.style.setProperty("--srx-page-padding-x", basePaddingX / scale + "px");
    root.style.setProperty("--srx-page-padding-top", 18 / scale + "px");
    root.style.setProperty("--srx-page-padding-bottom", 10 / scale + "px");
  },

  applyAccentOptions(options) {
    const root = document.documentElement;
    const prefs = Object.assign({}, THEME_UI_DEFAULTS, options || {});
    const accent = Number(prefs.accentColor) || 0;
    const systemPalette = this._systemAccentPalette;
    const systemAccent = this._isDarkResolved()
      ? systemPalette?.darkPrimary
      : systemPalette?.lightPrimary;
    const enabled = prefs.dynamicColor === true && (accent !== 0 || !!systemAccent);
    root.classList.toggle("custom-accent", enabled);
    if (!enabled) {
      root.style.removeProperty("--color-primary");
      root.style.removeProperty("--color-primary-2");
      root.style.removeProperty("--color-primary-bg");
      root.style.removeProperty("--color-primary-border");
      root.style.removeProperty("--color-info");
      root.style.removeProperty("--color-text-on-primary");
      if (!systemPalette) root.style.removeProperty("--system-accent-color");
      return;
    }
    const base = this._hexToRgb(
      accent === 0 ? systemAccent : THEME_ACCENT_COLORS[accent] || this._argbToHex(accent),
    );
    const style = prefs.colorStyle || THEME_UI_DEFAULTS.colorStyle;
    const spec = prefs.colorSpec || THEME_UI_DEFAULTS.colorSpec;
    const saturation =
      {
        TonalSpot: 0.9,
        Neutral: 0.42,
        Vibrant: 1.22,
        Expressive: 1.08,
        Rainbow: 1.14,
        FruitSalad: 1.05,
        Monochrome: 0,
        Fidelity: 1,
        Content: 0.82,
      }[style] ?? 0.9;
    const secondaryHue =
      {
        Expressive: 45,
        Rainbow: 92,
        FruitSalad: -55,
        Content: 28,
        Neutral: 18,
        Monochrome: 0,
      }[style] ?? 24;
    const specLightness = spec === "Spec2021" ? -2 : 0;
    const primary =
      accent === 0
        ? systemAccent
        : this._rgbToHex(this._adjustColor(base, { saturation, lightness: specLightness }));
    const secondary =
      accent === 0
        ? this._isDarkResolved()
          ? systemPalette?.darkSecondary || primary
          : systemPalette?.lightSecondary || primary
        : this._rgbToHex(
            this._adjustColor(base, {
              hue: secondaryHue,
              saturation: Math.max(0, saturation * 0.82),
              lightness: spec === "Spec2021" ? 7 : 11,
            }),
          );
    const primaryRgb = this._hexToRgb(primary);
    root.style.setProperty("--color-primary", primary);
    root.style.setProperty("--color-primary-2", secondary);
    root.style.setProperty(
      "--color-primary-bg",
      this._rgba(primaryRgb, this._isDarkResolved() ? 0.16 : 0.12),
    );
    root.style.setProperty(
      "--color-primary-border",
      this._rgba(primaryRgb, this._isDarkResolved() ? 0.28 : 0.24),
    );
    root.style.setProperty("--color-info", primary);
    root.style.setProperty(
      "--color-text-on-primary",
      this._relativeLuminance(primaryRgb) > 0.55 ? "#101828" : "#ffffff",
    );
    if (systemPalette) {
      root.style.setProperty(
        "--system-accent-color",
        this._isDarkResolved() ? systemPalette.darkPrimary : systemPalette.lightPrimary,
      );
    }
  },

  async refreshSystemAccent(force) {
    if (!force && this._systemAccentPalette) return this._systemAccentPalette;
    if (this._systemAccentRequest) return this._systemAccentRequest;
    if (typeof Api === "undefined" || typeof Api.readSystemAccentPalette !== "function")
      return null;
    this._systemAccentRequest = Api.readSystemAccentPalette()
      .then((palette) => {
        if (palette) this._systemAccentPalette = palette;
        this.applyAccentOptions(this.getUiOptions());
        return this._systemAccentPalette;
      })
      .finally(() => {
        this._systemAccentRequest = null;
      });
    return this._systemAccentRequest;
  },

  _isDarkResolved() {
    return document.documentElement.getAttribute("data-theme") === "dark";
  },

  _argbToHex(value) {
    const rgb = (Number(value) >>> 0) & 0xffffff;
    return "#" + rgb.toString(16).padStart(6, "0");
  },

  _hexToRgb(hex) {
    const value = String(hex || "#2f7dff")
      .replace("#", "")
      .trim();
    const normalized =
      value.length === 3
        ? value
            .split("")
            .map((ch) => ch + ch)
            .join("")
        : value.padStart(6, "0").slice(-6);
    const num = Number.parseInt(normalized, 16);
    return { r: (num >> 16) & 255, g: (num >> 8) & 255, b: num & 255 };
  },

  _rgbToHex(rgb) {
    const clamp = (value) => Math.max(0, Math.min(255, Math.round(value)));
    return (
      "#" +
      [rgb.r, rgb.g, rgb.b].map((value) => clamp(value).toString(16).padStart(2, "0")).join("")
    );
  },

  _rgba(rgb, alpha) {
    return (
      "rgba(" +
      Math.round(rgb.r) +
      "," +
      Math.round(rgb.g) +
      "," +
      Math.round(rgb.b) +
      "," +
      alpha +
      ")"
    );
  },

  _rgbToHsl(rgb) {
    let r = rgb.r / 255,
      g = rgb.g / 255,
      b = rgb.b / 255;
    const max = Math.max(r, g, b),
      min = Math.min(r, g, b);
    let h = 0,
      s = 0;
    const l = (max + min) / 2;
    if (max !== min) {
      const d = max - min;
      s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
      switch (max) {
        case r:
          h = (g - b) / d + (g < b ? 6 : 0);
          break;
        case g:
          h = (b - r) / d + 2;
          break;
        default:
          h = (r - g) / d + 4;
          break;
      }
      h *= 60;
    }
    return { h, s, l };
  },

  _hslToRgb(hsl) {
    const h = (((hsl.h % 360) + 360) % 360) / 360;
    const s = Math.max(0, Math.min(1, hsl.s));
    const l = Math.max(0, Math.min(1, hsl.l));
    if (s === 0) {
      const v = l * 255;
      return { r: v, g: v, b: v };
    }
    const hue2rgb = (p, q, t) => {
      if (t < 0) t += 1;
      if (t > 1) t -= 1;
      if (t < 1 / 6) return p + (q - p) * 6 * t;
      if (t < 1 / 2) return q;
      if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
      return p;
    };
    const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
    const p = 2 * l - q;
    return {
      r: hue2rgb(p, q, h + 1 / 3) * 255,
      g: hue2rgb(p, q, h) * 255,
      b: hue2rgb(p, q, h - 1 / 3) * 255,
    };
  },

  _adjustColor(rgb, options) {
    const hsl = this._rgbToHsl(rgb);
    return this._hslToRgb({
      h: hsl.h + (options?.hue || 0),
      s: Math.max(0, Math.min(1, hsl.s * (options?.saturation ?? 1))),
      l: Math.max(0, Math.min(1, hsl.l + (options?.lightness || 0) / 100)),
    });
  },

  _relativeLuminance(rgb) {
    const channel = (value) => {
      const normalized = value / 255;
      return normalized <= 0.03928
        ? normalized / 12.92
        : Math.pow((normalized + 0.055) / 1.055, 2.4);
    };
    return 0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b);
  },

  cycle() {
    const idx = THEME_OPTIONS.indexOf(this._current);
    const next = THEME_OPTIONS[(idx + 1) % THEME_OPTIONS.length];
    this.apply(next);
    const labels = { light: "浅色模式", dark: "深色模式", system: "跟随系统" };
    this.showToast("主题：" + labels[next]);
  },

  _watchSystem() {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    mq.addEventListener("change", () => {
      if (this._current === "system") this.apply("system");
    });
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden && this.getUiOption("dynamicColor")) this.refreshSystemAccent(true);
    });
  },

  _bindToggle() {
    document.getElementById("themeToggle")?.addEventListener("click", () => this.cycle());
  },

  _bindResize() {
    let globalResizeTimer;
    window.addEventListener("resize", () => {
      clearTimeout(globalResizeTimer);
      globalResizeTimer = setTimeout(() => {
        this.syncPageScaleLayout();
        this.resetNavIndicator();
        this.updateFilterIndicator();
      }, 150);
    });
  },

  _syncNavLens(activeItem) {
    document
      .querySelectorAll(".nav-item")
      .forEach((item) => item.classList.toggle("is-under-lens", item === activeItem));
  },

  _initIndicators() {
    const nav = document.getElementById("bottomNav");
    const navIndicator = nav?.querySelector(".nav-indicator");
    if (nav && navIndicator) {
      const updateNavIndicator = () => {
        const active = nav.querySelector(".nav-item.active");
        if (active) {
          if (nav._moveIndicatorTo) nav._moveIndicatorTo(active, true);
          else {
            navIndicator.style.width = active.offsetWidth + "px";
            navIndicator.style.left = active.offsetLeft + "px";
            this._syncNavLens(active);
          }
        }
      };
      requestAnimationFrame(updateNavIndicator);
      document.fonts?.ready.then(updateNavIndicator);
    }

    const filterGroup = document.getElementById("appFilterGroup");
    const filterIndicator = filterGroup?.querySelector(".filter-indicator");
    if (filterGroup && filterIndicator) {
      const updateFilterIndicator = () => {
        const active = filterGroup.querySelector(".filter-chip.active");
        if (active) {
          filterIndicator.style.width = active.offsetWidth + "px";
          filterIndicator.style.left = active.offsetLeft + "px";
        }
      };
      requestAnimationFrame(updateFilterIndicator);
      document.fonts?.ready.then(updateFilterIndicator);
    }
  },

  _bindLiquidSurfaceLight() {
    if (this._liquidSurfaceBound) return;
    this._liquidSurfaceBound = true;
    const selector =
      ".liquid-surface, .bottom-nav, .app-batch-bar, .modal-sheet, .dialog-box, .toast, .search-input-wrapper, .filter-group, .app-config-header, .status-card, .action-list, .app-list, .config-group, .theme-selector, .theme-settings-card, .template-card, .backup-restore-card, .log-card, .app-user-menu, .license-item";
    const update = (target, clientX, clientY) => {
      const surface = target?.closest?.(selector);
      if (
        !surface ||
        document.body.classList.contains("liquid-glass-disabled") ||
        document.body.classList.contains("liquid-surface-disabled")
      )
        return;
      const rect = surface.getBoundingClientRect();
      if (!rect.width || !rect.height) return;
      surface.style.setProperty(
        "--liquid-x",
        Math.max(0, Math.min(100, ((clientX - rect.left) / rect.width) * 100)).toFixed(2) + "%",
      );
      surface.style.setProperty(
        "--liquid-y",
        Math.max(0, Math.min(100, ((clientY - rect.top) / rect.height) * 100)).toFixed(2) + "%",
      );
    };
    document.addEventListener("pointerdown", (e) => update(e.target, e.clientX, e.clientY), {
      passive: true,
    });
    document.addEventListener(
      "pointermove",
      (e) => {
        if (!(e.buttons & 1)) return;
        update(e.target, e.clientX, e.clientY);
      },
      { passive: true },
    );
  },

  _bindLiquidToggleMotion() {
    if (this._liquidToggleMotionBound) return;
    this._liquidToggleMotionBound = true;
    const activePointers = new Map();
    const isLiquidEnabled = (toggle) =>
      !!toggle &&
      !toggle.disabled &&
      document.body.classList.contains("srx-lens-ready") &&
      !document.body.classList.contains("liquid-glass-disabled");
    const findToggleHit = (target, clientX, clientY) => {
      const directToggle = target?.closest?.(".toggle");
      if (directToggle) return { toggle: directToggle, expanded: false };
      const row = target?.closest?.(".switch-row");
      const toggle = row?.querySelector?.(".toggle");
      if (!toggle || toggle.disabled) return null;
      const rect = toggle.getBoundingClientRect();
      const hitSlop = 9;
      if (
        clientX < rect.left - hitSlop ||
        clientX > rect.right + hitSlop ||
        clientY < rect.top - hitSlop ||
        clientY > rect.bottom + hitSlop
      ) {
        return null;
      }
      return { toggle, expanded: true };
    };
    const updateLight = (toggle, clientX, clientY) => {
      const rect = toggle.getBoundingClientRect();
      if (!rect.width || !rect.height) return;
      toggle.style.setProperty(
        "--toggle-touch-x",
        Math.max(0, Math.min(100, ((clientX - rect.left) / rect.width) * 100)).toFixed(2) + "%",
      );
      toggle.style.setProperty(
        "--toggle-touch-y",
        Math.max(0, Math.min(100, ((clientY - rect.top) / rect.height) * 100)).toFixed(2) + "%",
      );
    };
    const beginPress = (toggle, clientX, clientY) => {
      if (!isLiquidEnabled(toggle)) return;
      clearTimeout(toggle._liquidPressTimer);
      toggle._liquidPressStartedAt = performance.now();
      updateLight(toggle, clientX, clientY);
      toggle.classList.remove("is-liquid-burst");
      toggle.classList.add("is-liquid-pressing");
    };
    const release = (toggle) => {
      if (!toggle) return;
      const startedAt = toggle._liquidPressStartedAt || performance.now();
      const remaining = Math.max(0, 150 - (performance.now() - startedAt));
      clearTimeout(toggle._liquidPressTimer);
      toggle._liquidPressTimer = setTimeout(() => {
        if (toggle._liquidPressStartedAt !== startedAt) return;
        toggle.classList.remove("is-liquid-pressing");
        toggle.classList.remove("is-liquid-burst");
        if (!isLiquidEnabled(toggle)) return;
        void toggle.offsetWidth;
        toggle.classList.add("is-liquid-burst");
        clearTimeout(toggle._liquidBurstTimer);
        toggle._liquidBurstTimer = setTimeout(
          () => toggle.classList.remove("is-liquid-burst"),
          480,
        );
      }, remaining);
    };
    document.addEventListener(
      "pointerdown",
      (event) => {
        if (event.button !== 0) return;
        const hit = findToggleHit(event.target, event.clientX, event.clientY);
        if (!hit) return;
        const { toggle, expanded } = hit;
        activePointers.set(event.pointerId, { toggle, expanded });
        beginPress(toggle, event.clientX, event.clientY);
      },
      { passive: true },
    );
    document.addEventListener(
      "pointermove",
      (event) => {
        const active = activePointers.get(event.pointerId);
        if (active && isLiquidEnabled(active.toggle)) {
          updateLight(active.toggle, event.clientX, event.clientY);
        }
      },
      { passive: true },
    );
    const finishPointer = (event) => {
      const active = activePointers.get(event.pointerId);
      activePointers.delete(event.pointerId);
      if (!active) return;
      release(active.toggle);
      if (event.type !== "pointerup" || !active.expanded || active.toggle.disabled) return;
      const hit = findToggleHit(event.target, event.clientX, event.clientY);
      if (hit?.toggle === active.toggle) active.toggle.click();
    };
    document.addEventListener("pointerup", finishPointer, { passive: true });
    document.addEventListener("pointercancel", finishPointer, { passive: true });
    document.addEventListener("keydown", (event) => {
      if (event.repeat || (event.key !== " " && event.key !== "Enter")) return;
      const toggle = event.target?.closest?.(".toggle");
      if (isLiquidEnabled(toggle)) {
        const rect = toggle.getBoundingClientRect();
        beginPress(toggle, rect.left + rect.width / 2, rect.top + rect.height / 2);
      }
    });
    document.addEventListener("keyup", (event) => {
      if (event.key !== " " && event.key !== "Enter") return;
      release(event.target?.closest?.(".toggle"));
    });
  },

  _bindNavDrag() {
    const nav = document.getElementById("bottomNav");
    const indicator = nav?.querySelector(".nav-indicator");
    if (!nav || !indicator || nav._dragBound) return;
    nav._dragBound = true;
    const items = () => Array.from(nav.querySelectorAll(".nav-item"));
    const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
    const state = {
      pointerId: null,
      longPressTimer: null,
      dragging: false,
      canceled: false,
      activeItem: null,
      currentX: 0,
      currentW: 0,
      targetX: 0,
      targetW: 0,
      lastClientX: 0,
      velocity: 0,
      positionVelocity: 0,
      widthVelocity: 0,
      lastFrameTime: 0,
      pressClientX: 0,
      pressClientY: 0,
      raf: 0,
      calibrationToken: 0,
    };

    nav._dragState = state;

    const navRect = () => nav.getBoundingClientRect();
    const pointerLocalX = (clientX, box = navRect()) => {
      const layoutWidth = nav.offsetWidth || box.width;
      if (!box.width || !layoutWidth) return 0;
      return (clientX - box.left) * (layoutWidth / box.width);
    };
    const indicatorBounds = (width) => {
      const style = getComputedStyle(nav);
      const leftInset = parseFloat(style.paddingLeft) || 0;
      const rightInset = parseFloat(style.paddingRight) || 0;
      const contentWidth = nav.clientWidth || nav.offsetWidth || 0;
      return {
        min: leftInset,
        max: Math.max(leftInset, contentWidth - rightInset - width),
      };
    };
    const setIndicatorGeometry = (left, width) => {
      const nextWidth = Math.max(40, Number(width) || indicator.offsetWidth || 40);
      const bounds = indicatorBounds(nextWidth);
      const clamped = Math.max(bounds.min, Math.min(bounds.max, left));
      indicator.style.width = nextWidth + "px";
      indicator.style.left = clamped + "px";
      return clamped;
    };
    const cancelIndicatorAnimation = () => {
      if (state.raf) cancelAnimationFrame(state.raf);
      state.raf = 0;
      state.lastFrameTime = 0;
      state.positionVelocity = 0;
      state.widthVelocity = 0;
    };
    const updateLightFromPointer = (clientX, clientY) => {
      const box = navRect();
      if (!box.width || !box.height) return;
      nav.style.setProperty(
        "--nav-touch-x",
        clamp(((clientX - box.left) / box.width) * 100, 0, 100).toFixed(2) + "%",
      );
      nav.style.setProperty(
        "--nav-touch-y",
        clamp(((clientY - box.top) / box.height) * 100, 0, 100).toFixed(2) + "%",
      );
    };
    const setRubberOffset = (clientX) => {
      // 拖动时保持底栏坐标固定，避免容器位移反过来改变指针坐标系。
      nav.style.setProperty("--nav-rubber-x", "0px");
    };
    const resetLiquidMotion = () => {
      nav.style.setProperty("--nav-rubber-x", "0px");
      indicator.style.setProperty("--nav-indicator-scale-x", "1");
      indicator.style.setProperty("--nav-indicator-scale-y", "1");
    };
    const syncActiveClass = (item) => {
      items().forEach((n) => {
        const active = n === item;
        n.classList.toggle("active", active);
        n.classList.toggle("is-under-lens", active);
      });
    };
    const syncPressTarget = (item = null) => {
      items().forEach((n) => n.classList.toggle("is-press-target", n === item));
    };
    const setTargetFromItem = (item, immediate) => {
      if (!item) return;
      state.activeItem = item;
      // 激活态字体权重会参与底栏布局，必须先更新 class 再读取几何值。
      syncActiveClass(item);
      const left = item.offsetLeft;
      const width = item.offsetWidth;
      state.targetX = left;
      state.targetW = width;
      if (immediate) {
        cancelIndicatorAnimation();
        state.currentX = left;
        state.currentW = width;
        state.positionVelocity = 0;
        state.widthVelocity = 0;
        state.currentX = setIndicatorGeometry(left, width);
        state.targetX = state.currentX;
        resetLiquidMotion();
      } else {
        kickAnimation();
      }
    };
    nav._moveIndicatorTo = (item, immediate = false) => setTargetFromItem(item, immediate);
    nav._scheduleIndicatorCalibration = () => {
      const token = ++state.calibrationToken;
      const align = (pass) => {
        requestAnimationFrame(() => {
          if (token !== state.calibrationToken || state.pointerId !== null) return;
          const active = nav.querySelector(".nav-item.active");
          if (active && active.offsetWidth > 0) setTargetFromItem(active, true);
          if (pass < 1) align(pass + 1);
        });
      };
      align(0);
    };
    const nearestItem = (clientX) => {
      const navBox = navRect();
      const x = pointerLocalX(clientX, navBox);
      let best = null;
      let bestScore = Infinity;
      items().forEach((item) => {
        const center = item.offsetLeft + item.offsetWidth / 2;
        const score = Math.abs(x - center);
        if (score < bestScore) {
          bestScore = score;
          best = item;
        }
      });
      return best;
    };
    const updateTargetFromPointer = (clientX, clientY) => {
      const navBox = navRect();
      const list = items();
      if (!list.length) return;
      const best = nearestItem(clientX) || list[0];
      if (best !== state.activeItem) {
        state.activeItem = best;
        syncActiveClass(best);
      }
      const width = best.offsetWidth;
      const x = pointerLocalX(clientX, navBox) - width / 2;
      const dx = clientX - state.lastClientX;
      state.velocity = state.velocity * 0.58 + dx * 0.42;
      state.lastClientX = clientX;
      const bounds = indicatorBounds(width);
      state.targetX = clamp(x, bounds.min, bounds.max);
      state.targetW = width;
      state.currentX = state.targetX;
      state.currentW = width;
      state.positionVelocity = 0;
      state.widthVelocity = 0;
      state.currentX = setIndicatorGeometry(state.currentX, width);
      indicator.style.setProperty("--nav-indicator-scale-x", "1");
      indicator.style.setProperty(
        "--nav-indicator-scale-y",
        (1 - clamp(Math.abs(state.velocity) / 980, 0, 0.035)).toFixed(3),
      );
      setRubberOffset(clientX);
      updateLightFromPointer(clientX, clientY);
    };
    const kickAnimation = () => {
      if (state.raf) return;
      const frame = (time) => {
        const frameScale = state.lastFrameTime
          ? clamp((time - state.lastFrameTime) / 16.667, 0.5, 2)
          : 1;
        state.lastFrameTime = time;
        const dx = state.targetX - state.currentX;
        const dw = state.targetW - state.currentW;
        const xSpring = state.dragging ? 0.22 : 0.16;
        const xDamping = state.dragging ? 0.62 : 0.76;
        const widthSpring = state.dragging ? 0.18 : 0.14;
        const widthDamping = state.dragging ? 0.66 : 0.74;
        state.positionVelocity =
          (state.positionVelocity + dx * xSpring * frameScale) * Math.pow(xDamping, frameScale);
        state.widthVelocity =
          (state.widthVelocity + dw * widthSpring * frameScale) *
          Math.pow(widthDamping, frameScale);
        state.currentX += state.positionVelocity * frameScale;
        state.currentW += state.widthVelocity * frameScale;
        if (Math.abs(dx) < 0.12 && Math.abs(state.positionVelocity) < 0.12) {
          state.currentX = state.targetX;
          state.positionVelocity = 0;
        }
        if (Math.abs(dw) < 0.12 && Math.abs(state.widthVelocity) < 0.12) {
          state.currentW = state.targetW;
          state.widthVelocity = 0;
        }
        state.currentX = setIndicatorGeometry(state.currentX, state.currentW);
        if (
          Math.abs(state.targetX - state.currentX) > 0.12 ||
          Math.abs(state.targetW - state.currentW) > 0.12 ||
          Math.abs(state.positionVelocity) > 0.12 ||
          Math.abs(state.widthVelocity) > 0.12
        ) {
          state.raf = requestAnimationFrame(frame);
        } else {
          state.raf = 0;
          state.lastFrameTime = 0;
        }
      };
      state.raf = requestAnimationFrame(frame);
    };

    nav.addEventListener("pointerdown", (e) => {
      const item = e.target.closest(".nav-item");
      if (!item || document.body.classList.contains("liquid-glass-disabled")) return;
      state.pointerId = e.pointerId;
      state.activeItem = item;
      state.lastClientX = e.clientX;
      state.pressClientX = e.clientX;
      state.pressClientY = e.clientY;
      state.velocity = 0;
      state.canceled = false;
      ++state.calibrationToken;
      clearTimeout(nav._movingTimer);
      nav.classList.remove("is-moving");
      nav.classList.add("is-pressing");
      syncPressTarget(item);
      updateLightFromPointer(e.clientX, e.clientY);
      const beginDrag = () => {
        if (state.dragging || state.pointerId === null) return;
        clearTimeout(state.longPressTimer);
        state.dragging = true;
        nav.classList.add("dragging");
        window.LiquidGlass?.freeze("navDrag");
        nav.setPointerCapture?.(state.pointerId);
        setTargetFromItem(state.activeItem, true);
        syncPressTarget();
      };
      state.beginDrag = beginDrag;
      clearTimeout(state.longPressTimer);
      state.longPressTimer = setTimeout(beginDrag, 160);
    });
    window.addEventListener("pointermove", (e) => {
      if (e.pointerId !== state.pointerId) return;
      if (!state.dragging) {
        const dx = e.clientX - state.pressClientX;
        const dy = e.clientY - state.pressClientY;
        if (Math.abs(dx) > 6 && Math.abs(dx) > Math.abs(dy)) {
          state.beginDrag?.();
          updateTargetFromPointer(e.clientX, e.clientY);
          e.preventDefault();
          return;
        }
        if (Math.abs(dy) > 8) {
          state.canceled = true;
          clearTimeout(state.longPressTimer);
          nav.classList.remove("is-pressing");
          syncPressTarget();
        }
        return;
      }
      e.preventDefault();
      updateTargetFromPointer(e.clientX, e.clientY);
    });
    const finish = (e) => {
      if (state.pointerId === null || e.pointerId !== state.pointerId) return;
      clearTimeout(state.longPressTimer);
      const draggedItem = state.dragging ? state.activeItem : null;
      if (draggedItem) {
        e.preventDefault();
        nav._suppressClickUntil = Date.now() + 360;
      } else if (e.type === "pointerup" && !state.canceled && state.activeItem) {
        nav._suppressClickUntil = Date.now() + 360;
      } else if (state.canceled) {
        nav._suppressClickUntil = Date.now() + 360;
      } else {
        const activeNow = nav.querySelector(".nav-item.active");
        if (activeNow && activeNow.offsetWidth > 0) {
          state.targetW = activeNow.offsetWidth;
          state.targetX = activeNow.offsetLeft;
          state.currentW = state.targetW;
        }
      }
      state.dragging = false;
      if (state.pointerId !== null && nav.hasPointerCapture?.(state.pointerId)) {
        nav.releasePointerCapture?.(state.pointerId);
      }
      state.pointerId = null;
      state.beginDrag = null;
      ++state.calibrationToken;
      nav.classList.remove("is-pressing", "dragging");
      window.LiquidGlass?.unfreeze("navDrag");
      syncPressTarget();
      resetLiquidMotion();
      if (draggedItem) {
        setTargetFromItem(draggedItem, true);
        window.App?.navigateFromNav?.(draggedItem.dataset.page);
      } else if (e.type === "pointerup" && !state.canceled && state.activeItem) {
        setTargetFromItem(state.activeItem, true);
        window.App?.navigateFromNav?.(state.activeItem.dataset.page);
      } else {
        kickAnimation();
      }
    };
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
  },

  _pulseIndicator(container) {
    if (!container) return;
    container.classList.add("is-moving");
    clearTimeout(container._movingTimer);
    container._movingTimer = setTimeout(() => container.classList.remove("is-moving"), 430);
  },

  updateNavIndicator() {
    const nav = document.getElementById("bottomNav");
    const navIndicator = nav?.querySelector(".nav-indicator");
    if (!nav || !navIndicator) return;
    if (nav._dragState) ++nav._dragState.calibrationToken;
    const active = nav.querySelector(".nav-item.active");
    if (active && active.offsetWidth > 0) {
      if (nav._moveIndicatorTo) {
        nav._moveIndicatorTo(active, true);
        return;
      }
      const left = active.offsetLeft;
      const width = active.offsetWidth;
      const style = getComputedStyle(nav);
      const leftInset = parseFloat(style.paddingLeft) || 0;
      const rightInset = parseFloat(style.paddingRight) || 0;
      const maxLeft = Math.max(
        leftInset,
        (nav.clientWidth || nav.offsetWidth) - rightInset - width,
      );
      const clampedLeft = Math.max(leftInset, Math.min(maxLeft, left));
      navIndicator.style.width = width + "px";
      navIndicator.style.left = clampedLeft + "px";

      if (nav._dragState) {
        nav._dragState.targetX = clampedLeft;
        nav._dragState.currentX = clampedLeft;
        nav._dragState.targetW = width;
        nav._dragState.currentW = width;
      }

      this._syncNavLens(active);
    }
  },

  resetNavIndicator() {
    const nav = document.getElementById("bottomNav");
    const navIndicator = nav?.querySelector(".nav-indicator");
    if (!nav || !navIndicator) return;
    nav.classList.remove("is-pressing", "is-moving", "dragging");
    nav.querySelectorAll(".nav-item").forEach((item) => item.classList.remove("is-press-target"));
    if (nav._scheduleIndicatorCalibration) {
      nav._scheduleIndicatorCalibration();
      return;
    }
    const align = () => {
      const active = nav.querySelector(".nav-item.active");
      if (!active || active.offsetWidth === 0) return;

      if (nav._moveIndicatorTo) {
        nav._moveIndicatorTo(active, true);
        return;
      }

      const left = active.offsetLeft;
      const width = active.offsetWidth;
      navIndicator.style.width = width + "px";
      navIndicator.style.left = left + "px";

      if (nav._dragState) {
        nav._dragState.targetX = left;
        nav._dragState.currentX = left;
        nav._dragState.targetW = width;
        nav._dragState.currentW = width;
      }

      this._syncNavLens(active);
    };
    requestAnimationFrame(align);
  },

  updateFilterIndicator() {
    const group = document.getElementById("appFilterGroup");
    const ind = group?.querySelector(".filter-indicator");
    if (!group || !ind) return;
    const active = group.querySelector(".filter-chip.active");
    if (active && active.offsetWidth > 0) {
      this._pulseIndicator(group);
      ind.style.width = active.offsetWidth + "px";
      ind.style.left = active.offsetLeft + "px";
    }
  },

  resetFilterIndicator() {
    const group = document.getElementById("appFilterGroup");
    const ind = group?.querySelector(".filter-indicator");
    if (!group || !ind) return;
    group.classList.remove("is-moving");
    const align = () => {
      const active = group.querySelector(".filter-chip.active");
      if (!active || active.offsetWidth === 0) return;
      ind.style.width = active.offsetWidth + "px";
      ind.style.left = active.offsetLeft + "px";
    };
    requestAnimationFrame(() => {
      align();
      requestAnimationFrame(align);
    });
  },

  /* ── Toast ── */
  showToast(message, type) {
    const container = document.getElementById("toastContainer");
    const toast = document.createElement("div");
    toast.className = "toast " + (type || "");
    toast.textContent = message;
    container.appendChild(toast);
    setTimeout(() => {
      toast.style.opacity = "0";
      toast.style.transition = "opacity 250ms ease";
      setTimeout(() => toast.remove(), 250);
    }, 2200);
  },

  /* ── Dialog ── */
  showDialog(message, onConfirm, onCancel) {
    const overlay = document.getElementById("dialogOverlay");
    const body = document.getElementById("dialogBody");
    const actions = document.getElementById("dialogActions");
    body.textContent = message;
    actions.innerHTML = "";

    document.body.classList.add("modal-open");

    const cancelBtn = document.createElement("button");
    cancelBtn.className = "btn btn-secondary";
    cancelBtn.textContent = "取消";
    cancelBtn.onclick = () => {
      overlay.classList.add("hidden");
      document.body.classList.remove("modal-open");
      if (onCancel) onCancel();
    };
    actions.appendChild(cancelBtn);

    const confirmBtn = document.createElement("button");
    confirmBtn.className = "btn btn-primary";
    confirmBtn.textContent = "确认";
    confirmBtn.onclick = () => {
      overlay.classList.add("hidden");
      document.body.classList.remove("modal-open");
      if (onConfirm) onConfirm();
    };
    actions.appendChild(confirmBtn);

    overlay.classList.remove("hidden");
  },

  showLoadingDialog(message) {
    const overlay = document.getElementById("dialogOverlay");
    const body = document.getElementById("dialogBody");
    const actions = document.getElementById("dialogActions");

    document.body.classList.add("modal-open");

    body.innerHTML =
      '<div class="loading-state dialog-loading">' +
      '<div class="spinner"></div>' +
      "<span>" +
      this.escapeHtml(message || "处理中...") +
      "</span>" +
      '<div class="dialog-progress hidden" role="progressbar" aria-valuemin="0" aria-valuemax="100">' +
      '<div class="dialog-progress-bar"></div>' +
      "</div>" +
      "</div>";
    actions.innerHTML = "";
    overlay.classList.remove("hidden");
    let progressValue = 0;
    return {
      close() {
        overlay.classList.add("hidden");
        document.body.classList.remove("modal-open");
      },
      setMessage(nextMessage) {
        const text = body.querySelector(".dialog-loading span");
        if (text) text.textContent = nextMessage || "";
      },
      setProgress(nextProgress) {
        const progress = Number(nextProgress);
        const track = body.querySelector(".dialog-progress");
        const bar = body.querySelector(".dialog-progress-bar");
        if (!track || !bar || !Number.isFinite(progress)) return;
        const percent = Math.max(progressValue, Math.max(0, Math.min(100, progress)));
        progressValue = percent;
        track.classList.remove("hidden");
        track.setAttribute("aria-valuenow", String(Math.round(percent)));
        bar.style.width = percent + "%";
      },
    };
  },

  escapeHtml(value) {
    return String(value ?? "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  },

  /* ── Modal (终极防冻结稳定版) ── */
  bindModalViewport() {
    const overlay = document.getElementById("modalOverlay");
    if (!overlay) return;
    overlay._modalViewportCleanup?.();

    const visualViewport = window.visualViewport;
    let raf = 0;
    let focusScrollTimer = 0;
    const scheduleViewportSync = () => {
      if (raf) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        const layoutHeight = window.innerHeight || document.documentElement.clientHeight || 0;
        const viewportHeight = visualViewport?.height || layoutHeight;
        const viewportTop = visualViewport?.offsetTop || 0;
        const keyboardInset = Math.max(0, Math.round(layoutHeight - viewportHeight - viewportTop));
        const keyboardActive = keyboardInset > 48;
        const availableHeight = Math.max(
          0,
          keyboardActive ? viewportHeight - viewportTop : layoutHeight,
        );
        const maxHeightRatio = keyboardActive ? 0.86 : 0.85;
        if (availableHeight) {
          overlay.style.setProperty(
            "--modal-sheet-max-height",
            Math.min(Math.round(availableHeight * maxHeightRatio), 720) + "px",
          );
        }
        overlay.classList.toggle("keyboard-active", keyboardActive);
      });
    };

    const onFocus = (e) => {
      if (e.target.tagName === "INPUT" || e.target.tagName === "TEXTAREA") {
        clearTimeout(focusScrollTimer);
        focusScrollTimer = setTimeout(() => {
          e.target.scrollIntoView({ block: "nearest", inline: "nearest" });
        }, 460);
      }
    };
    const onBlur = () => setTimeout(scheduleViewportSync, 120);
    overlay.addEventListener("focusin", onFocus);
    overlay.addEventListener("focusout", onBlur);
    window.addEventListener("resize", scheduleViewportSync);
    visualViewport?.addEventListener("resize", scheduleViewportSync);
    visualViewport?.addEventListener("scroll", scheduleViewportSync);
    scheduleViewportSync();

    overlay._modalViewportCleanup = () => {
      if (raf) cancelAnimationFrame(raf);
      clearTimeout(focusScrollTimer);
      overlay.removeEventListener("focusin", onFocus);
      overlay.removeEventListener("focusout", onBlur);
      window.removeEventListener("resize", scheduleViewportSync);
      visualViewport?.removeEventListener("resize", scheduleViewportSync);
      visualViewport?.removeEventListener("scroll", scheduleViewportSync);
      overlay.style.removeProperty("--modal-sheet-max-height");
      overlay.classList.remove("keyboard-active");
      overlay._modalViewportCleanup = null;
    };
  },

  releaseModalViewport() {
    document.getElementById("modalOverlay")?._modalViewportCleanup?.();
  },

  showModal(contentHtml, options) {
    const overlay = document.getElementById("modalOverlay");
    const content = document.getElementById("modalContent");
    if (!overlay || !content) return; // 防御性判断

    content.innerHTML = contentHtml;
    content.classList.toggle("update-dialog-sheet", !!content.querySelector(".update-dialog"));
    this.bindModalViewport();

    document.body.classList.add("modal-open");
    overlay.classList.remove("hidden");

    const closeHandler = (e) => {
      if (e.target === overlay) {
        overlay.classList.add("hidden");
        document.body.classList.remove("modal-open");
        this.releaseModalViewport();
        overlay.removeEventListener("click", closeHandler);
      }
    };

    if (!options?.disableBackdropClose) {
      overlay.addEventListener("click", closeHandler);
    }

    return {
      close: () => {
        overlay.classList.add("hidden");
        document.body.classList.remove("modal-open");
        this.releaseModalViewport();
        overlay.removeEventListener("click", closeHandler);
      },
      getElement(sel) {
        return content.querySelector(sel);
      },
    };
  },

  /* ── Navigation ── */
  navigateTo(page, options) {
    const navigationSequence = ++this._navigationSequence;
    document.querySelectorAll(".nav-item").forEach((n) => {
      n.classList.remove("active");
      n.classList.remove("is-under-lens");
      n.classList.remove("is-press-target");
    });
    const navItem = document.querySelector('.nav-item[data-page="' + page + '"]');
    if (navItem) {
      navItem.classList.add("active");
      this._syncNavLens(navItem);
    } else if (page === "about" || page === "update" || page === "theme") {
      const parentPage = page === "theme" ? "settings" : "dashboard";
      const parentNav = document.querySelector('.nav-item[data-page="' + parentPage + '"]');
      parentNav?.classList.add("active");
      this._syncNavLens(parentNav);
    }
    this.updateNavIndicator();

    const applyPage = () => {
      const current = document.querySelector(".page.active");
      if (current && current.id !== "page-" + page && !options?.noAnimation) {
        current.classList.add("leaving");
        setTimeout(() => current.classList.remove("leaving"), 240);
      }
      document.querySelectorAll(".page").forEach((p) => p.classList.remove("active"));
      const target = document.getElementById("page-" + page);
      if (target) {
        target.classList.toggle("no-animate", !!options?.noAnimation);
        target.classList.add("active");
        document.body.classList.toggle("about-active", page === "about");
        document.body.classList.toggle("apps-page-active", page === "apps");
        document.body.classList.toggle("logs-page-active", page === "logs");
        const isSecondaryPage =
          page === "app-config" || page === "about" || page === "update" || page === "theme";
        document.body.classList.toggle("app-config-active", page === "app-config");
        document.body.classList.toggle("secondary-page-active", isSecondaryPage);
        document.getElementById("bottomNav")?.toggleAttribute("hidden", isSecondaryPage);
        const scroller = document.querySelector(".app-container");
        if (scroller && !options?.preserveScroll) scroller.scrollTo({ top: 0, behavior: "auto" });
      }
      if (page === "apps") requestAnimationFrame(() => this.resetFilterIndicator());
      this.resetNavIndicator();
    };

    if (!options?.deferPage) {
      applyPage();
      return null;
    }
    return new Promise((resolve) => {
      requestAnimationFrame(() => {
        setTimeout(() => {
          if (navigationSequence !== this._navigationSequence) {
            resolve(false);
            return;
          }
          applyPage();
          resolve(true);
        }, 0);
      });
    });
  },

  confirmDelete(message, onConfirm) {
    this.showDialog(message || "确认删除？", onConfirm);
  },
};

window.Theme = Theme;
