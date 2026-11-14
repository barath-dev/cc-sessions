'use strict';

// The 3D usage model: a slab with a block for each day, as tall as its tokens, lit
// from one side, drawn on a 2D canvas. The page (concept.js) hands it numbers,
// strings and colours; it draws, and reports pointing, picking and the camera back.
//
//   const scene = ccSky.create(canvas, { onHover, onSelect, onCursor, onCamera, onSpin, onDragEnd, isFullscreen });
//   scene.setData({ cols, cells: [{ value, level, future }], colLabels, rowLabels, callouts, ticks, most });
//   scene.setPalette({ steps, ink, muted, halo, base, dark, font });
//   scene.view('angled' | 'front' | 'top' | 'side' | 'recent'); scene.enter(); scene.destroy();
//
// Blocks are drawn far to near by grid order from the eye (never sorted by depth),
// which is exactly right for boxes on a grid seen from anywhere above the floor.
(() => {
  const ROWS = 7;
  const TALLEST = 9;       // grid units, for the busiest day
  const GAP = 0.09;
  const DISTANCE = 150;
  const SLAB = 0.6;        // how far the slab reaches past the blocks
  const LIGHT = (() => { const l = Math.hypot(-0.45, 0.85, 0.35); return [-0.45 / l, 0.85 / l, 0.35 / l]; })();
  // Top, +x, -x, +z, -z.
  const NORMALS = [[0, 1, 0], [1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]];
  const LIT = NORMALS.map((n) => Math.max(0, n[0] * LIGHT[0] + n[1] * LIGHT[1] + n[2] * LIGHT[2]));
  // Each view's angle; its zoom is worked out to fit the canvas. `weeks` narrows it to the last few.
  const PRESETS = {
    angled: { yaw: 28, pitch: 32 },
    front: { yaw: 0, pitch: 16 },
    top: { yaw: 0, pitch: 89 },
    side: { yaw: 90, pitch: 12 },
    recent: { yaw: 24, pitch: 34, weeks: 5 },
  };
  const reduced = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
  const ease = (t) => 1 - (1 - t) ** 3;
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  const smooth = (lo, hi, v) => { const t = clamp((v - lo) / (hi - lo), 0, 1); return t * t * (3 - 2 * t); };

  function create(canvas, opts = {}) {
    const ctx = canvas.getContext('2d');
    let data = { cols: 0, cells: [], colLabels: [], rowLabels: [], callouts: [], ticks: [], most: 1 };
    let boxes = [];
    let palette = null;
    let colours = null;      // [level][face] → css, and dimmed versions
    let width = 0;
    let height = 0;
    let ratio = 1;
    const cur = { yaw: 28, pitch: 32, zoom: 1, panX: 0, panZ: 0, shift: 0 };   // shift: px the picture sits below the middle
    let tween = null;        // { from, to, start, ms }
    let rising = null;       // 0..1 while the blocks grow in
    let spin = false;
    let vel = { yaw: 0, pitch: 0 };
    let hover = null;
    let selected = null;
    let cursor = null;
    let pointer = null;      // last pointer position over the canvas
    let dragging = null;
    let frame = 0;
    let dirty = true;
    let visible = true;
    let last = 0;
    let preset = 'angled';
    let drawn = [];          // faces in draw order, for picking: { i, pts }
    const widths = new Map();

    // ------------------------------------------------------------ the camera

    const center = () => [data.cols / 2 + cur.panX, 1.2, 3.5 + cur.panZ];
    const lens = () => {
      const yaw = (cur.yaw * Math.PI) / 180;
      const pitch = (cur.pitch * Math.PI) / 180;
      return { cy: Math.cos(yaw), sy: Math.sin(yaw), cp: Math.cos(pitch), sp: Math.sin(pitch),
        focal: (cur.zoom * 0.92 * width * DISTANCE) / (data.cols + 4 || 1), c: center() };
    };
    const project = (L, x, y, z) => {
      const px = x - L.c[0];
      const py = y - L.c[1];
      const pz = z - L.c[2];
      const rx = px * L.cy - pz * L.sy;
      const rz = px * L.sy + pz * L.cy;
      const ry = py * L.cp - rz * L.sp;
      const depth = DISTANCE - (py * L.sp + rz * L.cp);
      return [width / 2 + (L.focal * rx) / depth, height / 2 + cur.shift - (L.focal * ry) / depth, depth];
    };
    const eyeOf = (L) => [L.c[0] + DISTANCE * L.cp * L.sy, L.c[1] + DISTANCE * L.sp, L.c[2] + DISTANCE * L.cp * L.cy];
    // The world point on the floor under a screen point, for zooming and panning by the pointer.
    const groundAt = (L, sx, sy) => {
      const X = (sx - width / 2) / L.focal;
      const Y = -(sy - height / 2 - cur.shift) / L.focal;
      const s = (-L.c[1] - DISTANCE * L.sp) / (Y * L.cp - L.sp);
      if (!(s > 0)) return null;
      const rx = X * s;
      const ry = Y * s;
      const rzp = DISTANCE - s;
      const rz = -ry * L.sp + rzp * L.cp;
      return [L.c[0] + rx * L.cy + rz * L.sy, L.c[2] - rx * L.sy + rz * L.cy];
    };

    const setPreset = (name) => {
      if (preset === name) return;
      preset = name;
      if (opts.onCamera) opts.onCamera(name);
    };
    // The zoom and shift at which the slab between columns c0 and c1, and its tallest block
    // there, fills the canvas for this camera. Screen offsets from the centre grow with the zoom.
    const fit = (cam, c0, c1) => {
      if (!width) return { zoom: 1, shift: 0 };
      const tallest = boxes.reduce((m, b) => (b.c >= c0 && b.c < c1 && !b.future ? Math.max(m, b.h) : m), 1);
      const saved = { ...cur };
      Object.assign(cur, cam, { zoom: 1, shift: 0 });
      const L = lens();
      let zoom = 4;
      let top = Infinity;
      let bottom = -Infinity;
      for (const x of [c0 - SLAB, c1 + SLAB]) for (const y of [-0.55, tallest]) for (const z of [-SLAB, ROWS + SLAB]) {
        const [sx, sy] = project(L, x, y, z);
        const dx = Math.abs(sx - width / 2);
        if (dx > 0.5) zoom = Math.min(zoom, (width / 2 - 60) / dx);
        top = Math.min(top, sy - height / 2);
        bottom = Math.max(bottom, sy - height / 2);
      }
      Object.assign(cur, saved);
      // Room above for callouts, below for the month names.
      if (bottom - top > 0.5) zoom = Math.min(zoom, (height - 36 - 30) / (bottom - top));
      zoom = clamp(zoom, 0.6, 4);
      return { zoom, shift: 3 - (zoom * (top + bottom)) / 2 };
    };
    const target = (name) => {
      const { yaw, pitch, weeks } = PRESETS[name];
      const c0 = weeks ? Math.max(0, data.cols - weeks) : 0;
      const goal = { yaw, pitch, panX: (c0 + data.cols) / 2 - data.cols / 2, panZ: 0 };
      return { ...goal, ...fit(goal, c0, data.cols) };
    };
    // After a resize or new numbers, a view still in place is fitted again.
    const refit = () => {
      if (!preset || dragging) return;
      const { zoom, shift } = target(preset);
      Object.assign(tween ? tween.to : cur, { zoom, shift });
    };
    // Move to `to` over `ms`, the short way round.
    const animateTo = (to, ms = 700) => {
      rising = null;
      const from = { ...cur };
      const goal = { ...cur, ...to };
      goal.yaw = from.yaw + ((((goal.yaw - from.yaw) % 360) + 540) % 360) - 180;
      if (reduced() || ms <= 0) {
        Object.assign(cur, goal);
        tween = null;
      } else {
        tween = { from, to: goal, start: performance.now(), ms };
      }
      invalidate();
    };
    const stopMotion = () => {
      tween = null;
      rising = null;   // an entrance cut short: the blocks stand at full height
      vel = { yaw: 0, pitch: 0 };
      if (spin) setSpin(false);
    };
    const setSpin = (on) => {
      spin = on && !reduced();
      if (opts.onSpin) opts.onSpin(spin);
      invalidate();
    };

    // ------------------------------------------------------------ the loop

    const invalidate = () => {
      dirty = true;
      if (!frame && visible) frame = requestAnimationFrame(step);
    };
    const step = (now) => {
      frame = 0;
      const dt = last ? Math.min(0.05, (now - last) / 1000) : 0.016;
      last = now;
      let moving = false;
      if (tween) {
        const t = clamp((now - tween.start) / tween.ms, 0, 1);
        const k = ease(t);
        for (const key of Object.keys(tween.to)) cur[key] = tween.from[key] + (tween.to[key] - tween.from[key]) * k;
        if (tween.grow) rising = t;
        if (t >= 1) { tween = null; rising = null; }
        moving = true;
      }
      if (!dragging && (Math.abs(vel.yaw) > 0.5 || Math.abs(vel.pitch) > 0.5)) {
        cur.yaw += vel.yaw * dt;
        cur.pitch = clamp(cur.pitch + vel.pitch * dt, 8, 89);
        const keep = Math.exp(-dt / 0.25);
        vel.yaw *= keep;
        vel.pitch *= keep;
        moving = true;
      }
      if (spin && !dragging && !tween) {
        cur.yaw += 7 * dt;   // degrees a second
        moving = true;
      }
      cur.yaw = ((cur.yaw % 360) + 360) % 360;
      if (moving || dirty) {
        dirty = false;
        draw();
        // The pointer may now be over a different block.
        if (pointer && moving && !dragging) pick(pointer.x, pointer.y, pointer.event);
      }
      last = moving ? now : 0;
      if (moving && visible) frame = requestAnimationFrame(step);
    };

    // ------------------------------------------------------------ data, colours

    const setData = (next) => {
      data = next;
      boxes = data.cells.map((cell, i) => {
        const c = Math.floor(i / ROWS);
        const r = i % ROWS;
        return { i, c, r, x0: c + GAP, x1: c + 1 - GAP, z0: r + GAP, z1: r + 1 - GAP, future: cell.future,
          h: cell.value ? Math.max(0.14, (cell.value / data.most) * TALLEST) : 0.05, level: cell.level, used: cell.value > 0 };
      });
      if (selected !== null && !data.cells[selected]) selected = null;
      refit();
      invalidate();
    };
    const rgb = ([r, g, b], k = 1) => `rgb(${Math.min(255, Math.round(r * k))}, ${Math.min(255, Math.round(g * k))}, ${Math.min(255, Math.round(b * k))})`;
    const mix = (a, b, t) => a.map((v, n) => v + (b[n] - v) * t);
    const lit = (normal) => {
      const d = Math.max(0, normal[0] * LIGHT[0] + normal[1] * LIGHT[1] + normal[2] * LIGHT[2]);
      return (palette.dark ? 0.5 : 0.56) + (palette.dark ? 0.62 : 0.5) * d + (normal[1] ? 0.08 : 0);
    };
    const setPalette = (next) => {
      palette = next;
      const build = (dim) => next.steps.map((step) => NORMALS.map((normal) => rgb(dim ? mix(step, next.base, 0.6) : step, lit(normal))));
      colours = { plain: build(false), dim: build(true),
        slab: NORMALS.map((normal) => rgb(next.base, lit(normal) + (normal[1] ? 0.04 : -0.05))) };
      widths.clear();
      invalidate();
    };

    // ------------------------------------------------------------ drawing

    const path = (pts) => {
      ctx.beginPath();
      ctx.moveTo(pts[0][0], pts[0][1]);
      for (let n = 1; n < pts.length; n++) ctx.lineTo(pts[n][0], pts[n][1]);
      ctx.closePath();
    };
    // A box's visible faces as [face, points], seen from `eye`.
    const facesOf = (L, eye, x0, x1, y0, y1, z0, z1) => {
      const out = [];
      if (eye[1] > y1) out.push([0, [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]]]);
      if (eye[0] > x1) out.push([1, [[x1, y0, z1], [x1, y1, z1], [x1, y1, z0], [x1, y0, z0]]]);
      if (eye[0] < x0) out.push([2, [[x0, y0, z0], [x0, y1, z0], [x0, y1, z1], [x0, y0, z1]]]);
      if (eye[2] > z1) out.push([3, [[x0, y0, z1], [x0, y1, z1], [x1, y1, z1], [x1, y0, z1]]]);
      if (eye[2] < z0) out.push([4, [[x1, y0, z0], [x1, y1, z0], [x0, y1, z0], [x0, y0, z0]]]);
      return out.map(([face, corners]) => [face, corners.map(([x, y, z]) => project(L, x, y, z))]);
    };
    const heightOf = (b) => {
      if (rising === null) return b.h;
      const local = clamp((rising - (b.c / data.cols) * 0.45) / 0.55, 0, 1);
      return Math.max(0.02, b.h * ease(local));
    };
    // Rows far to near, and in each row columns far to near: nothing drawn later can be behind.
    const order = (eye) => {
      const rows = [...Array(ROWS).keys()].sort((a, b) => Math.abs(b + 0.5 - eye[2]) - Math.abs(a + 0.5 - eye[2]));
      const cols = [...Array(data.cols).keys()].sort((a, b) => Math.abs(b + 0.5 - eye[0]) - Math.abs(a + 0.5 - eye[0]));
      const out = [];
      for (const r of rows) for (const c of cols) out.push(c * ROWS + r);
      return out;
    };
    // Text with a halo in the card's colour, so it reads over blocks and slab alike.
    const label = (text, x, y, align = 'center', strong = false) => {
      ctx.font = `${strong ? 600 : 500} 11px ${palette.font}`;
      ctx.textAlign = align;
      ctx.lineJoin = 'round';
      ctx.lineWidth = 3;
      ctx.strokeStyle = palette.halo;
      ctx.strokeText(text, Math.round(x), Math.round(y));
      ctx.fillStyle = strong ? palette.ink : palette.muted;
      ctx.fillText(text, Math.round(x), Math.round(y));
    };
    const measure = (text, strong = false) => {
      const key = `${strong}|${text}`;
      if (!widths.has(key)) {
        ctx.font = `${strong ? 600 : 500} 11px ${palette.font}`;
        widths.set(key, ctx.measureText(text).width);
      }
      return widths.get(key);
    };
    const free = (placed, rect) => {
      if (rect.x < 2 || rect.x + rect.w > width - 2 || rect.y < 2 || rect.y + rect.h > height - 2) return false;
      return !placed.some((p) => rect.x < p.x + p.w + 6 && p.x < rect.x + rect.w + 6 && rect.y < p.y + p.h + 2 && p.y < rect.y + rect.h + 2);
    };

    const draw = () => {
      if (!palette || !width) return;
      ctx.clearRect(0, 0, width, height);
      const L = lens();
      const eye = eyeOf(L);
      const cols = data.cols;
      drawn = [];

      // The slab.
      for (const [face, pts] of facesOf(L, eye, -SLAB, cols + SLAB, -0.55, 0, -SLAB, ROWS + SLAB)) {
        path(pts);
        ctx.fillStyle = colours.slab[face];
        ctx.fill();
      }
      // Pointing: faint strips along the hovered day's week and weekday.
      const focus = hover !== null ? boxes[hover] : null;
      if (focus) {
        ctx.fillStyle = palette.dark ? 'rgb(255 255 255 / 0.06)' : 'rgb(0 0 0 / 0.05)';
        for (const strip of [[focus.c, focus.c + 1, -SLAB, ROWS + SLAB], [-SLAB, cols + SLAB, focus.r, focus.r + 1]]) {
          path([[strip[0], 0, strip[2]], [strip[1], 0, strip[2]], [strip[1], 0, strip[3]], [strip[0], 0, strip[3]]].map(([x, y, z]) => project(L, x, y, z)));
          ctx.fill();
        }
      }
      // Shadows, all in one path so where they overlap they stay one shade, and only on the slab.
      ctx.save();
      path([[-SLAB, 0, -SLAB], [cols + SLAB, 0, -SLAB], [cols + SLAB, 0, ROWS + SLAB], [-SLAB, 0, ROWS + SLAB]].map(([x, y, z]) => project(L, x, y, z)));
      ctx.clip();
      const dxk = (-LIGHT[0] / LIGHT[1]) * 0.6;
      const dzk = (-LIGHT[2] / LIGHT[1]) * 0.6;
      ctx.beginPath();
      for (const b of boxes) {
        if (!b.used || b.future) continue;
        const h = heightOf(b);
        const dx = dxk * h;
        const dz = dzk * h;
        // The outline of the footprint swept along the shadow: a hexagon whose corners depend on the light's side.
        const hull = dx >= 0
          ? (dz <= 0
            ? [[b.x0, b.z1], [b.x0, b.z0], [b.x0 + dx, b.z0 + dz], [b.x1 + dx, b.z0 + dz], [b.x1 + dx, b.z1 + dz], [b.x1, b.z1]]
            : [[b.x0, b.z0], [b.x1, b.z0], [b.x1 + dx, b.z0 + dz], [b.x1 + dx, b.z1 + dz], [b.x0 + dx, b.z1 + dz], [b.x0, b.z1]])
          : (dz <= 0
            ? [[b.x1, b.z1], [b.x0, b.z1], [b.x0 + dx, b.z1 + dz], [b.x0 + dx, b.z0 + dz], [b.x1 + dx, b.z0 + dz], [b.x1, b.z0]]
            : [[b.x1, b.z0], [b.x1, b.z1], [b.x1 + dx, b.z1 + dz], [b.x0 + dx, b.z1 + dz], [b.x0 + dx, b.z0 + dz], [b.x0, b.z0]]);
        hull.forEach(([x, z], n) => { const [sx, sy] = project(L, x, 0, z); if (n) ctx.lineTo(sx, sy); else ctx.moveTo(sx, sy); });
        ctx.closePath();
      }
      ctx.fillStyle = palette.dark ? 'rgb(0 0 0 / 0.3)' : 'rgb(0 0 0 / 0.12)';
      ctx.fill('nonzero');
      ctx.restore();

      const placed = [];   // label boxes so far, so later ones keep clear
      // The height ruler, behind the blocks: dashed lines along the far long edge.
      const rulerAlpha = (1 - smooth(68, 80, cur.pitch)) * (rising === null ? 1 : rising);
      if (rulerAlpha > 0.02 && data.ticks.length) {
        const farZ = eye[2] > 3.5 ? -SLAB : ROWS + SLAB;
        ctx.save();
        ctx.globalAlpha = rulerAlpha;
        ctx.setLineDash([3, 4]);
        ctx.lineWidth = 1;
        ctx.strokeStyle = palette.muted;
        for (const tick of data.ticks) {
          const y = (tick.value / data.most) * TALLEST;
          const a = project(L, -SLAB, y, farZ);
          const b = project(L, cols + SLAB, y, farZ);
          ctx.globalAlpha = rulerAlpha * 0.45;
          ctx.beginPath(); ctx.moveTo(a[0], a[1]); ctx.lineTo(b[0], b[1]); ctx.stroke();
          ctx.globalAlpha = rulerAlpha;
          const end = Math.abs(a[0] - width / 2) > Math.abs(b[0] - width / 2) ? a : b;
          const left = end[0] < width / 2;
          const w = measure(tick.text);
          const x = end[0] + (left ? -6 : 6);
          placed.push({ x: left ? x - w : x, y: end[1] - 6, w, h: 12 });
          label(tick.text, x, end[1] + 4, left ? 'right' : 'left');
        }
        ctx.restore();
      }

      // The blocks, far to near.
      const anyPicked = selected !== null;
      for (const i of order(eye)) {
        const b = boxes[i];
        if (!b || b.future) continue;
        const h = heightOf(b);
        const faces = facesOf(L, eye, b.x0, b.x1, 0, h, b.z0, b.z1);
        const set = anyPicked && i !== selected ? colours.dim : colours.plain;
        const shades = set[b.level];
        // Under each box its darkest face, a touch wider: hides seams and draws a fine edge.
        let darkest = -1;
        for (const [face] of faces) if (face && (darkest < 0 || LIT[face] < LIT[darkest])) darkest = face;
        if (darkest > 0) {
          ctx.lineWidth = 0.8;
          ctx.strokeStyle = shades[darkest];
          for (const [, pts] of faces) { path(pts); ctx.stroke(); }
        }
        for (const [face, pts] of faces) {
          path(pts);
          ctx.fillStyle = shades[face];
          ctx.fill();
          if (i === hover && face === 0) {
            ctx.fillStyle = palette.dark ? 'rgb(255 255 255 / 0.18)' : 'rgb(255 255 255 / 0.28)';
            ctx.fill();
          }
          drawn.push({ i, pts });
        }
        // The top's outline for the hovered, the picked and the keyboard's day.
        if (i === hover || i === selected || i === cursor) {
          const top = faces.find(([face]) => face === 0);
          if (top) {
            path(top[1]);
            ctx.lineWidth = i === selected ? 2 : 1.25;
            ctx.setLineDash(i === cursor && i !== selected ? [3, 2] : []);
            ctx.strokeStyle = palette.ink;
            ctx.stroke();
            ctx.setLineDash([]);
          }
        }
      }

      // Labels on the edges that face the eye, over everything.
      const monthsAlpha = smooth(0.12, 0.4, Math.abs(L.cy)) || (cur.pitch > 70 ? 1 : 0);
      const daysAlpha = smooth(0.12, 0.4, Math.abs(L.sy)) || (cur.pitch > 70 ? 1 : 0);
      ctx.save();
      if (monthsAlpha > 0.02) {
        ctx.globalAlpha = monthsAlpha;
        const z = eye[2] > 3.5 ? ROWS + SLAB + 0.9 : -SLAB - 0.9;
        // The year's first and January first, so they survive thinning.
        const ranked = [...data.colLabels].sort((a, b) => (b.strong ? 1 : 0) - (a.strong ? 1 : 0));
        for (const m of ranked) {
          const [x, y] = project(L, m.col + 0.5, 0, z);
          const w = measure(m.text, m.strong);
          const rect = { x: x - w / 2, y: y - 9, w, h: 12 };
          if (!free(placed, rect)) continue;
          placed.push(rect);
          label(m.text, x, y, 'center', m.strong);
        }
      }
      if (daysAlpha > 0.02) {
        ctx.globalAlpha = daysAlpha;
        const x = eye[0] > cols / 2 ? cols + SLAB + 0.9 : -SLAB - 0.9;
        for (const d of data.rowLabels) {
          const [sx, sy] = project(L, x, 0, d.row + 0.5);
          const w = measure(d.text);
          const right = sx < width / 2;
          const rect = { x: right ? sx - w : sx, y: sy - 8, w, h: 12 };
          if (!free(placed, rect)) continue;
          placed.push(rect);
          label(d.text, sx, sy + 3, right ? 'right' : 'left');
        }
      }
      ctx.restore();

      // Callouts: the busiest day, today. A short leader up from the block's top.
      for (const call of data.callouts) {
        const b = boxes[call.index];
        if (!b || b.future) continue;
        const h = heightOf(b);
        const [x, y] = project(L, (b.x0 + b.x1) / 2, h, (b.z0 + b.z1) / 2);
        const lift = call.strong ? 26 : 16;
        const w = measure(call.text, call.strong);
        const rect = { x: x - w / 2, y: y - lift - 13, w, h: 13 };
        if (!free(placed, rect)) continue;
        placed.push(rect);
        ctx.strokeStyle = palette.muted;
        ctx.lineWidth = 1;
        ctx.beginPath(); ctx.moveTo(Math.round(x) + 0.5, y - 2); ctx.lineTo(Math.round(x) + 0.5, y - lift + 2); ctx.stroke();
        label(call.text, x, y - lift - 2, 'center', call.strong);
      }
    };

    // ------------------------------------------------------------ pointing

    const inside = (pts, x, y) => {
      let hit = false;
      for (let k = 0, j = pts.length - 1; k < pts.length; j = k++) {
        const [xk, yk] = pts[k];
        const [xj, yj] = pts[j];
        if ((yk > y) !== (yj > y) && x < ((xj - xk) * (y - yk)) / (yj - yk) + xk) hit = !hit;
      }
      return hit;
    };
    const hitAt = (x, y) => {
      for (let n = drawn.length - 1; n >= 0; n--) if (inside(drawn[n].pts, x, y)) return drawn[n].i;
      return null;
    };
    const pick = (x, y, event) => {
      const i = hitAt(x, y);
      const real = i !== null && boxes[i] && !boxes[i].future ? i : null;
      if (real !== hover) {
        hover = real;
        invalidate();
      }
      if (opts.onHover) opts.onHover(real, event);
    };
    const local = (event) => {
      const box = canvas.getBoundingClientRect();
      return { x: event.clientX - box.left, y: event.clientY - box.top };
    };

    const onDown = (event) => {
      if (event.button !== 0) return;
      canvas.setPointerCapture(event.pointerId);
      const { x, y } = local(event);
      dragging = { x, y, startX: x, startY: y, time: performance.now(), pan: event.shiftKey, moved: false, last: performance.now() };
      stopMotion();
      if (opts.onHover) opts.onHover(null, event);
    };
    const onMove = (event) => {
      const { x, y } = local(event);
      pointer = { x, y, event };
      if (!dragging) { pick(x, y, event); return; }
      const dx = x - dragging.x;
      const dy = y - dragging.y;
      if (Math.hypot(x - dragging.startX, y - dragging.startY) > 4) dragging.moved = true;
      const now = performance.now();
      const dt = Math.max(1, now - dragging.last) / 1000;
      if (dragging.pan) {
        const L = lens();
        const a = groundAt(L, dragging.x, dragging.y);
        const b = groundAt(L, x, y);
        if (a && b) { cur.panX += a[0] - b[0]; cur.panZ += a[1] - b[1]; }
      } else {
        cur.yaw += dx * 0.4;
        cur.pitch = clamp(cur.pitch + dy * 0.3, 8, 89);
        vel = { yaw: (dx * 0.4) / dt, pitch: (dy * 0.3) / dt };
      }
      dragging.x = x;
      dragging.y = y;
      dragging.last = now;
      if (dragging.moved) setPreset(null);
      invalidate();
    };
    const onUp = (event) => {
      if (!dragging) return;
      const { moved, last: lastMove } = dragging;
      dragging = null;
      if (!moved) {
        vel = { yaw: 0, pitch: 0 };
        const { x, y } = local(event);
        const i = hitAt(x, y);
        const real = i !== null && boxes[i] && !boxes[i].future ? i : null;
        select(real === selected ? null : real);
      } else if (performance.now() - lastMove > 80 || reduced()) {
        vel = { yaw: 0, pitch: 0 };
      } else {
        invalidate();   // let it coast
      }
      if (opts.onDragEnd) opts.onDragEnd();
    };
    const onDouble = (event) => {
      const { x, y } = local(event);
      const i = hitAt(x, y);
      if (i === null) return;
      const b = boxes[i];
      setPreset(null);
      animateTo({ zoom: Math.max(2.2, cur.zoom), panX: b.c + 0.5 - data.cols / 2, panZ: b.r + 0.5 - 3.5 }, 600);
    };
    const onLeave = (event) => {
      pointer = null;
      if (hover !== null) { hover = null; invalidate(); }
      if (opts.onHover) opts.onHover(null, event);
    };
    // ⌘/Ctrl-scroll (and a trackpad pinch) zooms toward the pointer; a plain scroll scrolls the page, except full screen.
    const onWheel = (event) => {
      const full = opts.isFullscreen && opts.isFullscreen();
      if (!event.ctrlKey && !event.metaKey && !full) return;
      event.preventDefault();
      stopMotion();
      const { x, y } = local(event);
      const before = groundAt(lens(), x, y);
      cur.zoom = clamp(cur.zoom * Math.exp(-event.deltaY * 0.0025), 0.6, 4);
      const after = groundAt(lens(), x, y);
      if (before && after) { cur.panX += before[0] - after[0]; cur.panZ += before[1] - after[1]; }
      setPreset(null);
      invalidate();
    };

    const select = (i) => {
      selected = i;
      if (i !== null) cursor = i;
      invalidate();
      if (opts.onSelect) opts.onSelect(i);
    };
    // Bring a day into view if it has gone off the edge.
    const keepInView = (i) => {
      const b = boxes[i];
      if (!b) return;
      const [x, y] = project(lens(), (b.x0 + b.x1) / 2, b.h, (b.z0 + b.z1) / 2);
      if (x > 40 && x < width - 40 && y > 30 && y < height - 30) return;
      animateTo({ panX: b.c + 0.5 - data.cols / 2, panZ: b.r + 0.5 - 3.5 }, 450);
    };
    const moveCursor = (to) => {
      const n = data.cells.length;
      let i = clamp(to, 0, n - 1);
      while (i > 0 && data.cells[i].future) i--;
      cursor = i;
      keepInView(i);
      invalidate();
      if (opts.onCursor) opts.onCursor(i);
    };

    const onKey = (event) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;   // the page's own shortcuts
      const here = cursor ?? selected ?? (() => { let i = data.cells.length - 1; while (i > 0 && data.cells[i].future) i--; return i; })();
      const views = { 1: 'angled', 2: 'front', 3: 'top', 4: 'side', 5: 'recent' };
      let done = true;
      if (event.shiftKey && event.key.startsWith('Arrow')) {
        stopMotion();
        setPreset(null);
        if (event.key === 'ArrowLeft') cur.yaw -= 10;
        if (event.key === 'ArrowRight') cur.yaw += 10;
        if (event.key === 'ArrowUp') cur.pitch = clamp(cur.pitch + 6, 8, 89);
        if (event.key === 'ArrowDown') cur.pitch = clamp(cur.pitch - 6, 8, 89);
        invalidate();
      } else if (event.key === 'ArrowLeft') moveCursor(here - ROWS);
      else if (event.key === 'ArrowRight') moveCursor(here + ROWS);
      else if (event.key === 'ArrowUp') moveCursor(here - 1);
      else if (event.key === 'ArrowDown') moveCursor(here + 1);
      else if (event.key === 'PageUp') moveCursor(here - ROWS * 4);
      else if (event.key === 'PageDown') moveCursor(here + ROWS * 4);
      else if (event.key === 'Home') moveCursor(0);
      else if (event.key === 'End') moveCursor(data.cells.length - 1);
      else if (event.key === 'Enter') select(here === selected ? null : here);
      else if (event.key === 'Escape' && selected !== null) select(null);
      else if (views[event.key]) view(views[event.key]);
      else if (event.key === '+' || event.key === '=') { stopMotion(); cur.zoom = clamp(cur.zoom * 1.15, 0.6, 4); setPreset(null); invalidate(); }
      else if (event.key === '-') { stopMotion(); cur.zoom = clamp(cur.zoom / 1.15, 0.6, 4); setPreset(null); invalidate(); }
      else if (event.key === ' ') setSpin(!spin);
      else done = false;
      if (done) {
        event.preventDefault();
        event.stopPropagation();
      }
    };

    // ------------------------------------------------------------ size, visibility

    const sizeUp = () => {
      const box = canvas.parentElement;
      // Moved between renders, it is briefly out of the page: keep the size it had.
      if (!box || !box.isConnected || !box.clientWidth) return;
      width = Math.max(200, box.clientWidth);
      const full = opts.isFullscreen && opts.isFullscreen();
      height = full ? Math.max(300, box.clientHeight || window.innerHeight) : Math.round(Math.min(500, Math.max(300, width * 0.42)));
      ratio = window.devicePixelRatio || 1;
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
      canvas.style.setProperty('width', `${width}px`);
      canvas.style.setProperty('height', `${height}px`);
      ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
      widths.clear();
      refit();
      dirty = true;
      draw();
    };
    const resizer = new ResizeObserver(() => sizeUp());
    resizer.observe(canvas.parentElement);
    let dpr = null;
    const watchDpr = () => {
      dpr = matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
      dpr.addEventListener('change', onDpr, { once: true });
    };
    const onDpr = () => { sizeUp(); watchDpr(); };
    watchDpr();
    const seen = new IntersectionObserver(([entry]) => {
      visible = entry.isIntersecting && !document.hidden;
      if (visible) invalidate();
    });
    seen.observe(canvas);
    const onVisibility = () => { visible = !document.hidden; if (visible) invalidate(); };
    document.addEventListener('visibilitychange', onVisibility);

    canvas.addEventListener('pointerdown', onDown);
    canvas.addEventListener('pointermove', onMove);
    canvas.addEventListener('pointerup', onUp);
    canvas.addEventListener('pointercancel', onUp);
    canvas.addEventListener('pointerleave', onLeave);
    canvas.addEventListener('dblclick', onDouble);
    canvas.addEventListener('wheel', onWheel, { passive: false });
    canvas.addEventListener('keydown', onKey);

    const view = (name) => {
      if (!PRESETS[name]) return;
      stopMotion();
      setPreset(name);
      animateTo(target(name), 750);
    };

    return {
      setData,
      setPalette,
      view,
      setSpin,
      setSelected: (i) => { selected = i; invalidate(); },
      turn: (degrees) => { stopMotion(); setPreset(null); animateTo({ yaw: cur.yaw + degrees }, 400); },
      // The entrance: from the flat calendar's view, it swings round and down while the blocks rise.
      enter: (name = preset || 'angled') => {
        preset = name;
        const goal = target(name);
        if (reduced()) { Object.assign(cur, goal); invalidate(); return; }
        Object.assign(cur, { ...goal, yaw: 0, pitch: 89 });
        rising = 0;
        tween = { from: { ...cur }, to: goal, start: performance.now(), ms: 1600, grow: true };
        invalidate();
      },
      // Straight to a view, without moving there.
      jump: (name) => {
        preset = PRESETS[name] ? name : 'angled';
        animateTo(target(preset), 0);
      },
      resize: sizeUp,
      isDragging: () => dragging !== null,
      selectedIndex: () => selected,
      destroy: () => {
        if (frame) cancelAnimationFrame(frame);
        frame = 0;
        resizer.disconnect();
        seen.disconnect();
        if (dpr) dpr.removeEventListener('change', onDpr);
        document.removeEventListener('visibilitychange', onVisibility);
        canvas.removeEventListener('pointerdown', onDown);
        canvas.removeEventListener('pointermove', onMove);
        canvas.removeEventListener('pointerup', onUp);
        canvas.removeEventListener('pointercancel', onUp);
        canvas.removeEventListener('pointerleave', onLeave);
        canvas.removeEventListener('dblclick', onDouble);
        canvas.removeEventListener('wheel', onWheel);
        canvas.removeEventListener('keydown', onKey);
      },
      // For the tests: the order blocks are drawn in, and the eye, for a given camera.
      _orderFor: (camera, w = 1000, hgt = 400) => {
        Object.assign(cur, camera);
        width = w;
        height = hgt;
        const L = lens();
        const eye = eyeOf(L);
        return { eye, order: order(eye), L, project: (x, y, z) => project(L, x, y, z) };
      },
    };
  }

  // Checks the draw order against ray casting for random cameras and heights: wherever two
  // blocks overlap on screen, the one drawn later must be the one nearer along that ray.
  function selfTest(cameras = 200) {
    const cols = 14;
    const canvas = document.createElement('canvas');
    const holder = document.createElement('div');
    holder.style.setProperty('width', '1000px');
    holder.append(canvas);
    document.body.append(holder);
    const scene = create(canvas);
    let checked = 0;
    let wrong = 0;
    let seed = 7;
    const random = () => { seed = (seed * 16807) % 2147483647; return seed / 2147483647; };
    const heights = Array.from({ length: cols * ROWS }, () => (random() < 0.3 ? 0.05 : 0.2 + random() * TALLEST));
    scene.setData({ cols, cells: heights.map((h) => ({ value: h, level: 1, future: false })), colLabels: [], rowLabels: [], callouts: [], ticks: [], most: TALLEST });
    for (let n = 0; n < cameras; n++) {
      const cam = { yaw: random() * 360, pitch: 8 + random() * 81, zoom: 0.8 + random() * 2, panX: (random() - 0.5) * 6, panZ: (random() - 0.5) * 4 };
      const { eye, order } = scene._orderFor(cam);
      const rank = new Map(order.map((i, k) => [i, k]));
      // Rays from the eye through random points; each hits boxes in some order; the nearest must be drawn last.
      for (let k = 0; k < 120; k++) {
        const target = [random() * cols, random() * TALLEST * 0.6, random() * ROWS];
        const dir = target.map((v, d) => v - eye[d]);
        const hits = [];
        for (let i = 0; i < cols * ROWS; i++) {
          const c = Math.floor(i / ROWS);
          const r = i % ROWS;
          const lo = [c + GAP, 0, r + GAP];
          const hi = [c + 1 - GAP, heights[i], r + 1 - GAP];
          let t0 = -Infinity;
          let t1 = Infinity;
          for (let d = 0; d < 3; d++) {
            if (Math.abs(dir[d]) < 1e-9) { if (eye[d] < lo[d] || eye[d] > hi[d]) { t0 = Infinity; break; } continue; }
            const a = (lo[d] - eye[d]) / dir[d];
            const b = (hi[d] - eye[d]) / dir[d];
            t0 = Math.max(t0, Math.min(a, b));
            t1 = Math.min(t1, Math.max(a, b));
          }
          if (t0 <= t1 && t1 > 0) hits.push({ i, t: t0 });
        }
        if (hits.length < 2) continue;
        hits.sort((a, b) => a.t - b.t);
        checked++;
        const nearest = hits[0].i;
        if (hits.some((h) => h.i !== nearest && rank.get(h.i) > rank.get(nearest))) wrong++;
      }
    }
    scene.destroy();
    holder.remove();
    return { cameras, raysChecked: checked, wrongOrder: wrong };
  }

  window.ccSky = { create, selfTest };
})();
