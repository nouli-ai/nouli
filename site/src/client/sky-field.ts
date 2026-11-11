import { prefersReducedMotion } from "./motion";
/**
 * One sky behind the page: each `<sky-field>` paints its slice of a slow flowing gradient fixed to
 * the viewport, so several panels read as windows onto the same sky. With the `comb` attribute it
 * also reveals a hidden honeycomb around the pointer. Without WebGL the panel keeps its CSS
 * background.
 */
const FRAGMENT = `precision mediump float;
uniform vec2 o; uniform vec2 v; uniform float t; uniform vec2 m;
uniform vec3 c0; uniform vec3 c1; uniform vec3 c2; uniform vec3 c3;
float h(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
float n(vec2 p){vec2 i=floor(p),f=fract(p);f=f*f*(3.-2.*f);
 return mix(mix(h(i),h(i+vec2(1,0)),f.x),mix(h(i+vec2(0,1)),h(i+vec2(1,1)),f.x),f.y);}
float fbm(vec2 p){float r=0.,a=.5;for(int i=0;i<5;i++){r+=a*n(p);p*=2.02;a*=.5;}return r;}
void main(){
 vec2 uv=(gl_FragCoord.xy+o)/v; vec2 p=uv*vec2(v.x/v.y,1.)*1.4;
 float q=fbm(p+vec2(t*.025,t*.018)+m*.25);
 float w=fbm(p*1.25+q*1.9-vec2(t*.018,-t*.01));
 vec3 col=mix(c0,c1,smoothstep(0.,1.,uv.y+w*.4-.2));
 col=mix(col,c2,smoothstep(.45,.85,w)*.75);
 col=mix(col,c3,smoothstep(.5,.95,q)*(1.-uv.y*.6)*.7);
 col+=(h(gl_FragCoord.xy+fract(t))-.5)*.035;
 gl_FragColor=vec4(col,1.);
}`;
const VERTEX = "attribute vec2 p;void main(){gl_Position=vec4(p,0.,1.);}";
// The sky is soft, so half resolution is enough.
const SCALE = 0.5;
const HEX = 20;
const reducedQuery = matchMedia("(prefers-reduced-motion: reduce)");

const rgb = (hex: string): [number, number, number] => {
  const value = Number.parseInt(hex.replace("#", ""), 16);
  return [(value >> 16) / 255, ((value >> 8) & 255) / 255, (value & 255) / 255];
};

class SkyField extends HTMLElement {
  #controller: AbortController | undefined;
  #view: IntersectionObserver | undefined;
  #frame: number | undefined;
  #visible = false;
  #lastTop = Number.NaN;
  #rect: DOMRect = new DOMRect();
  #resize: ResizeObserver | undefined;
  #colors = { line: "", glow: "" };
  #pointer = { x: 0, y: 0, tx: 0, ty: 0 };
  #comb = { x: 0, y: 0, k: 0, on: false };
  #draw: ((now: number) => void) | undefined;
  #combCanvas: HTMLCanvasElement | undefined;

  connectedCallback(): void {
    this.#controller = new AbortController();
    const { signal } = this.#controller;
    const style = getComputedStyle(this);
    this.#colors = {
      line: style.getPropertyValue("--net-comb").trim(),
      glow: style.getPropertyValue("--net-glow").trim(),
    };
    this.#rect = this.getBoundingClientRect();
    this.#resize = new ResizeObserver(() => {
      this.#rect = this.getBoundingClientRect();
      this.#lastTop = Number.NaN;
      this.#schedule();
    });
    this.#resize.observe(this);
    const canvas = document.createElement("canvas");
    this.append(canvas);
    const gl = canvas.getContext("webgl", { antialias: false });
    if (gl) this.#draw = this.#sky(gl, canvas);
    if (this.hasAttribute("comb")) {
      this.#combCanvas = document.createElement("canvas");
      this.append(this.#combCanvas);
    }
    this.#bindPointer(signal);
    this.#observe(signal);
  }

  #bindPointer(signal: AbortSignal): void {
    addEventListener(
      "pointermove",
      (event) => {
        this.#pointer.tx = event.clientX / innerWidth - 0.5;
        this.#pointer.ty = 0.5 - event.clientY / innerHeight;
        if (!this.#combCanvas || event.pointerType === "touch") return;
        const rect = this.#rect;
        const inside =
          event.clientX >= rect.left &&
          event.clientX <= rect.right &&
          event.clientY >= rect.top &&
          event.clientY <= rect.bottom;
        this.#comb.on = inside;
        if (inside) {
          if (this.#comb.k < 0.02)
            Object.assign(this.#comb, { x: event.clientX, y: event.clientY });
          this.#comb.x += (event.clientX - this.#comb.x) * (prefersReducedMotion() ? 1 : 0.35);
          this.#comb.y += (event.clientY - this.#comb.y) * (prefersReducedMotion() ? 1 : 0.35);
        }
        this.#schedule();
      },
      { signal },
    );
  }

  #observe(signal: AbortSignal): void {
    this.#view = new IntersectionObserver(([entry]) => {
      this.#visible = entry?.isIntersecting ?? false;
      this.#schedule();
    });
    this.#view.observe(this);
    const schedule = (): void => {
      this.#schedule();
    };
    document.addEventListener("visibilitychange", schedule, { signal });
    const refresh = (): void => {
      this.#rect = this.getBoundingClientRect();
      schedule();
    };
    addEventListener("scroll", refresh, { signal, passive: true });
    addEventListener("resize", refresh, { signal, passive: true });
    document.addEventListener(
      "animation:change",
      () => {
        this.#lastTop = Number.NaN;
        schedule();
      },
      { signal },
    );
    document.addEventListener(
      "pointerleave",
      () => {
        this.#comb.on = false;
        schedule();
      },
      { signal },
    );
    reducedQuery.addEventListener("change", schedule, { signal });
  }

  disconnectedCallback(): void {
    this.#controller?.abort();
    this.#view?.disconnect();
    this.#resize?.disconnect();
    if (this.#frame !== undefined) cancelAnimationFrame(this.#frame);
    this.#frame = undefined;
    this.replaceChildren();
  }

  /** Run every frame while visible and moving; with reduced motion, repaint only when scrolled. */
  #schedule(): void {
    if (this.#frame !== undefined || !this.#visible || document.hidden) return;
    this.#frame = requestAnimationFrame((now) => {
      this.#frame = undefined;
      const top = this.#rect.top;
      const moved = top !== this.#lastTop;
      this.#lastTop = top;
      if (!prefersReducedMotion() || moved) this.#draw?.(now);
      this.#paintComb();
      if (!prefersReducedMotion()) this.#schedule();
    });
  }

  #sky(gl: WebGLRenderingContext, canvas: HTMLCanvasElement): (now: number) => void {
    const shader = (type: number, source: string): WebGLShader | null => {
      const created = gl.createShader(type);
      if (!created) return null;
      gl.shaderSource(created, source);
      gl.compileShader(created);
      return created;
    };
    const program = gl.createProgram();
    const vertex = shader(gl.VERTEX_SHADER, VERTEX);
    const fragment = shader(gl.FRAGMENT_SHADER, FRAGMENT);
    if (vertex) gl.attachShader(program, vertex);
    if (fragment) gl.attachShader(program, fragment);
    gl.linkProgram(program);
    gl.useProgram(program);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const position = gl.getAttribLocation(program, "p");
    gl.enableVertexAttribArray(position);
    gl.vertexAttribPointer(position, 2, gl.FLOAT, false, 0, 0);
    const uniform = (name: string): WebGLUniformLocation | null =>
      gl.getUniformLocation(program, name);
    const style = getComputedStyle(this);
    ["--sky-low", "--sky-high", "--sky-violet", "--sky-rose"].forEach((name, i) => {
      gl.uniform3fv(uniform(`c${i}`), rgb(style.getPropertyValue(name).trim()));
    });
    const start = performance.now();
    return (now) => {
      const rect = this.#rect;
      const [w, h] = [Math.round(rect.width * SCALE), Math.round(rect.height * SCALE)];
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
        gl.viewport(0, 0, w, h);
      }
      const p = this.#pointer;
      p.x += (p.tx - p.x) * 0.03;
      p.y += (p.ty - p.y) * 0.03;
      gl.uniform2f(uniform("o"), rect.left * SCALE, (innerHeight - rect.bottom) * SCALE);
      gl.uniform2f(uniform("v"), innerWidth * SCALE, innerHeight * SCALE);
      gl.uniform1f(uniform("t"), prefersReducedMotion() ? 12 : (now - start) / 1000);
      gl.uniform2f(uniform("m"), p.x, p.y);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
  }

  /** Draw the honeycomb cells near the pointer, on the page-fixed grid the networks share. */
  #paintComb(): void {
    const canvas = this.#combCanvas;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    const comb = this.#comb;
    comb.k += ((comb.on ? 1 : 0) - comb.k) * (prefersReducedMotion() ? 1 : 0.08);
    const rect = this.#rect;
    const dpr = Math.min(devicePixelRatio, 2);
    if (canvas.width !== Math.round(rect.width * dpr)) canvas.width = Math.round(rect.width * dpr);
    if (canvas.height !== Math.round(rect.height * dpr))
      canvas.height = Math.round(rect.height * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, rect.width, rect.height);
    if (comb.k < 0.004) return;
    const { line, glow } = this.#colors;
    const [left, top] = [rect.left + scrollX, rect.top + scrollY];
    const [px, py] = [comb.x - rect.left, comb.y - rect.top];
    const reach = 95;
    const hw = Math.sqrt(3) * HEX;
    const rowH = HEX * 1.5;
    ctx.lineWidth = 1;
    for (let row = Math.floor((top + py - reach) / rowH); row * rowH - top <= py + reach; row++) {
      const y = row * rowH - top;
      const shift = (row % 2) * (hw / 2);
      for (
        let col = Math.floor((left + px - reach - shift) / hw);
        col * hw + shift - left <= px + reach;
        col++
      ) {
        const x = col * hw + shift - left;
        const a = comb.k * Math.exp(-((x - px) ** 2 + (y - py) ** 2) / (2 * (reach / 2.2) ** 2));
        if (a < 0.02) continue;
        ctx.beginPath();
        for (let i = 0; i < 6; i++) {
          const angle = (Math.PI / 3) * i + Math.PI / 6;
          ctx.lineTo(x + (HEX - 1.5) * Math.cos(angle), y + (HEX - 1.5) * Math.sin(angle));
        }
        ctx.closePath();
        ctx.strokeStyle = `rgba(${line},${0.4 * a})`;
        ctx.stroke();
        if (a > 0.7) {
          ctx.fillStyle = `rgba(${glow},${(a - 0.7) * 0.12})`;
          ctx.fill();
        }
      }
    }
  }
}

customElements.define("sky-field", SkyField);
