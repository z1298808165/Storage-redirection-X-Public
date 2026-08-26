/**
 * SRX WebUI - 液态玻璃悬浮底栏渲染层
 *
 * 按 compose-miuix-ui 示例 IosLiquidGlassNavigationBar 的分层模型重建底栏：
 * 面板层用实时 backdrop-filter 叠加圆角矩形有向距离场折射，指示器层把强调色页签
 * 副本与页面背景一起折射，边缘高光按 BloomStroke 双峰光照模型逐像素烘焙成贴图，
 * 再用 plus-lighter 叠加，等价于参考实现的 BlendMode.Plus。
 * 折射与高光算法来自 Kyant0/AndroidLiquidGlass（Apache-2.0）。
 */
(function () {
  "use strict";

  const SVG_NS = "http://www.w3.org/2000/svg";
  const FILTER_ROOT_ID = "srxNavLensFilters";

  /* 几何与材质常量，单位为 CSS px，数值与参考实现的 dp 一致。 */
  const PANEL_PADDING = 4;
  /* feGaussianBlur 用标准差表达强度，2 对应 CSS 的 blur(4px)。 */
  const PANEL_BLUR_SIGMA = 2;
  /* 不支持引用式 backdrop-filter 时没有折射补偿边缘，需要更厚的模糊撑住玻璃感。 */
  const PANEL_FALLBACK_BLUR = 14;
  const PANEL_SATURATION = 1.5;
  const PANEL_LENS_HEIGHT = 24;
  const PANEL_LENS_AMOUNT = 24;
  const PANEL_PRESS_REACH = 16;
  const PANEL_BLOOM_ALPHA = 0.75;
  const PILL_LENS_HEIGHT = 10;
  const PILL_LENS_AMOUNT = 14;
  const PILL_CHROMATIC = 0.5;
  const TAB_PRESS_SCALE = 1.2;

  /* BloomStroke(dualPeak) 参数，对应参考实现的 iosIndicatorSpecular。 */
  const BLOOM_STROKE_WIDTH = 1;
  const BLOOM_STROKE_ALPHA = 0.12;
  const BLOOM_INNER_BLUR = 2;
  const BLOOM_REF_X = 0.5;
  const BLOOM_REF_Y = 0.7;
  const BLOOM_PRIMARY_Z = -0.05;
  const BLOOM_PRIMARY_INTENSITY = 1;
  const BLOOM_SECONDARY_POSITION = [0.5, 0.8, -0.5];
  const BLOOM_SECONDARY_INTENSITY = 0.4;
  const BLOOM_PANEL_OFFSET_DEG = -45;
  const BLOOM_PILL_OFFSET_DEG = 90;
  /**
   * 光源基准方向。参考实现用重力传感器求屏幕平面内的重力角，WebUI 没有稳定的
   * 传感器授权，这里固定为设备竖持时的 90°（重力指向屏幕下方）。
   */
  const BLOOM_BASE_ANGLE_DEG = 90;

  const clamp = (value, min, max) => (value < min ? min : value > max ? max : value);

  const smoothstep = (edge0, edge1, value) => {
    const span = edge1 - edge0;
    const t = clamp(span === 0 ? (value < edge0 ? 0 : 1) : (value - edge0) / span, 0, 1);
    return t * t * (3 - 2 * t);
  };

  /** 已折叠到第一象限的圆角矩形有向距离场，内部为负值。 */
  function foldedRoundedRectSdf(x, y, halfWidth, halfHeight, radius) {
    const limited = Math.min(radius, Math.min(halfWidth, halfHeight));
    const dx = x - halfWidth + limited;
    const dy = y - halfHeight + limited;
    return Math.hypot(Math.max(dx, 0), Math.max(dy, 0)) + Math.min(Math.max(dx, dy), 0) - limited;
  }

  /** 把光源位置换算为单位方向向量，与参考实现的 applyLightUniforms 一致。 */
  function bloomLightDirection(dx, dy, dz) {
    const length = Math.max(Math.hypot(dx, dy, dz), 1e-6);
    return [dx / length, dy / length, dz / length];
  }

  /**
   * 边缘法线：把圆角矩形边缘视为半球状倒角，向内 innerBlurRadius 范围内抬起法线，
   * 内部平面区域返回朝向观察者的常量法线。
   */
  function bloomNormal(
    fragX,
    fragY,
    floorHalfWidth,
    floorHalfHeight,
    halfWidth,
    halfHeight,
    guard,
    sd,
  ) {
    const x = fragX - floorHalfWidth;
    const y = fragY - floorHalfHeight;
    const absX = Math.abs(x);
    const absY = Math.abs(y);
    const t = smoothstep(-BLOOM_INNER_BLUR, 0, sd);
    const z = -Math.sqrt(Math.max(BLOOM_INNER_BLUR * BLOOM_INNER_BLUR - t * t, 0));
    let cornerX = Math.min(halfWidth - guard, absX);
    let cornerY = Math.min(halfHeight - guard, absY);
    const dirX = absX - cornerX;
    const dirY = absY - cornerY;
    const dirLength = Math.hypot(dirX, dirY) || 1;
    cornerX += (dirX / dirLength) * (guard - BLOOM_INNER_BLUR);
    cornerY += (dirY / dirLength) * (guard - BLOOM_INNER_BLUR);
    if (absX < cornerX || absY < cornerY) return [0, 0, -1];
    const normalX = absX - cornerX;
    const normalY = absY - cornerY;
    const length = Math.hypot(normalX, normalY, z) || 1;
    return [(normalX / length) * Math.sign(x), (normalY / length) * Math.sign(y), z / length];
  }

  let bloomCanvas = null;

  function bloomContext(width, height) {
    if (!bloomCanvas) bloomCanvas = document.createElement("canvas");
    bloomCanvas.width = width;
    bloomCanvas.height = height;
    return bloomCanvas.getContext("2d", { willReadFrequently: true });
  }

  /**
   * 烘焙边缘高光贴图：RGB 固定为白光，alpha 存遮罩内的高光强度。
   *
   * 参考实现用 BlendMode.Plus 叠加自发光，CSS 对应的 `mix-blend-mode: plus-lighter`
   * 会把所在分组变成 backdrop root，使同级的 backdrop-filter 采样不到页面背景，
   * 因此这里改用普通 alpha 合成。白色高光在两种合成方式下的观感差异很小。
   */
  function buildBloomTexture(width, height, radius, angleDeg) {
    if (width < 2 || height < 2) return "";
    const context = bloomContext(width, height);
    if (!context) return "";
    const image = context.createImageData(width, height);
    const data = image.data;
    const halfWidth = width / 2;
    const halfHeight = height / 2;
    const floorHalfWidth = Math.floor(halfWidth);
    const floorHalfHeight = Math.floor(halfHeight);
    const cornerRadius = Math.min(radius, Math.min(halfWidth, halfHeight));
    const guard = Math.max(cornerRadius, BLOOM_INNER_BLUR);
    const angle = (angleDeg * Math.PI) / 180;
    const primary = bloomLightDirection(Math.cos(angle), Math.sin(angle), BLOOM_PRIMARY_Z);
    const secondary = bloomLightDirection(
      BLOOM_SECONDARY_POSITION[0] - BLOOM_REF_X,
      BLOOM_SECONDARY_POSITION[1] - BLOOM_REF_Y,
      BLOOM_SECONDARY_POSITION[2],
    );
    for (let y = 0; y < height; y += 1) {
      const fragY = y + 0.5;
      const absY = Math.abs(fragY - halfHeight);
      for (let x = 0; x < width; x += 1) {
        const fragX = x + 0.5;
        const absX = Math.abs(fragX - halfWidth);
        if (absX < halfWidth - guard && absY < halfHeight - guard) continue;
        const sd = foldedRoundedRectSdf(absX, absY, halfWidth, halfHeight, cornerRadius);
        const mask = smoothstep(0, -1, sd);
        if (mask <= 0) continue;
        const stroke = smoothstep(-BLOOM_STROKE_WIDTH, -BLOOM_STROKE_WIDTH + 1, sd);
        const normal = bloomNormal(
          fragX,
          fragY,
          floorHalfWidth,
          floorHalfHeight,
          halfWidth,
          halfHeight,
          guard,
          sd,
        );
        const primaryDot = normal[0] * primary[0] + normal[1] * primary[1];
        const secondaryDot = normal[0] * secondary[0] + normal[1] * secondary[1];
        const glow =
          BLOOM_STROKE_ALPHA * stroke * stroke +
          primaryDot * primaryDot * BLOOM_PRIMARY_INTENSITY +
          secondaryDot * secondaryDot * BLOOM_SECONDARY_INTENSITY;
        const index = (y * width + x) * 4;
        data[index] = 255;
        data[index + 1] = 255;
        data[index + 2] = 255;
        data[index + 3] = Math.round(clamp(glow, 0, 1) * mask * 255);
      }
    }
    context.putImageData(image, 0, 0);
    return bloomCanvas.toDataURL("image/png");
  }

  function filterRoot() {
    let svg = document.getElementById(FILTER_ROOT_ID);
    if (!svg) {
      svg = document.createElementNS(SVG_NS, "svg");
      svg.setAttribute("id", FILTER_ROOT_ID);
      svg.setAttribute("aria-hidden", "true");
      svg.setAttribute("width", "0");
      svg.setAttribute("height", "0");
      svg.style.cssText =
        "position:fixed;top:0;left:0;width:0;height:0;pointer-events:none;opacity:0";
      document.body.appendChild(svg);
    }
    return svg;
  }

  function createPrimitive(name, attributes) {
    const node = document.createElementNS(SVG_NS, name);
    Object.keys(attributes).forEach((key) => node.setAttribute(key, String(attributes[key])));
    return node;
  }

  /**
   * 搭建完整的玻璃效果滤镜。Chromium 的 backdrop-filter 一旦混用 `url()` 引用与
   * 内置函数就会丢掉引用，因此饱和度提升、模糊和折射必须全部放进同一个 SVG 滤镜，
   * 顺序与参考实现的 vibrancy → blur → lens 保持一致。
   *
   * `spec.pad` 把滤镜作用域向外扩出一圈，让边缘的模糊仍有真实背景可采样，
   * 避免 backdrop-filter 在圆角边缘出现透明拖影。
   */
  function buildLensFilter(id, spec) {
    const pad = spec.pad || 0;
    const filter = createPrimitive("filter", {
      id,
      filterUnits: "userSpaceOnUse",
      primitiveUnits: "userSpaceOnUse",
      "color-interpolation-filters": "sRGB",
      x: -pad,
      y: -pad,
      width: spec.width + pad * 2,
      height: spec.height + pad * 2,
    });
    let source = "SourceGraphic";
    if (spec.saturation && spec.saturation !== 1) {
      filter.appendChild(
        createPrimitive("feColorMatrix", {
          in: source,
          type: "saturate",
          values: spec.saturation,
          result: "srxNavVibrancy",
        }),
      );
      source = "srxNavVibrancy";
    }
    if (spec.blurSigma > 0) {
      filter.appendChild(
        createPrimitive("feGaussianBlur", {
          in: source,
          stdDeviation: spec.blurSigma,
          edgeMode: "duplicate",
          result: "srxNavBlur",
        }),
      );
      source = "srxNavBlur";
    }
    const maps = spec.maps;
    const displacements = [];
    maps.forEach((map, index) => {
      filter.appendChild(
        createPrimitive("feImage", {
          href: map,
          x: 0,
          y: 0,
          width: spec.width,
          height: spec.height,
          result: "srxNavMap" + index,
        }),
      );
      const displace = createPrimitive("feDisplacementMap", {
        in: source,
        in2: "srxNavMap" + index,
        scale: spec.scale,
        xChannelSelector: "R",
        yChannelSelector: "G",
        result: "srxNavPass" + index,
      });
      filter.appendChild(displace);
      displacements.push(displace);
    });
    if (maps.length > 1) {
      // 逐通道取三次采样结果，等价于参考实现的光谱分离。
      // 三个分量都保留 alpha=1：feComposite 的算术相加作用于预乘色，
      // alpha 归零会连带丢掉颜色，相加溢出的 alpha 会被自动截断到 1。
      maps.forEach((_map, index) => {
        const matrix = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0];
        matrix[index * 5 + index] = 1;
        filter.appendChild(
          createPrimitive("feColorMatrix", {
            in: "srxNavPass" + index,
            type: "matrix",
            values: matrix.join(" "),
            result: "srxNavChannel" + index,
          }),
        );
      });
      filter.appendChild(
        createPrimitive("feComposite", {
          in: "srxNavChannel0",
          in2: "srxNavChannel1",
          operator: "arithmetic",
          k1: 0,
          k2: 1,
          k3: 1,
          k4: 0,
          result: "srxNavChannelRG",
        }),
      );
      filter.appendChild(
        createPrimitive("feComposite", {
          in: "srxNavChannelRG",
          in2: "srxNavChannel2",
          operator: "arithmetic",
          k1: 0,
          k2: 1,
          k3: 1,
          k4: 0,
        }),
      );
    }
    return { node: filter, displacements };
  }

  const LiquidNav = {
    nav: null,
    panel: null,
    panelGlass: null,
    panelBloom: null,
    pill: null,
    pillGlass: null,
    pillTint: null,
    pillShade: null,
    pillAccent: null,
    pillAccentLens: null,
    pillAccentRow: null,
    pillBloom: null,
    metrics: {
      navWidth: 0,
      navHeight: 0,
      rowWidth: 0,
      tabWidth: 0,
      panelWidth: 0,
      panelHeight: 0,
      pillWidth: 0,
      pillHeight: 0,
      tabCount: 0,
    },
    _built: false,
    _supported: null,
    _enabled: true,
    _blurEnabled: true,
    _lensReady: false,
    _lensActive: false,
    _accentSignature: "",
    _panelFilter: null,
    _pillFilter: null,
    _pillAccentFilter: null,
    _pillLensScale: 0,
    _resizeObserver: null,

    /** 判断浏览器是否支持在 backdrop-filter 中引用 SVG 滤镜。 */
    isSupported() {
      if (this._supported !== null) return this._supported;
      const probe = 'blur(2px) url("#srxNavLensProbe")';
      this._supported =
        typeof CSS !== "undefined" &&
        typeof CSS.supports === "function" &&
        (CSS.supports("backdrop-filter", probe) || CSS.supports("-webkit-backdrop-filter", probe));
      return this._supported;
    },

    items() {
      return this.nav ? Array.from(this.nav.querySelectorAll(".nav-item")) : [];
    },

    init() {
      if (this._built) return;
      const nav = document.getElementById("bottomNav");
      if (!nav) return;
      this.nav = nav;
      this.build();
      this.measure();
      if (typeof ResizeObserver === "function") {
        this._resizeObserver = new ResizeObserver(() => this.measure());
        this._resizeObserver.observe(nav);
      } else {
        window.addEventListener("resize", () => this.measure(), { passive: true });
      }
    },

    /** 构建面板与指示器的分层结构，并同步强调色页签副本。 */
    build() {
      const nav = this.nav;
      if (!nav || this._built) return;
      nav
        .querySelectorAll(
          ".nav-lens-backdrop, .nav-lens-canvas, .nav-indicator, .nav-panel, .nav-pill",
        )
        .forEach((node) => node.remove());
      const layer = (className, parent) => {
        const node = document.createElement("span");
        node.className = className;
        node.setAttribute("aria-hidden", "true");
        parent.appendChild(node);
        return node;
      };
      this.panel = document.createElement("span");
      this.panel.className = "nav-panel";
      this.panel.setAttribute("aria-hidden", "true");
      nav.insertBefore(this.panel, nav.firstChild);
      this.panelGlass = layer("nav-panel-glass", this.panel);
      this.panelBloom = layer("nav-panel-bloom", this.panel);
      this.pill = document.createElement("span");
      this.pill.className = "nav-pill";
      this.pill.setAttribute("aria-hidden", "true");
      nav.appendChild(this.pill);
      this.pillGlass = layer("nav-pill-glass", this.pill);
      this.pillAccent = layer("nav-pill-accent", this.pill);
      this.pillAccentLens = layer("nav-pill-accent-lens", this.pillAccent);
      this.pillAccentRow = layer("nav-pill-accent-row", this.pillAccentLens);
      this.pillTint = layer("nav-pill-tint", this.pill);
      this.pillShade = layer("nav-pill-shade", this.pill);
      this.pillBloom = layer("nav-pill-bloom", this.pill);
      this._built = true;
      this.syncAccentRow();
    },

    /**
     * 在指示器内维护一份与真实页签等宽等距的强调色副本。指示器裁剪出当前页签，
     * 副本随折射一起弯曲，对应参考实现里被合并进指示器背景的 tabsBackdrop。
     */
    syncAccentRow() {
      if (!this.pillAccentRow) return;
      const items = this.items();
      const signature = items.map((item) => item.dataset.page || "").join("|");
      if (signature === this._accentSignature) return;
      this._accentSignature = signature;
      this.pillAccentRow.textContent = "";
      items.forEach((item) => {
        const tab = document.createElement("span");
        tab.className = "nav-accent-tab";
        Array.from(item.childNodes).forEach((child) => tab.appendChild(child.cloneNode(true)));
        this.pillAccentRow.appendChild(tab);
      });
    },

    /** 量测底栏几何并重建折射滤镜与高光贴图。 */
    measure() {
      if (!this._built || !this.nav) return;
      const nav = this.nav;
      const count = this.items().length;
      const navWidth = Math.round(nav.clientWidth);
      const navHeight = Math.round(nav.clientHeight);
      if (!count || navWidth < 40 || navHeight < 24) return;
      const rowWidth = navWidth - PANEL_PADDING * 2;
      const tabWidth = rowWidth / count;
      nav.style.setProperty("--nav-row-w", rowWidth.toFixed(3) + "px");
      nav.style.setProperty("--nav-pill-w", tabWidth.toFixed(3) + "px");
      // 底栏可能被切换成贴底铺满形态，几何直接取自布局结果而不是推算。
      const metrics = {
        navWidth,
        navHeight,
        rowWidth,
        tabWidth,
        panelWidth: Math.max(2, Math.round(this.panel.offsetWidth)),
        panelHeight: Math.max(2, Math.round(this.panel.offsetHeight)),
        pillWidth: Math.max(2, Math.round(this.pill.offsetWidth)),
        pillHeight: Math.max(2, Math.round(this.pill.offsetHeight)),
        tabCount: count,
      };
      const unchanged =
        metrics.panelWidth === this.metrics.panelWidth &&
        metrics.panelHeight === this.metrics.panelHeight &&
        metrics.pillWidth === this.metrics.pillWidth &&
        metrics.pillHeight === this.metrics.pillHeight;
      this.metrics = metrics;
      if (unchanged && this._lensReady) return;
      this.buildSurfaces();
    },

    /** 读取元素实际圆角，贴底铺满形态下会得到 0，从而按直角矩形烘焙。 */
    _cornerRadius(element, width, height) {
      const raw = getComputedStyle(element).borderTopLeftRadius || "0";
      const first = raw.trim().split(/\s+/)[0] || "0";
      const numeric = parseFloat(first) || 0;
      const value = first.endsWith("%") ? (numeric / 100) * Math.min(width, height) : numeric;
      return Math.max(0, Math.min(Math.min(width, height) / 2, Math.round(value)));
    },

    /** 生成面板与指示器的折射滤镜、边缘高光贴图，并写回 CSS 效果链。 */
    buildSurfaces() {
      const metrics = this.metrics;
      if (!metrics.panelWidth || !this.nav) return;
      const panelRadius = this._cornerRadius(
        this.panelGlass,
        metrics.panelWidth,
        metrics.panelHeight,
      );
      const pillRadius = this._cornerRadius(this.pill, metrics.pillWidth, metrics.pillHeight);
      this._setBloom(
        this.panelBloom,
        metrics.panelWidth,
        metrics.panelHeight,
        panelRadius,
        BLOOM_BASE_ANGLE_DEG + BLOOM_PANEL_OFFSET_DEG,
      );
      this._setBloom(
        this.pillBloom,
        metrics.pillWidth,
        metrics.pillHeight,
        pillRadius,
        BLOOM_BASE_ANGLE_DEG + BLOOM_PILL_OFFSET_DEG,
      );
      this._releaseFilters();
      this._lensReady = false;
      this._lensActive = false;
      this.nav.classList.toggle("nav-lens-unavailable", !this.isSupported());
      if (!this._enabled || !this.isSupported() || !window.LiquidGlass) {
        this._applyPanelEffects();
        return;
      }
      const panelMap = window.LiquidGlass.buildDisplacementMap({
        width: metrics.panelWidth,
        height: metrics.panelHeight,
        radii: [panelRadius, panelRadius, panelRadius, panelRadius],
        refractionHeight: Math.min(PANEL_LENS_HEIGHT, metrics.panelHeight / 2),
        refractionAmount: PANEL_LENS_AMOUNT,
        depthEffect: false,
      });
      const pillSpec = {
        width: metrics.pillWidth,
        height: metrics.pillHeight,
        radii: [pillRadius, pillRadius, pillRadius, pillRadius],
        refractionHeight: Math.min(PILL_LENS_HEIGHT, metrics.pillHeight / 2),
        refractionAmount: PILL_LENS_AMOUNT,
        depthEffect: true,
        chromaticAberration: PILL_CHROMATIC,
      };
      const pillMaps = [1, 0, -1].map((spectralShift) =>
        window.LiquidGlass.buildDisplacementMap(Object.assign({}, pillSpec, { spectralShift })),
      );
      if (!panelMap || pillMaps.some((map) => !map)) {
        this._applyPanelEffects();
        return;
      }
      const root = filterRoot();
      this._panelFilter = buildLensFilter("srxNavPanelLens", {
        width: metrics.panelWidth,
        height: metrics.panelHeight,
        pad: PANEL_LENS_HEIGHT + PANEL_BLUR_SIGMA * 3,
        saturation: PANEL_SATURATION,
        blurSigma: this._blurEnabled ? PANEL_BLUR_SIGMA : 0,
        scale: PANEL_LENS_AMOUNT * 2,
        maps: [panelMap],
      });
      this._pillFilter = buildLensFilter("srxNavPillLens", {
        width: metrics.pillWidth,
        height: metrics.pillHeight,
        pad: PILL_LENS_AMOUNT,
        scale: 0,
        maps: pillMaps,
      });
      this._pillAccentFilter = buildLensFilter("srxNavPillAccentLens", {
        width: metrics.pillWidth,
        height: metrics.pillHeight,
        pad: PILL_LENS_AMOUNT,
        scale: 0,
        maps: [pillMaps[1]],
      });
      root.appendChild(this._panelFilter.node);
      root.appendChild(this._pillFilter.node);
      root.appendChild(this._pillAccentFilter.node);
      this._pillLensScale = PILL_LENS_AMOUNT * 2;
      this._lensReady = true;
      this._applyPanelEffects();
    },

    _setBloom(target, width, height, radius, angleDeg) {
      if (!target) return;
      const texture = this._enabled ? buildBloomTexture(width, height, radius, angleDeg) : "";
      if (texture) target.style.backgroundImage = 'url("' + texture + '")';
      else target.style.removeProperty("background-image");
    },

    /**
     * 面板效果链：支持引用式滤镜时整条链都在 SVG 里完成，只留一个 url() 引用；
     * 不支持时退回内置的模糊与饱和度函数。
     */
    _applyPanelEffects() {
      if (!this.panelGlass) return;
      let value = "";
      if (this._enabled && this._lensReady) {
        value = 'url("#srxNavPanelLens")';
      } else if (this._enabled) {
        const blur = this._blurEnabled ? "blur(" + PANEL_FALLBACK_BLUR + "px) " : "";
        value = ("saturate(" + PANEL_SATURATION + ") " + blur).trim();
      }
      if (value) {
        this.panelGlass.style.setProperty("backdrop-filter", value);
        this.panelGlass.style.setProperty("-webkit-backdrop-filter", value);
      } else {
        this.panelGlass.style.removeProperty("backdrop-filter");
        this.panelGlass.style.removeProperty("-webkit-backdrop-filter");
      }
    },

    _releaseFilters() {
      [this._panelFilter, this._pillFilter, this._pillAccentFilter].forEach((filter) =>
        filter?.node.remove(),
      );
      this._panelFilter = null;
      this._pillFilter = null;
      this._pillAccentFilter = null;
      this.pillGlass?.style.removeProperty("backdrop-filter");
      this.pillGlass?.style.removeProperty("-webkit-backdrop-filter");
      this.pillAccentLens?.style.removeProperty("filter");
    },

    /**
     * 每帧写入动画状态。`state` 由底栏手势弹簧提供：`value` 为页签浮点索引，
     * `panelOffset` 为橡皮筋位移，`shapeX`/`shapeY` 为含速度挤压的指示器缩放。
     */
    apply(state) {
      const metrics = this.metrics;
      if (!this._built || !metrics.navWidth) return;
      const press = clamp(state.pressProgress || 0, 0, 1);
      const style = this.nav.style;
      style.setProperty("--nav-rubber-x", (state.panelOffset || 0).toFixed(3) + "px");
      style.setProperty("--nav-pill-x", ((state.value || 0) * metrics.tabWidth).toFixed(3) + "px");
      style.setProperty("--nav-pill-scale-x", (state.shapeX || 1).toFixed(4));
      style.setProperty("--nav-pill-scale-y", (state.shapeY || 1).toFixed(4));
      style.setProperty("--nav-press", press.toFixed(4));
      style.setProperty(
        "--nav-panel-scale",
        (1 + (PANEL_PRESS_REACH / metrics.navWidth) * press).toFixed(5),
      );
      style.setProperty("--nav-tab-scale", (1 + (TAB_PRESS_SCALE - 1) * press).toFixed(4));
      this._applyLens(press);
    },

    /**
     * 指示器折射只在按压时出现，强度随按压进度线性放大：位移贴图按满强度烘焙，
     * 逐帧只改 feDisplacementMap 的 scale，避免重复生成贴图。
     */
    _applyLens(press) {
      if (!this._lensReady) return;
      const active = press > 0.002;
      const scale = (this._pillLensScale * press).toFixed(3);
      this._pillFilter.displacements.forEach((node) => node.setAttribute("scale", scale));
      this._pillAccentFilter.displacements.forEach((node) => node.setAttribute("scale", scale));
      if (active === this._lensActive) return;
      this._lensActive = active;
      if (active) {
        this.pillGlass.style.setProperty("backdrop-filter", 'url("#srxNavPillLens")');
        this.pillGlass.style.setProperty("-webkit-backdrop-filter", 'url("#srxNavPillLens")');
        this.pillAccentLens.style.setProperty("filter", 'url("#srxNavPillAccentLens")');
      } else {
        this.pillGlass.style.removeProperty("backdrop-filter");
        this.pillGlass.style.removeProperty("-webkit-backdrop-filter");
        this.pillAccentLens.style.removeProperty("filter");
      }
    },

    /** 液态玻璃总开关：关闭时退回不含折射与高光的纯色底栏。 */
    setEnabled(enabled) {
      const next = !!enabled;
      if (next === this._enabled && this._built) return;
      this._enabled = next;
      if (!this._built) return;
      this.buildSurfaces();
    },

    /** 材质模糊开关：模糊在 SVG 滤镜链内部，需要整体重建。 */
    setBlurEnabled(enabled) {
      const next = !!enabled;
      if (next === this._blurEnabled) return;
      this._blurEnabled = next;
      if (!this._built) return;
      this.buildSurfaces();
    },

    /** 页签集合或主题变化后重新同步副本与贴图。 */
    refresh() {
      if (!this._built) return;
      this.syncAccentRow();
      this.buildSurfaces();
    },
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", () => LiquidNav.init(), { once: true });
  } else {
    LiquidNav.init();
  }
  window.LiquidNav = LiquidNav;
})();
