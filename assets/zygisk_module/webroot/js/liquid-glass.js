/**
 * SRX Core WebUI - 液态玻璃折射引擎
 *
 * 参考 AndroidLiquidGlass 的圆角矩形有向距离场折射算法：先按元素尺寸与圆角
 * 生成位移贴图，再通过 SVG feDisplacementMap 让 backdrop-filter 产生真实的
 * 边缘折射。相同尺寸与圆角的元素共享同一份贴图与滤镜，避免逐控件重复开销。
 * 浏览器不支持引用式 backdrop-filter 时保留既有渐变高光表现。
 */

const LIQUID_GLASS_SVG_ID = "srxLiquidGlassFilters";
const LIQUID_GLASS_SVG_NS = "http://www.w3.org/2000/svg";
const LIQUID_GLASS_READY_CLASS = "srx-lens-ready";
const LIQUID_GLASS_ELEMENT_CLASS = "srx-lens";
const LIQUID_GLASS_MIN_EDGE = 12;
const LIQUID_GLASS_MAX_EDGE = 1024;
const LIQUID_GLASS_MAX_FILTERS = 40;
const LIQUID_GLASS_SCAN_DELAY = 90;
const LIQUID_GLASS_SIZE_STEP = 2;
const LIQUID_GLASS_REFRESH_BUDGET_MS = 6;
const LIQUID_GLASS_REFRESH_BATCH_SIZE = 4;

// 每类表面的折射参数：blur 与 saturate 对应材质厚度，折射高度与幅度对应边缘弯曲程度。
// 悬浮底栏由 liquid-nav.js 自行分层渲染，不在这里登记。
const LIQUID_GLASS_TARGETS = [
  {
    selector: ".app-batch-bar",
    blur: 11,
    saturate: 1.28,
    refractionHeight: 18,
    refractionAmount: 16,
  },
  {
    selector: ".app-batch-status",
    blur: 5,
    saturate: 1.36,
    refractionHeight: 11,
    refractionAmount: 14,
    depthEffect: true,
  },
  {
    selector: ".modal-sheet",
    blur: 16,
    saturate: 1.24,
    refractionHeight: 22,
    refractionAmount: 18,
  },
  {
    selector: ".dialog-box",
    blur: 16,
    saturate: 1.24,
    refractionHeight: 22,
    refractionAmount: 18,
  },
  { selector: ".toast", blur: 12, saturate: 1.3, refractionHeight: 14, refractionAmount: 14 },
  {
    selector: ".search-input-wrapper",
    blur: 8,
    saturate: 1.28,
    refractionHeight: 14,
    refractionAmount: 14,
  },
  {
    selector: ".filter-group",
    blur: 8,
    saturate: 1.28,
    refractionHeight: 14,
    refractionAmount: 14,
  },
  {
    selector: ".filter-indicator",
    blur: 5,
    saturate: 1.34,
    refractionHeight: 11,
    refractionAmount: 14,
    depthEffect: true,
  },
  {
    selector: ".icon-btn, .back-btn",
    blur: 6,
    saturate: 1.3,
    refractionHeight: 9,
    refractionAmount: 12,
    depthEffect: true,
  },
  {
    selector: ".toggle",
    // 小尺寸开关保留边缘折射，但降低模糊与位移，避免轨道和滑块发虚；
    // 轨道用半透明玻璃渐变 + backdrop blur 体现通透，滑块(圆圈)本身保持锐利。
    blur: 2,
    saturate: 1.18,
    refractionHeight: 3,
    refractionAmount: 5,
    depthEffect: true,
  },
  {
    selector: ".btn-secondary, .backup-action, .setting-option-row, .user-tab, .app-user-trigger",
    blur: 7,
    saturate: 1.28,
    refractionHeight: 10,
    refractionAmount: 12,
  },
  {
    selector: ".app-user-menu",
    blur: 12,
    saturate: 1.28,
    refractionHeight: 16,
    refractionAmount: 16,
  },
  {
    selector:
      ".liquid-surface, .app-config-header, .status-card, .action-list, .app-list, .config-group, .theme-selector, .theme-settings-card, .template-card, .backup-restore-card, .log-card, .license-item",
    blur: 10,
    saturate: 1.24,
    refractionHeight: 16,
    refractionAmount: 15,
  },
];

const LiquidGlass = {
  _supported: null,
  _enabled: false,
  _blurEnabled: true,
  _initialized: false,
  _svg: null,
  _filters: new Map(),
  _elements: new Map(),
  _sequence: 0,
  _canvas: null,
  _resizeObserver: null,
  _mutationObserver: null,
  _scanTimer: 0,
  _pendingScanRoots: new Set(),
  _scanFrame: 0,
  _pendingRefresh: new Set(),
  _refreshFrame: 0,
  _freezeOwners: new Set(),

  /** 判断浏览器是否支持在 backdrop-filter 中引用 SVG 滤镜。 */
  isSupported() {
    if (this._supported !== null) return this._supported;
    const probe = 'blur(2px) url("#srxLensProbe")';
    this._supported =
      typeof CSS !== "undefined" &&
      typeof CSS.supports === "function" &&
      (CSS.supports("backdrop-filter", probe) || CSS.supports("-webkit-backdrop-filter", probe));
    return this._supported;
  },

  init() {
    if (this._initialized) return;
    this._initialized = true;
    if (!this.isSupported()) return;
    this._bindObservers();
    this._bindTiltLight();
  },

  /** 液态玻璃开关控制折射；材质模糊由独立状态决定是否叠加。 */
  setEnabled(enabled) {
    const next = !!enabled && this.isSupported();
    if (next === this._enabled) {
      if (next) this.scan();
      return;
    }
    this._enabled = next;
    if (!document.body) return;
    document.body.classList.toggle(LIQUID_GLASS_READY_CLASS, next);
    if (next) {
      this.scan();
    } else {
      this._releaseAll();
    }
  },

  isEnabled() {
    return this._enabled;
  },

  /**
   * 供悬浮底栏等自建分层的模块复用同一套折射贴图算法，避免重复实现。
   * 返回值为 PNG data URL，位移编码规则见 `_displacementDataUrl`。
   */
  buildDisplacementMap(spec) {
    return this._displacementDataUrl(spec);
  },

  setBlurEnabled(enabled) {
    const next = !!enabled;
    if (next === this._blurEnabled) return;
    this._blurEnabled = next;
    this.refreshAll();
  },

  /** 扫描并登记指定范围内的玻璃表面。 */
  scan(root) {
    if (!this._enabled) return;
    const scope = root && root.querySelectorAll ? root : document;
    for (const target of LIQUID_GLASS_TARGETS) {
      scope.querySelectorAll(target.selector).forEach((element) => this.register(element, target));
      if (scope !== document && scope.matches && scope.matches(target.selector)) {
        this.register(scope, target);
      }
    }
  },

  register(element, options) {
    if (!this._enabled || !element || this._elements.has(element)) return;
    this._elements.set(element, { options, key: "" });
    element.classList.add(LIQUID_GLASS_ELEMENT_CLASS);
    this._resizeObserver?.observe(element);
    if (this._freezeOwners.size) this._pendingRefresh.add(element);
    else this.refresh(element);
  },

  release(element) {
    const entry = this._elements.get(element);
    if (!entry) return;
    this._elements.delete(element);
    this._resizeObserver?.unobserve(element);
    this._dropFilterReference(entry.key);
    element.classList.remove(LIQUID_GLASS_ELEMENT_CLASS);
    element.style.removeProperty("backdrop-filter");
    element.style.removeProperty("-webkit-backdrop-filter");
  },

  refresh(element) {
    const entry = this._elements.get(element);
    if (!entry || !this._enabled) return;
    if (!element.isConnected) {
      this.release(element);
      return;
    }
    const rect = element.getBoundingClientRect();
    const width = this._quantize(rect.width);
    const height = this._quantize(rect.height);
    if (
      width < LIQUID_GLASS_MIN_EDGE ||
      height < LIQUID_GLASS_MIN_EDGE ||
      width > LIQUID_GLASS_MAX_EDGE ||
      height > LIQUID_GLASS_MAX_EDGE
    ) {
      element.style.removeProperty("backdrop-filter");
      element.style.removeProperty("-webkit-backdrop-filter");
      return;
    }
    const options = entry.options;
    const radii = this._cornerRadii(element, width, height);
    const spec = {
      width,
      height,
      radii,
      refractionHeight: Math.min(options.refractionHeight, Math.min(width, height) / 2),
      refractionAmount: options.refractionAmount,
      depthEffect: !!options.depthEffect,
    };
    const key = this._filterKey(spec);
    const keyChanged = key !== entry.key;
    if (keyChanged) {
      this._dropFilterReference(entry.key);
      entry.key = key;
    }
    const filter =
      !keyChanged && this._filters.has(key)
        ? this._filters.get(key).url
        : this._filterFor(key, spec);
    if (!filter) return;
    const blur = this._blurEnabled && options.blur > 0 ? `blur(${options.blur}px) ` : "";
    const value = `${blur}${filter} saturate(${options.saturate})`;
    element.style.setProperty("backdrop-filter", value);
    element.style.setProperty("-webkit-backdrop-filter", value);
  },

  /**
   * 按调用方冻结折射贴图重建。指示器拖动等高频尺寸变化期间只保留既有贴图，
   * 避免逐帧生成位移图拖慢低端设备。同一调用方重复冻结不会累计，
   * 多点触控或事件丢失时也不会把引擎永久留在冻结状态。
   */
  freeze(owner) {
    this._freezeOwners.add(owner || "default");
  },

  unfreeze(owner) {
    if (!this._freezeOwners.delete(owner || "default")) return;
    if (this._freezeOwners.size === 0 && this._pendingRefresh.size && !this._refreshFrame) {
      this._refreshFrame = requestAnimationFrame(() => this._flushRefresh());
    }
  },

  /** 合并同一帧内的多次刷新请求，拖动指示器时避免重复生成贴图。 */
  scheduleRefresh(element) {
    if (!this._enabled) return;
    this._pendingRefresh.add(element);
    if (this._freezeOwners.size) return;
    if (this._refreshFrame) return;
    this._refreshFrame = requestAnimationFrame(() => this._flushRefresh());
  },

  _flushRefresh() {
    this._refreshFrame = 0;
    if (this._freezeOwners.size || !this._pendingRefresh.size) return;
    const startedAt = performance.now();
    let processed = 0;
    for (const item of this._pendingRefresh) {
      this._pendingRefresh.delete(item);
      this.refresh(item);
      processed += 1;
      if (
        processed >= LIQUID_GLASS_REFRESH_BATCH_SIZE ||
        performance.now() - startedAt >= LIQUID_GLASS_REFRESH_BUDGET_MS
      ) {
        break;
      }
    }
    if (this._pendingRefresh.size && !this._freezeOwners.size) {
      this._refreshFrame = requestAnimationFrame(() => this._flushRefresh());
    }
  },

  refreshAll() {
    if (!this._enabled) return;
    this._elements.forEach((_entry, element) => this._pendingRefresh.add(element));
    if (!this._freezeOwners.size && !this._refreshFrame) {
      this._refreshFrame = requestAnimationFrame(() => this._flushRefresh());
    }
  },

  _quantize(value) {
    const size = Math.round(value / LIQUID_GLASS_SIZE_STEP) * LIQUID_GLASS_SIZE_STEP;
    return Math.max(0, size);
  },

  _cornerRadii(element, width, height) {
    const style = getComputedStyle(element);
    const limit = Math.min(width, height) / 2;
    const parse = (raw) => {
      const first =
        String(raw || "0")
          .trim()
          .split(/\s+/)[0] || "0";
      const numeric = parseFloat(first) || 0;
      const value = first.endsWith("%") ? (numeric / 100) * Math.min(width, height) : numeric;
      return Math.max(0, Math.min(limit, Math.round(value)));
    };
    return [
      parse(style.borderTopLeftRadius),
      parse(style.borderTopRightRadius),
      parse(style.borderBottomRightRadius),
      parse(style.borderBottomLeftRadius),
    ];
  },

  _filterKey(spec) {
    return [
      spec.width,
      spec.height,
      spec.radii.join("-"),
      Math.round(spec.refractionHeight),
      Math.round(spec.refractionAmount),
      spec.depthEffect ? 1 : 0,
    ].join(":");
  },

  _ensureSvg() {
    if (this._svg && this._svg.isConnected) return this._svg;
    let svg = document.getElementById(LIQUID_GLASS_SVG_ID);
    if (!svg) {
      svg = document.createElementNS(LIQUID_GLASS_SVG_NS, "svg");
      svg.setAttribute("id", LIQUID_GLASS_SVG_ID);
      svg.setAttribute("aria-hidden", "true");
      svg.setAttribute("width", "0");
      svg.setAttribute("height", "0");
      svg.style.cssText =
        "position:fixed;top:0;left:0;width:0;height:0;pointer-events:none;opacity:0";
      document.body.appendChild(svg);
    }
    this._svg = svg;
    return svg;
  },

  _filterFor(key, spec) {
    const cached = this._filters.get(key);
    if (cached) {
      cached.refs += 1;
      cached.usedAt = performance.now();
      return cached.url;
    }
    const map = this._displacementDataUrl(spec);
    if (!map) return "";
    const svg = this._ensureSvg();
    this._sequence += 1;
    const id = `srxLens${this._sequence}`;
    const filter = document.createElementNS(LIQUID_GLASS_SVG_NS, "filter");
    filter.setAttribute("id", id);
    filter.setAttribute("filterUnits", "userSpaceOnUse");
    filter.setAttribute("primitiveUnits", "userSpaceOnUse");
    filter.setAttribute("color-interpolation-filters", "sRGB");
    filter.setAttribute("x", "0");
    filter.setAttribute("y", "0");
    filter.setAttribute("width", String(spec.width));
    filter.setAttribute("height", String(spec.height));

    const image = document.createElementNS(LIQUID_GLASS_SVG_NS, "feImage");
    image.setAttribute("href", map);
    image.setAttribute("x", "0");
    image.setAttribute("y", "0");
    image.setAttribute("width", String(spec.width));
    image.setAttribute("height", String(spec.height));
    image.setAttribute("result", "srxLensMap");
    filter.appendChild(image);

    const displace = document.createElementNS(LIQUID_GLASS_SVG_NS, "feDisplacementMap");
    displace.setAttribute("in", "SourceGraphic");
    displace.setAttribute("in2", "srxLensMap");
    displace.setAttribute("scale", String(spec.refractionAmount * 2));
    displace.setAttribute("xChannelSelector", "R");
    displace.setAttribute("yChannelSelector", "G");
    filter.appendChild(displace);

    svg.appendChild(filter);
    const record = { id, node: filter, refs: 1, url: `url("#${id}")`, usedAt: performance.now() };
    this._filters.set(key, record);
    this._pruneFilters();
    return record.url;
  },

  _dropFilterReference(key) {
    if (!key) return;
    const record = this._filters.get(key);
    if (!record) return;
    record.refs = Math.max(0, record.refs - 1);
    record.usedAt = performance.now();
  },

  /** 超出缓存上限时回收最久未使用且无引用的滤镜。 */
  _pruneFilters() {
    if (this._filters.size <= LIQUID_GLASS_MAX_FILTERS) return;
    const idle = [...this._filters.entries()]
      .filter(([, record]) => record.refs === 0)
      .sort((left, right) => left[1].usedAt - right[1].usedAt);
    let removable = this._filters.size - LIQUID_GLASS_MAX_FILTERS;
    for (const [key, record] of idle) {
      if (removable <= 0) break;
      record.node.remove();
      this._filters.delete(key);
      removable -= 1;
    }
  },

  _context(width, height) {
    if (!this._canvas) {
      this._canvas = document.createElement("canvas");
    }
    this._canvas.width = width;
    this._canvas.height = height;
    return this._canvas.getContext("2d", { willReadFrequently: true });
  },

  /**
   * 生成折射位移贴图：R 通道编码水平位移，G 通道编码垂直位移，
   * 中性值 128 表示不偏移，位移方向沿圆角矩形距离场梯度指向内部。
   * `chromaticAberration` 与 `spectralShift` 用于生成色散分量贴图：
   * `spectralShift` 取 1 / 0 / -1 分别对应红、绿、蓝三个采样通道。
   */
  _displacementDataUrl(spec) {
    const width = spec.width;
    const height = spec.height;
    const amount = spec.refractionAmount;
    const band = spec.refractionHeight;
    if (amount <= 0 || band <= 0) return "";
    const context = this._context(width, height);
    if (!context) return "";
    const chromatic = spec.chromaticAberration > 0 ? spec.chromaticAberration : 0;
    const spectralShift = chromatic > 0 ? spec.spectralShift || 0 : 0;
    const image = context.createImageData(width, height);
    const data = image.data;
    const halfWidth = width / 2;
    const halfHeight = height / 2;
    const scale = amount * 2;
    const radii = spec.radii;
    for (let y = 0; y < height; y += 1) {
      const centerY = y + 0.5 - halfHeight;
      for (let x = 0; x < width; x += 1) {
        const index = (y * width + x) * 4;
        const centerX = x + 0.5 - halfWidth;
        const radius = liquidGlassRadiusAt(centerX, centerY, radii);
        const distance = liquidGlassRoundedRectSdf(centerX, centerY, halfWidth, halfHeight, radius);
        data[index + 2] = 128;
        data[index + 3] = 255;
        const inner = Math.min(distance, 0);
        const progress = 1 + inner / band;
        if (progress <= 0) {
          data[index] = 128;
          data[index + 1] = 128;
          continue;
        }
        const depth = 1 - Math.sqrt(Math.max(0, 1 - progress * progress));
        const gradientRadius = Math.min(radius * 1.5, Math.min(halfWidth, halfHeight));
        const gradient = liquidGlassRoundedRectGradient(
          centerX,
          centerY,
          halfWidth,
          halfHeight,
          gradientRadius,
        );
        let gradientX = gradient[0];
        let gradientY = gradient[1];
        if (spec.depthEffect) {
          const length = Math.hypot(centerX, centerY) || 1;
          gradientX += centerX / length;
          gradientY += centerY / length;
        }
        const gradientLength = Math.hypot(gradientX, gradientY) || 1;
        let offsetX = (-amount * depth * gradientX) / gradientLength;
        let offsetY = (-amount * depth * gradientY) / gradientLength;
        if (spectralShift !== 0) {
          // 色散强度随象限位置变化，与参考实现的 dispersionIntensity 一致。
          const dispersion =
            1 + spectralShift * chromatic * ((centerX * centerY) / (halfWidth * halfHeight));
          offsetX *= dispersion;
          offsetY *= dispersion;
        }
        data[index] = liquidGlassChannel(offsetX, scale);
        data[index + 1] = liquidGlassChannel(offsetY, scale);
      }
    }
    context.putImageData(image, 0, 0);
    return this._canvas.toDataURL("image/png");
  },

  _bindObservers() {
    if (typeof ResizeObserver === "function") {
      this._resizeObserver = new ResizeObserver((entries) => {
        entries.forEach((entry) => this.scheduleRefresh(entry.target));
      });
    }
    if (typeof MutationObserver === "function") {
      this._mutationObserver = new MutationObserver((records) => {
        records.forEach((record) => {
          record.addedNodes.forEach((node) => {
            if (node.nodeType === Node.ELEMENT_NODE) this._pendingScanRoots.add(node);
          });
        });
        this._scheduleMutationScan();
      });
    }
    const start = () => {
      if (!document.body) return;
      document.body.classList.toggle(LIQUID_GLASS_READY_CLASS, this._enabled);
      this._mutationObserver?.observe(document.body, { childList: true, subtree: true });
      this.scan();
    };
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", start, { once: true });
    } else {
      start();
    }
    window.addEventListener("resize", () => this.refreshAll(), { passive: true });
  },

  _collectDetached() {
    [...this._elements.keys()].forEach((element) => {
      if (!element.isConnected) this.release(element);
    });
  },

  _scheduleMutationScan() {
    if (this._scanFrame || this._scanTimer) return;
    this._scanTimer = setTimeout(() => {
      this._scanTimer = 0;
      const run = () => {
        this._scanFrame = 0;
        const roots = [...this._pendingScanRoots];
        this._pendingScanRoots.clear();
        roots.forEach((root) => this.scan(root));
        this._collectDetached();
        if (this._pendingScanRoots.size) this._scheduleMutationScan();
      };
      if (typeof requestIdleCallback === "function") requestIdleCallback(run, { timeout: 400 });
      else requestAnimationFrame(run);
    }, LIQUID_GLASS_SCAN_DELAY);
  },

  _releaseAll() {
    this._freezeOwners.clear();
    this._pendingRefresh.clear();
    this._pendingScanRoots.clear();
    if (this._scanFrame) cancelAnimationFrame(this._scanFrame);
    this._scanFrame = 0;
    clearTimeout(this._scanTimer);
    this._scanTimer = 0;
    [...this._elements.keys()].forEach((element) => this.release(element));
    document.querySelectorAll(".toggle").forEach((toggle) => {
      toggle.classList.remove("is-liquid-pressing", "is-liquid-burst");
      clearTimeout(toggle._liquidPressTimer);
      clearTimeout(toggle._liquidBurstTimer);
      toggle._liquidPressStartedAt = 0;
      toggle.style.removeProperty("--toggle-touch-x");
      toggle.style.removeProperty("--toggle-touch-y");
    });
    this._filters.forEach((record) => record.node.remove());
    this._filters.clear();
  },

  /**
   * 用设备倾斜驱动高光方向，无传感器时回退到指针位置，
   * 与 AndroidLiquidGlass 的重力光源保持一致的观感。
   */
  _bindTiltLight() {
    const root = document.documentElement;
    const applyLight = (lightX, lightY) => {
      const clampedX = Math.max(-1, Math.min(1, lightX));
      const clampedY = Math.max(-1, Math.min(1, lightY));
      root.style.setProperty("--srx-lens-light-x", (50 + clampedX * 38).toFixed(2) + "%");
      root.style.setProperty("--srx-lens-light-y", (28 + clampedY * 26).toFixed(2) + "%");
      const angle = (Math.atan2(clampedY, clampedX) * 180) / Math.PI + 90;
      root.style.setProperty("--srx-lens-angle", angle.toFixed(2) + "deg");
    };
    applyLight(0, -0.6);
    let tiltActive = false;
    window.addEventListener(
      "deviceorientation",
      (event) => {
        if (event.gamma === null && event.beta === null) return;
        tiltActive = true;
        applyLight((event.gamma || 0) / 45, ((event.beta || 0) - 40) / 45);
      },
      { passive: true },
    );
    window.addEventListener(
      "pointermove",
      (event) => {
        if (tiltActive) return;
        const width = window.innerWidth || 1;
        const height = window.innerHeight || 1;
        applyLight((event.clientX / width) * 2 - 1, (event.clientY / height) * 2 - 1);
      },
      { passive: true },
    );
  },
};

/** 按象限取对应圆角半径，顺序为左上、右上、右下、左下。 */
function liquidGlassRadiusAt(x, y, radii) {
  if (x >= 0) return y <= 0 ? radii[1] : radii[2];
  return y <= 0 ? radii[0] : radii[3];
}

/** 圆角矩形有向距离场，内部为负值。 */
function liquidGlassRoundedRectSdf(x, y, halfWidth, halfHeight, radius) {
  const cornerX = Math.abs(x) - (halfWidth - radius);
  const cornerY = Math.abs(y) - (halfHeight - radius);
  const outside = Math.hypot(Math.max(cornerX, 0), Math.max(cornerY, 0)) - radius;
  const inside = Math.min(Math.max(cornerX, cornerY), 0);
  return outside + inside;
}

/** 距离场梯度，用于确定折射采样方向。 */
function liquidGlassRoundedRectGradient(x, y, halfWidth, halfHeight, radius) {
  const signX = x >= 0 ? 1 : -1;
  const signY = y >= 0 ? 1 : -1;
  const cornerX = Math.abs(x) - (halfWidth - radius);
  const cornerY = Math.abs(y) - (halfHeight - radius);
  if (cornerX >= 0 || cornerY >= 0) {
    const clampedX = Math.max(cornerX, 0);
    const clampedY = Math.max(cornerY, 0);
    const length = Math.hypot(clampedX, clampedY) || 1;
    return [(signX * clampedX) / length, (signY * clampedY) / length];
  }
  const horizontal = cornerX >= cornerY ? 1 : 0;
  return [signX * horizontal, signY * (1 - horizontal)];
}

/** 把像素位移编码为 feDisplacementMap 通道值。 */
function liquidGlassChannel(offset, scale) {
  const value = Math.round(127.5 + (offset / scale) * 255);
  return Math.max(0, Math.min(255, value));
}

LiquidGlass.init();
window.LiquidGlass = LiquidGlass;
