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
    window.LiquidNav?.setBlurEnabled(prefs.blurEffect !== false);
    window.LiquidNav?.setEnabled(prefs.liquidGlass !== false);
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
    if (nav) {
      const updateNavIndicator = () => {
        const active = nav.querySelector(".nav-item.active");
        if (active) nav._moveIndicatorTo?.(active, true);
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
    if (!nav || nav._dragBound) return;
    nav._dragBound = true;
    const items = () => Array.from(nav.querySelectorAll(".nav-item"));
    const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
    const state = {
      pointerId: null,
      dragging: false,
      canceled: false,
      releasePending: false,
      dragEligible: false,
      pressItem: null,
      activeItem: null,
      value: 0,
      targetValue: 0,
      valueVelocity: 0,
      velocity: 0,
      velocityTarget: 0,
      velocityVelocity: 0,
      pressProgress: 0,
      pressTarget: 0,
      pressVelocity: 0,
      scaleX: 1,
      scaleY: 1,
      scaleXTarget: 1,
      scaleYTarget: 1,
      scaleXVelocity: 0,
      scaleYVelocity: 0,
      panelOffset: 0,
      panelTarget: 0,
      panelVelocity: 0,
      shapeX: 1,
      shapeY: 1,
      lastPointerX: 0,
      lastPointerTime: 0,
      lastFrameTime: 0,
      raf: 0,
      calibrationToken: 0,
    };

    nav._dragState = state;
    // 参考实现的按压放大倍率：56dp 指示器放大到 78dp。
    const PRESS_SCALE = 78 / 56;
    const tabWidth = () => {
      const first = items()[0];
      return first?.offsetWidth || 1;
    };
    const indexOf = (item) => Math.max(0, items().indexOf(item));
    const syncActiveClass = (item) => {
      items().forEach((entry) => {
        const active = entry === item;
        entry.classList.toggle("active", active);
        entry.classList.toggle("is-under-lens", active);
      });
    };
    const syncLensClass = (item) => {
      items().forEach((entry) => entry.classList.toggle("is-under-lens", entry === item));
    };
    const applyVisualState = () => {
      window.LiquidNav?.apply({
        value: state.value,
        panelOffset: state.panelOffset,
        shapeX: state.shapeX,
        shapeY: state.shapeY,
        pressProgress: state.pressProgress,
      });
    };
    const springStep = (value, velocity, target, stiffness, damping, frameScale) => {
      const nextVelocity =
        (velocity + (target - value) * stiffness * frameScale) * Math.pow(damping, frameScale);
      return [value + nextVelocity * frameScale, nextVelocity];
    };
    const startAnimation = () => {
      if (state.raf) return;
      const frame = (time) => {
        const frameScale = state.lastFrameTime
          ? clamp((time - state.lastFrameTime) / 16.667, 0.5, 2)
          : 1;
        state.lastFrameTime = time;
        [state.value, state.valueVelocity] = springStep(
          state.value,
          state.valueVelocity,
          state.targetValue,
          0.28,
          0.66,
          frameScale,
        );
        [state.velocity, state.velocityVelocity] = springStep(
          state.velocity,
          state.velocityVelocity,
          state.velocityTarget,
          0.1,
          0.78,
          frameScale,
        );
        [state.pressProgress, state.pressVelocity] = springStep(
          state.pressProgress,
          state.pressVelocity,
          state.pressTarget,
          0.24,
          0.68,
          frameScale,
        );
        [state.scaleX, state.scaleXVelocity] = springStep(
          state.scaleX,
          state.scaleXVelocity,
          state.scaleXTarget,
          0.24,
          0.68,
          frameScale,
        );
        [state.scaleY, state.scaleYVelocity] = springStep(
          state.scaleY,
          state.scaleYVelocity,
          state.scaleYTarget,
          0.24,
          0.68,
          frameScale,
        );
        [state.panelOffset, state.panelVelocity] = springStep(
          state.panelOffset,
          state.panelVelocity,
          state.panelTarget,
          0.12,
          0.72,
          frameScale,
        );
        const visibleItems = items();
        const visibleIndex = clamp(Math.round(state.value), 0, visibleItems.length - 1);
        if (visibleItems[visibleIndex]) syncLensClass(visibleItems[visibleIndex]);
        if (
          state.releasePending &&
          Math.abs(state.targetValue - state.value) < Math.max(0.025, (items().length - 1) * 0.025)
        ) {
          state.releasePending = false;
          state.pressTarget = 0;
          state.scaleXTarget = 1;
          state.scaleYTarget = 1;
        }
        // 对齐参考实现 layerBlock：形状由欠阻尼的 scaleX/scaleY 弹簧驱动（按压时
        // 过冲 + 振荡产生果冻波动），再叠加带方向的速度果冻形变。
        // quality-allow(chinese-language): 以下两行保留参考实现 layerBlock 的速度果冻公式原样，便于逐字对照，翻译会丢失精度。
        //   scaleX /= 1 - clamp(velocity/10 * 0.75, -0.2, 0.2)
        //   scaleY *= 1 - clamp(velocity/10 * 0.25, -0.2, 0.2)
        // 参考实现 velocity 单位是 range/sec(除以 tab 数-1)，这里 state.velocity 是
        // tab/sec，需先除以 (tabCount-1) 再对齐，否则速度果冻会提前饱和、拉伸过头。
        const rangeCount = Math.max(1, items().length - 1);
        const velocityTenth = state.velocity / rangeCount / 10;
        const dvx = clamp(velocityTenth * 0.75, -0.2, 0.2);
        const dvy = clamp(velocityTenth * 0.25, -0.2, 0.2);
        state.shapeX = state.scaleX / (1 - dvx);
        state.shapeY = state.scaleY * (1 - dvy);
        applyVisualState();
        const moving =
          Math.abs(state.targetValue - state.value) > 0.001 ||
          Math.abs(state.valueVelocity) > 0.001 ||
          Math.abs(state.velocityTarget - state.velocity) > 0.01 ||
          Math.abs(state.velocityVelocity) > 0.01 ||
          Math.abs(state.pressTarget - state.pressProgress) > 0.001 ||
          Math.abs(state.pressVelocity) > 0.001 ||
          Math.abs(state.scaleXTarget - state.scaleX) > 0.001 ||
          Math.abs(state.scaleXVelocity) > 0.001 ||
          Math.abs(state.scaleYTarget - state.scaleY) > 0.001 ||
          Math.abs(state.scaleYVelocity) > 0.001 ||
          Math.abs(state.panelTarget - state.panelOffset) > 0.001 ||
          Math.abs(state.panelVelocity) > 0.001;
        if (moving) state.raf = requestAnimationFrame(frame);
        else {
          state.raf = 0;
          state.lastFrameTime = 0;
        }
      };
      state.raf = requestAnimationFrame(frame);
    };
    const updateLightFromPointer = (clientX, clientY) => {
      const box = nav.getBoundingClientRect();
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
    const setTargetFromItem = (item, immediate) => {
      if (!item) return;
      state.activeItem = item;
      syncActiveClass(item);
      state.targetValue = indexOf(item);
      if (immediate) {
        state.value = state.targetValue;
        state.valueVelocity = 0;
        applyVisualState();
      } else startAnimation();
    };
    nav._moveIndicatorTo = (item, immediate = false) => setTargetFromItem(item, immediate);
    nav._scheduleIndicatorCalibration = () => {
      const token = ++state.calibrationToken;
      const align = (pass) => {
        requestAnimationFrame(() => {
          if (token !== state.calibrationToken || state.pointerId !== null) return;
          const active = nav.querySelector(".nav-item.active");
          if (active && active.offsetWidth > 0) {
            setTargetFromItem(active, !state.releaseAnimating);
          }
          if (pass < 1) align(pass + 1);
        });
      };
      align(0);
    };
    const updateTargetFromPointer = (clientX, clientY, time) => {
      const list = items();
      if (!list.length) return;
      const elapsed = Math.max(1, time - state.lastPointerTime);
      const delta = clientX - state.lastPointerX;
      const valueDelta = delta / tabWidth();
      state.targetValue = clamp(state.targetValue + valueDelta, 0, list.length - 1);
      state.velocityTarget = state.velocityTarget * 0.52 + ((valueDelta * 1000) / elapsed) * 0.48;
      state.lastPointerX = clientX;
      state.lastPointerTime = time;
      const best = list[Math.round(state.targetValue)] || list[0];
      if (best !== state.activeItem) {
        state.activeItem = best;
        syncLensClass(best);
      }
      const totalWidth = nav.offsetWidth || 1;
      const dragFraction = clamp(delta / totalWidth, -1, 1);
      state.panelTarget = 4 * Math.sign(dragFraction) * Math.sqrt(Math.abs(dragFraction));
      updateLightFromPointer(clientX, clientY);
      startAnimation();
    };

    nav.addEventListener("pointerdown", (e) => {
      const item = e.target.closest(".nav-item");
      if (!item || document.body.classList.contains("liquid-glass-disabled")) return;
      e.preventDefault();
      state.pointerId = e.pointerId;
      state.pressItem = item;
      state.dragEligible = item.classList.contains("active");
      state.activeItem = item.classList.contains("active")
        ? item
        : nav.querySelector(".nav-item.active");
      state.targetValue = indexOf(item);
      state.lastPointerX = e.clientX;
      state.lastPointerTime = e.timeStamp;
      state.canceled = false;
      state.dragging = true;
      if (!state.dragEligible) state.dragging = false;
      state.releasePending = false;
      state.pressTarget = 1;
      state.scaleXTarget = PRESS_SCALE;
      state.scaleYTarget = PRESS_SCALE;
      ++state.calibrationToken;
      window.LiquidGlass?.freeze("navDrag");
      if (state.dragEligible) nav.setPointerCapture?.(state.pointerId);
      startAnimation();
    });
    window.addEventListener("pointermove", (e) => {
      if (e.pointerId !== state.pointerId || !state.dragging || !state.dragEligible) return;
      e.preventDefault();
      updateTargetFromPointer(e.clientX, e.clientY, e.timeStamp);
    });
    const finish = (e) => {
      if (state.pointerId === null || e.pointerId !== state.pointerId) return;
      e.preventDefault();
      const list = items();
      const targetIndex = state.dragEligible
        ? clamp(Math.round(state.targetValue), 0, list.length - 1)
        : indexOf(state.pressItem);
      const targetItem = list[targetIndex];
      state.targetValue = targetIndex;
      state.panelTarget = 0;
      state.velocityTarget = 0;
      state.dragging = false;
      state.dragEligible = false;
      state.releasePending = true;
      state.releaseAnimating = true;
      clearTimeout(nav._releaseAnimationTimer);
      nav._releaseAnimationTimer = setTimeout(() => {
        state.releaseAnimating = false;
      }, 720);
      if (state.pointerId !== null && nav.hasPointerCapture?.(state.pointerId)) {
        nav.releasePointerCapture?.(state.pointerId);
      }
      state.pointerId = null;
      ++state.calibrationToken;
      window.LiquidGlass?.unfreeze("navDrag");
      nav._suppressClickUntil = Date.now() + 360;
      if (targetItem) {
        state.activeItem = targetItem;
        syncActiveClass(targetItem);
        window.App?.navigateFromNav?.(targetItem.dataset.page);
      }
      startAnimation();
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
    if (!nav) return;
    if (nav._dragState) ++nav._dragState.calibrationToken;
    const active = nav.querySelector(".nav-item.active");
    if (active && active.offsetWidth > 0) {
      nav._moveIndicatorTo?.(active, !nav._dragState?.releaseAnimating);
    }
  },

  resetNavIndicator() {
    const nav = document.getElementById("bottomNav");
    if (!nav) return;
    nav.classList.remove("is-moving");
    window.LiquidNav?.measure();
    nav._scheduleIndicatorCalibration?.();
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
