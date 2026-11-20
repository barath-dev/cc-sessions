// The Paddock film's sound, synthesized here in Chrome's offline Web Audio: no
// samples and no loops, so it is the project's own. Two stems come out as 32-bit
// float WAVs, the music and the effects, for film.mjs to mix and master.
//
// The music is in A minor at the cut's tempo (timeline.js), felt in half time, two bars to a
// scene: a wide pad that opens up over the first scene, the drums and bass from
// the second, a build into the end card, a drop-out under the riser, and the
// impact on the name. The effects follow the picture's cues (timeline.js).

import { timeline } from './timeline.js';

// ?cut=long is the 25 s cut, with the intro: a drone and the pad opening up, a blip
// for each outline, the pull of the gathering and a drop as the window lands.
const CUT = new URLSearchParams(location.search).get('cut') || 'short';
const LONG = CUT === 'long';
const { BEAT, BAR, STEP, LENGTH, INTRO, SCENES, RISE, IMPACT, GROOVE, GATHER } = timeline(CUT);

const RATE = 48000;
const TAIL = 0.5;

const hz = (midi) => 440 * 2 ** ((midi - 69) / 12);

// Seeded random numbers: the same noise and the same feel every run.
function seeded(seed) {
  return () => {
    seed = (seed + 0x6d2b79f5) >>> 0;
    let t = seed;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ----------------------------------------------------------------- building blocks

// White noise; `wide` makes each ear its own, for things meant to fill the field
// (air, cymbals, risers). Drums and clicks stay in the middle.
function noise(ctx, seconds, seed = 1, wide = false) {
  const buffer = ctx.createBuffer(wide ? 2 : 1, Math.ceil(seconds * RATE), RATE);
  for (let ch = 0; ch < buffer.numberOfChannels; ch++) {
    const random = seeded(seed * 31 + ch);
    const data = buffer.getChannelData(ch);
    for (let i = 0; i < data.length; i++) data[i] = random() * 2 - 1;
  }
  return buffer;
}

// A plate reverb's impulse: decorrelated noise in each ear, dying away and darkening
// as it goes, after a short gap before the first reflections.
function plate(ctx, seconds, seed) {
  const buffer = ctx.createBuffer(2, Math.ceil(seconds * RATE), RATE);
  const gap = Math.round(0.018 * RATE);
  for (let ch = 0; ch < 2; ch++) {
    const random = seeded(seed * 17 + ch);
    const data = buffer.getChannelData(ch);
    let low = 0;
    for (let i = gap; i < data.length; i++) {
      const t = (i - gap) / RATE;
      const k = 0.9 * Math.exp(-t / (seconds * 0.28)) + 0.06;   // brightness, falling
      low += k * ((random() * 2 - 1) - low);
      data[i] = low * Math.exp(-6.9 * t / seconds);
    }
  }
  return buffer;
}

function bus(ctx, out, { reverb = null, wet = 0, delay = null, echo = 0, gain = 1 } = {}) {
  const input = ctx.createGain();
  input.gain.value = gain;
  input.connect(out);
  if (reverb && wet) { const send = ctx.createGain(); send.gain.value = wet; input.connect(send).connect(reverb); }
  if (delay && echo) { const send = ctx.createGain(); send.gain.value = echo; input.connect(send).connect(delay); }
  return input;
}

function reverbOf(ctx, out, seconds, seed, level = 1) {
  const conv = ctx.createConvolver();
  conv.normalize = true;
  conv.buffer = plate(ctx, seconds, seed);
  const hp = ctx.createBiquadFilter();
  hp.type = 'highpass';
  hp.frequency.value = 180;
  const g = ctx.createGain();
  g.gain.value = level;
  conv.connect(hp).connect(g).connect(out);
  return conv;
}

// A ping-pong delay: each repeat crosses to the other ear and loses a little top.
function pingPong(ctx, out, time, feedback = 0.38) {
  const input = ctx.createGain();
  const left = ctx.createDelay(2);
  const right = ctx.createDelay(2);
  left.delayTime.value = right.delayTime.value = time;
  const fbL = ctx.createGain();
  const fbR = ctx.createGain();
  fbL.gain.value = fbR.gain.value = feedback;
  const tone = ctx.createBiquadFilter();
  tone.type = 'lowpass';
  tone.frequency.value = 4200;
  const merge = ctx.createChannelMerger(2);
  input.connect(tone).connect(left);
  left.connect(fbL).connect(right);
  right.connect(fbR).connect(left);
  left.connect(merge, 0, 0);
  right.connect(merge, 0, 1);
  merge.connect(out);
  return input;
}

function shaper(ctx, drive) {
  const s = ctx.createWaveShaper();
  const curve = new Float32Array(2048);
  for (let i = 0; i < curve.length; i++) {
    const x = (i / (curve.length - 1)) * 2 - 1;
    curve[i] = Math.tanh(drive * x) / Math.tanh(drive);
  }
  s.curve = curve;
  s.oversample = '4x';
  return s;
}

// An envelope on a gain: up to `peak` in `attack`, then down exponentially over `decay`.
function hit(param, at, peak, attack, decay) {
  param.setValueAtTime(0, at);
  param.linearRampToValueAtTime(peak, at + attack);
  param.setTargetAtTime(0, at + attack, decay / 4);
}

function play(ctx, node, at, until) {
  node.start(at);
  node.stop(until);
}

// ------------------------------------------------------------------- the music

// A chord to each bar: Am9, Fmaj9, Cadd9, G6, twice; then the impact on Am9 and
// out on Fmaj9. [root, voices].
const CHORDS = [
  [33, [57, 60, 64, 67, 71]],
  [29, [53, 57, 60, 64, 67]],
  [36, [55, 60, 62, 64, 67]],
  [31, [55, 59, 62, 64, 69]],
];
// `bar` counts from the start; the intro's three bars (Am, F, G) lead into the first scene's Am.
const chordAt = (bar) => {
  const b = bar - Math.round(INTRO / BAR);
  if (b < 0) return [CHORDS[0], CHORDS[1], CHORDS[3]][b + 3];
  return b >= 9 ? CHORDS[1] : CHORDS[b % 4];
};
const BARS = Math.ceil(LENGTH / BAR);

// Where the kick falls in a bar of sixteenths: half time, with a push before the backbeat.
const KICKS = [0, 3, 10];
const kickTimes = [];
for (let t = GROOVE.from; t < GROOVE.until - 1e-6; t += BAR) for (const s of KICKS) kickTimes.push(t + s * STEP);

// The pump: a level that dips on every kick and comes back, for the pad and bass.
function pump(param, level, depth) {
  param.setValueAtTime(level, 0);
  for (const k of kickTimes) {
    param.setValueAtTime(level, k - 0.005);
    param.linearRampToValueAtTime(level * (1 - depth), k + 0.01);
    param.setTargetAtTime(level, k + 0.03, 0.06);
  }
}

function compose(ctx, out) {
  const glue = ctx.createDynamicsCompressor();
  glue.threshold.value = -16;
  glue.ratio.value = 2;
  glue.attack.value = 0.02;
  glue.release.value = 0.25;
  glue.knee.value = 8;
  glue.connect(out);
  const room = reverbOf(ctx, glue, 2.4, 3, 0.9);
  const hall = reverbOf(ctx, glue, 4.2, 5, 1);
  const echoes = pingPong(ctx, glue, BEAT * 0.75, 0.36);

  // The whole mix fades over the last second, so the film ends on air, not a cut.
  const ending = ctx.createGain();
  ending.gain.setValueAtTime(1, LENGTH - 1.1);
  ending.gain.linearRampToValueAtTime(0, LENGTH - 0.02);
  ending.connect(glue);

  pad(ctx, bus(ctx, ending, { reverb: hall, wet: 0.5 }));
  arp(ctx, bus(ctx, ending, { reverb: room, wet: 0.25, delay: echoes, echo: 0.3 }));
  if (LONG) drone(ctx, bus(ctx, ending, { reverb: hall, wet: 0.4 })); else sub(ctx, ending);
  bass(ctx, ending);
  drums(ctx, ending, room);
  build(ctx, ending, room);
  stab(ctx, bus(ctx, ending, { reverb: hall, wet: 0.7 }));
}

// Seven detuned saws to a voice, spread across the stereo field, through one filter
// that opens over the first scene, breathes with the groove and opens fully on the impact.
function pad(ctx, out) {
  const filter = ctx.createBiquadFilter();
  filter.type = 'lowpass';
  filter.Q.value = 0.7;
  const f = filter.frequency;
  if (LONG) {
    // Nearly shut over the scattered outlines, opening as they gather, then on with the app.
    f.setValueAtTime(220, 0);
    f.exponentialRampToValueAtTime(700, GATHER);
    f.exponentialRampToValueAtTime(3200, INTRO - 0.02);
    f.setValueAtTime(1500, INTRO);
  } else {
    f.setValueAtTime(320, 0);
  }
  f.exponentialRampToValueAtTime(2300, SCENES.all);
  f.linearRampToValueAtTime(2600, SCENES.usage);
  f.exponentialRampToValueAtTime(4800, RISE);
  f.setValueAtTime(900, RISE);
  f.setValueAtTime(7000, IMPACT);
  f.exponentialRampToValueAtTime(2200, LENGTH);
  const level = ctx.createGain();
  // Fuller alone at the start, back in the mix under the drums, out under the riser.
  pump(level.gain, 0.16, 0.45);
  level.gain.setValueAtTime(LONG ? 0.15 : 0.2, 0);
  level.gain.setValueAtTime(0.16, LONG ? INTRO : SCENES.all);
  level.gain.setValueAtTime(0.16, RISE);
  level.gain.linearRampToValueAtTime(0.05, RISE + 0.15);
  level.gain.setValueAtTime(0.05, IMPACT - 0.01);
  level.gain.linearRampToValueAtTime(0.22, IMPACT + 0.02);
  filter.connect(level).connect(out);

  const detune = [-14, -9, -4, 0, 4, 9, 14];
  for (let b = 0; b < BARS; b++) {
    const at = b * BAR;
    const until = Math.min(LENGTH + TAIL, at + BAR + 0.5);
    const [, voices] = chordAt(b);
    for (const note of voices) {
      detune.forEach((cents, i) => {
        const osc = ctx.createOscillator();
        osc.type = 'sawtooth';
        osc.frequency.value = hz(note);
        osc.detune.value = cents;
        const pan = ctx.createStereoPanner();
        pan.pan.value = (i / (detune.length - 1)) * 1.1 - 0.55;
        const g = ctx.createGain();
        g.gain.setValueAtTime(0, at);
        g.gain.linearRampToValueAtTime(0.05, at + (b === 0 ? 0.25 : 0.12));
        g.gain.setValueAtTime(0.05, at + BAR);
        g.gain.linearRampToValueAtTime(0, until);
        osc.connect(g).connect(pan).connect(filter);
        play(ctx, osc, at, until);
      });
    }
  }
}

// Sixteenths up the chord, plucked, bouncing between the ears.
function arp(ctx, out) {
  const filter = ctx.createBiquadFilter();
  filter.type = 'lowpass';
  filter.frequency.value = 3400;
  filter.connect(out);
  const random = seeded(11);
  for (let t = LONG ? BAR : BEAT * 2, n = 0; t < RISE - 1e-6; t += STEP, n++) {
    const [, voices] = chordAt(Math.floor(t / BAR));
    const ladder = [...voices.slice(1), ...voices.slice(1).map((v) => v + 12)];
    const note = ladder[n % ladder.length] + 12;
    const osc = ctx.createOscillator();
    osc.type = 'triangle';
    osc.frequency.value = hz(note);
    const g = ctx.createGain();
    const lift = t < INTRO ? 0.45 : t < SCENES.all ? 0.6 : t > SCENES.usage ? 1.15 : 1;
    hit(g.gain, t, (0.028 + random() * 0.012) * lift, 0.003, 0.16);
    osc.connect(g).connect(filter);
    play(ctx, osc, t, t + 0.35);
  }
}

// A low pulse in eighths under the first scene, before the bass comes in.
function sub(ctx, out) {
  for (let t = 0; t < SCENES.all - 1e-6; t += BEAT / 2) {
    const osc = ctx.createOscillator();
    osc.frequency.value = hz(chordAt(Math.floor(t / BAR))[0] + 12);
    const g = ctx.createGain();
    hit(g.gain, t, 0.12 + 0.08 * (t / SCENES.all), 0.01, 0.22);
    osc.connect(g).connect(out);
    play(ctx, osc, t, t + 0.3);
  }
}

// Under the intro: a low A held and slowly breathing, with a band of dark noise over it,
// swelling as the outlines gather and stopping on the drop.
function drone(ctx, out) {
  const level = ctx.createGain();
  level.gain.setValueAtTime(0, 0);
  level.gain.linearRampToValueAtTime(0.5, 0.8);
  level.gain.linearRampToValueAtTime(0.65, GATHER);
  level.gain.exponentialRampToValueAtTime(1, INTRO - 0.03);
  level.gain.linearRampToValueAtTime(0, INTRO);
  level.connect(out);
  for (const [note, detune, gain] of [[33, -4, 0.5], [33, 4, 0.5], [45, 0, 0.18]]) {
    const osc = ctx.createOscillator();
    osc.frequency.value = hz(note);
    osc.detune.value = detune;
    const g = ctx.createGain();
    g.gain.value = gain;
    osc.connect(g).connect(level);
    play(ctx, osc, 0, INTRO);
  }
  const air = ctx.createBufferSource();
  air.buffer = noise(ctx, INTRO, 83, true);
  const band = ctx.createBiquadFilter();
  band.type = 'bandpass';
  band.Q.value = 0.8;
  band.frequency.setValueAtTime(260, 0);
  band.frequency.exponentialRampToValueAtTime(1400, INTRO);
  const g = ctx.createGain();
  g.gain.value = 0.12;
  air.connect(band).connect(g).connect(level);
  play(ctx, air, 0, INTRO);
}

// The bass: a saw and a sine an octave down, through a filter that snaps open on each note.
function bass(ctx, out) {
  const level = ctx.createGain();
  pump(level.gain, 0.32, 0.7);
  const warm = shaper(ctx, 1.6);
  warm.connect(level).connect(out);
  const STEPS = [0, 3, 6, 8, 11, 14];
  for (let bar = GROOVE.from; bar < GROOVE.until - 1e-6; bar += BAR) {
    const root = chordAt(Math.round(bar / BAR))[0] + 12;
    for (const [i, s] of STEPS.entries()) {
      const at = bar + s * STEP;
      const note = i === 4 ? root + 12 : root;
      const saw = ctx.createOscillator();
      saw.type = 'sawtooth';
      saw.frequency.value = hz(note);
      const sine = ctx.createOscillator();
      sine.frequency.value = hz(note - 12);
      const filter = ctx.createBiquadFilter();
      filter.type = 'lowpass';
      filter.Q.value = 4;
      filter.frequency.setValueAtTime(1800, at);
      filter.frequency.exponentialRampToValueAtTime(260, at + 0.16);
      const g = ctx.createGain();
      hit(g.gain, at, 0.55, 0.004, 0.22);
      const gs = ctx.createGain();
      hit(gs.gain, at, 0.6, 0.004, 0.26);
      saw.connect(filter).connect(g).connect(warm);
      sine.connect(gs).connect(warm);
      play(ctx, saw, at, at + 0.4);
      play(ctx, sine, at, at + 0.4);
    }
  }
}

function kick(ctx, out, at, size = 1, buffer) {
  const osc = ctx.createOscillator();
  osc.frequency.setValueAtTime(165 * size, at);
  osc.frequency.exponentialRampToValueAtTime(46, at + 0.085);
  osc.frequency.exponentialRampToValueAtTime(38, at + 0.4);
  const g = ctx.createGain();
  hit(g.gain, at, 0.95 * size, 0.002, 0.42 * size);
  const drive = shaper(ctx, 2.2);
  osc.connect(g).connect(drive).connect(out);
  play(ctx, osc, at, at + 0.7);
  // The beater: a few milliseconds of bright noise.
  const click = ctx.createBufferSource();
  click.buffer = buffer;
  const hp = ctx.createBiquadFilter();
  hp.type = 'highpass';
  hp.frequency.value = 1400;
  const gc = ctx.createGain();
  hit(gc.gain, at, 0.35 * size, 0.0005, 0.012);
  click.connect(hp).connect(gc).connect(out);
  play(ctx, click, at, at + 0.05);
}

function snare(ctx, out, at, level, buffer, room) {
  // A clap: three quick bursts and a tail, over a snare's crack and body.
  const body = ctx.createOscillator();
  body.type = 'triangle';
  body.frequency.setValueAtTime(210, at);
  body.frequency.exponentialRampToValueAtTime(170, at + 0.08);
  const gb = ctx.createGain();
  hit(gb.gain, at, 0.32 * level, 0.001, 0.09);
  body.connect(gb).connect(out);
  play(ctx, body, at, at + 0.2);
  const parts = [[0, 0.5, 0.008], [0.011, 0.45, 0.008], [0.022, 0.55, 0.16]];
  for (const [dt, peak, decay] of parts) {
    const src = ctx.createBufferSource();
    src.buffer = buffer;
    src.playbackRate.value = 1 + dt * 4;
    const bp = ctx.createBiquadFilter();
    bp.type = 'bandpass';
    bp.frequency.value = 1500;
    bp.Q.value = 0.9;
    const g = ctx.createGain();
    hit(g.gain, at + dt, peak * level, 0.0008, decay);
    src.connect(bp).connect(g);
    g.connect(out);
    const send = ctx.createGain();
    send.gain.value = 0.35;
    g.connect(send).connect(room);
    play(ctx, src, at + dt, at + dt + 0.35);
  }
}

function hat(ctx, out, at, level, open, buffer) {
  const src = ctx.createBufferSource();
  src.buffer = buffer;
  const hp = ctx.createBiquadFilter();
  hp.type = 'highpass';
  hp.frequency.value = 7200;
  const peak = ctx.createBiquadFilter();
  peak.type = 'peaking';
  peak.frequency.value = 10500;
  peak.gain.value = 6;
  const g = ctx.createGain();
  hit(g.gain, at, level, 0.0008, open ? 0.24 : 0.04);
  src.connect(hp).connect(peak).connect(g).connect(out);
  play(ctx, src, at, at + (open ? 0.5 : 0.12));
}

function drums(ctx, out, room) {
  const kit = ctx.createGain();
  kit.gain.value = 0.9;
  kit.connect(out);
  const hats = ctx.createStereoPanner();
  hats.pan.value = 0.22;
  hats.connect(kit);
  const buffer = noise(ctx, 1, 7);
  const random = seeded(23);
  // After the intro the first beat is the drop's (build), not the groove's.
  for (const k of kickTimes) if (!(LONG && k === GROOVE.from)) kick(ctx, kit, k, 1, buffer);
  for (let bar = GROOVE.from; bar < GROOVE.until - 1e-6; bar += BAR) {
    // The backbeat on the third beat; the last bar before the build is left to the roll.
    if (bar < SCENES.usage + BAR - 1e-6) snare(ctx, kit, bar + 8 * STEP, 1, buffer, room);
    for (let s = 0; s < 16; s++) {
      const busy = bar >= SCENES.search;
      if (s % 2 && !busy) continue;
      const open = s === 14;
      const accent = s % 4 === 2 ? 1 : s % 2 ? 0.45 : 0.7;
      hat(ctx, hats, bar + s * STEP + (s % 2 ? 0.006 : 0), (0.1 + random() * 0.05) * accent * (open ? 0.8 : 1), open, buffer);
    }
  }
}

// Into the end card: a snare roll that speeds up and climbs, then everything drops
// out for the riser (the effects stem's), and the impact lands on the name.
function build(ctx, out, room) {
  const buffer = noise(ctx, 1, 13);
  const from = SCENES.usage + BAR;
  const rolls = [];
  for (let t = from; t < from + BEAT * 2 - 1e-6; t += BEAT / 2) rolls.push(t);
  for (let t = from + BEAT * 2; t < from + BEAT * 3 - 1e-6; t += STEP) rolls.push(t);
  for (let t = from + BEAT * 3; t < RISE - 1e-6; t += STEP / 2) rolls.push(t);
  rolls.forEach((t, i) => snare(ctx, out, t, 0.35 + 0.65 * (i / rolls.length), buffer, room));
  // The impact's drums: a big low kick, and a crash that rings out; a smaller pair on the drop.
  const crash = (at, size) => {
    kick(ctx, out, at, size, buffer);
    const src = ctx.createBufferSource();
    src.buffer = noise(ctx, 3, 29, true);
    const hp = ctx.createBiquadFilter();
    hp.type = 'highpass';
    hp.frequency.value = 4200;
    const g = ctx.createGain();
    hit(g.gain, at, 0.22 * (size - 0.25), 0.002, 2.2 * (size - 0.25));
    src.connect(hp).connect(g).connect(out);
    const send = ctx.createGain();
    send.gain.value = 0.4;
    g.connect(send).connect(room);
    play(ctx, src, at, at + 3);
  };
  crash(IMPACT, 1.25);
  if (LONG) crash(INTRO, 1.1);
}

// The chord on the impact: all the saws at once, the filter wide open and closing.
function stab(ctx, out) {
  const [root, voices] = chordAt(Math.round(IMPACT / BAR));
  const filter = ctx.createBiquadFilter();
  filter.type = 'lowpass';
  filter.frequency.setValueAtTime(9000, IMPACT);
  filter.frequency.exponentialRampToValueAtTime(1400, IMPACT + 1.2);
  filter.connect(out);
  for (const note of [root + 24, ...voices, voices[2] + 12]) {
    for (const cents of [-12, 0, 12]) {
      const osc = ctx.createOscillator();
      osc.type = 'sawtooth';
      osc.frequency.value = hz(note);
      osc.detune.value = cents;
      const g = ctx.createGain();
      hit(g.gain, IMPACT, 0.035, 0.004, 1.4);
      osc.connect(g).connect(filter);
      play(ctx, osc, IMPACT, IMPACT + 2.5);
    }
  }
}

// ----------------------------------------------------------------- the effects

function effects(ctx, out, cues) {
  const room = reverbOf(ctx, out, 1.8, 41, 0.9);
  const hall = reverbOf(ctx, out, 3.8, 43, 1);
  const echoes = pingPong(ctx, out, BEAT * 0.75, 0.42);
  const buffer = noise(ctx, 1, 51);
  const air = noise(ctx, 4, 53, true);
  const random = seeded(61);
  let ticks = 0;
  for (const cue of cues) {
    const at = cue.at;
    if (cue.kind === 'whoosh') whoosh(ctx, out, room, air, at, cue.pan, cue.size);
    else if (cue.kind === 'key') key(ctx, out, at, 1 + (random() - 0.5) * 0.12, buffer);
    else if (cue.kind === 'click') { key(ctx, out, at, 0.62, buffer, 1.3); key(ctx, out, at + 0.075, 0.7, buffer, 0.8); }
    else if (cue.kind === 'pop') blip(ctx, bus(ctx, out, { reverb: room, wet: 0.2 }), at, 620, 1120, 0.16);
    else if (cue.kind === 'tick') { blip(ctx, bus(ctx, out, { reverb: room, wet: 0.25 }), at, hz(76 + [0, 3, 7, 10, 12][ticks % 5]), null, 0.08); ticks++; }
    else if (cue.kind === 'chime') bell(ctx, bus(ctx, out, { reverb: hall, wet: 0.45, delay: echoes, echo: 0.25 }), at, hz(88), 0.09);
    else if (cue.kind === 'riser') riser(ctx, out, hall, at, cue.until);
    else if (cue.kind === 'impact') impact(ctx, out, hall, at);
    else if (cue.kind === 'drop') impact(ctx, out, hall, at, 0.65);
    else if (cue.kind === 'gather') gather(ctx, out, hall, air, at, cue.until);
    else if (cue.kind === 'blip') data(ctx, bus(ctx, out, { reverb: room, wet: 0.3, delay: echoes, echo: 0.18 }), at, cue.pan, cue.depth, random);
    else if (cue.kind === 'shimmer') [81, 84, 88, 93, 95, 100].forEach((n, i) => bell(ctx, bus(ctx, out, { reverb: hall, wet: 0.6, delay: echoes, echo: 0.3 }), at + i * 0.045, hz(n), 0.05));
  }
}

// Air rushing past as the camera moves: noise through a band that sweeps up into the
// cut and falls away after it, travelling across the stereo field the way the move goes.
function whoosh(ctx, out, room, buffer, at, pan, size) {
  const src = ctx.createBufferSource();
  src.buffer = buffer;
  const bp = ctx.createBiquadFilter();
  bp.type = 'bandpass';
  bp.Q.value = 1.1;
  bp.frequency.setValueAtTime(260, at - 0.45);
  bp.frequency.exponentialRampToValueAtTime(3800, at);
  bp.frequency.exponentialRampToValueAtTime(520, at + 0.4);
  const g = ctx.createGain();
  g.gain.setValueAtTime(0.0001, at - 0.45);
  g.gain.exponentialRampToValueAtTime(0.55 * size, at);
  g.gain.exponentialRampToValueAtTime(0.0001, at + 0.42);
  const p = ctx.createStereoPanner();
  p.pan.setValueAtTime(-pan * 0.8, at - 0.45);
  p.pan.linearRampToValueAtTime(pan * 0.8, at + 0.4);
  src.connect(bp).connect(g).connect(p).connect(out);
  const send = ctx.createGain();
  send.gain.value = 0.25;
  p.connect(send).connect(room);
  play(ctx, src, at - 0.45, at + 0.5);
  if (size >= 0.8) {
    // Under the bigger moves, a soft low thump on the cut itself.
    const thump = ctx.createOscillator();
    thump.frequency.setValueAtTime(90, at);
    thump.frequency.exponentialRampToValueAtTime(45, at + 0.18);
    const gt = ctx.createGain();
    hit(gt.gain, at, 0.4 * size, 0.004, 0.22);
    thump.connect(gt).connect(out);
    play(ctx, thump, at, at + 0.4);
  }
}

// A key or button: a tiny bright tick with a little body under it.
function key(ctx, out, at, pitch, buffer, level = 1) {
  const osc = ctx.createOscillator();
  osc.frequency.setValueAtTime(2900 * pitch, at);
  osc.frequency.exponentialRampToValueAtTime(1700 * pitch, at + 0.012);
  const g = ctx.createGain();
  hit(g.gain, at, 0.09 * level, 0.0006, 0.018);
  osc.connect(g).connect(out);
  play(ctx, osc, at, at + 0.06);
  const src = ctx.createBufferSource();
  src.buffer = buffer;
  const hp = ctx.createBiquadFilter();
  hp.type = 'highpass';
  hp.frequency.value = 4500;
  const gn = ctx.createGain();
  hit(gn.gain, at, 0.12 * level, 0.0004, 0.006);
  src.connect(hp).connect(gn).connect(out);
  play(ctx, src, at, at + 0.03);
}

// A soft rounded blip, gliding up when `to` is given.
function blip(ctx, out, at, from, to, level) {
  const osc = ctx.createOscillator();
  osc.frequency.setValueAtTime(from, at);
  if (to) osc.frequency.exponentialRampToValueAtTime(to, at + 0.07);
  const g = ctx.createGain();
  hit(g.gain, at, level, 0.004, 0.14);
  osc.connect(g).connect(out);
  play(ctx, osc, at, at + 0.3);
}

// A bell: a few inharmonic partials, the higher ones dying first.
function bell(ctx, out, at, f, level) {
  for (const [ratio, part, decay] of [[1, 1, 1.4], [2.76, 0.45, 0.6], [5.4, 0.25, 0.3], [8.93, 0.12, 0.15]]) {
    const osc = ctx.createOscillator();
    osc.frequency.value = f * ratio;
    const g = ctx.createGain();
    hit(g.gain, at, level * part, 0.002, decay);
    osc.connect(g).connect(out);
    play(ctx, osc, at, at + decay * 1.6 + 0.1);
  }
}

// The rise into the impact: noise sweeping up, a saw climbing two octaves, and a
// reversed-cymbal swell in the last moments, all stopping dead on the impact.
function riser(ctx, out, hall, from, until) {
  const length = until - from;
  const src = ctx.createBufferSource();
  src.buffer = noise(ctx, length + 0.1, 71, true);
  const bp = ctx.createBiquadFilter();
  bp.type = 'bandpass';
  bp.Q.value = 1.4;
  bp.frequency.setValueAtTime(380, from);
  bp.frequency.exponentialRampToValueAtTime(9000, until);
  const g = ctx.createGain();
  g.gain.setValueAtTime(0.0001, from);
  g.gain.exponentialRampToValueAtTime(0.42, until - 0.01);
  g.gain.linearRampToValueAtTime(0, until);
  src.connect(bp).connect(g).connect(out);
  const send = ctx.createGain();
  send.gain.value = 0.35;
  g.connect(send).connect(hall);
  play(ctx, src, from, until);

  const saw = ctx.createOscillator();
  saw.type = 'sawtooth';
  saw.frequency.setValueAtTime(hz(45), from);
  saw.frequency.exponentialRampToValueAtTime(hz(69), until);
  const lp = ctx.createBiquadFilter();
  lp.type = 'lowpass';
  lp.frequency.setValueAtTime(400, from);
  lp.frequency.exponentialRampToValueAtTime(5000, until);
  const gs = ctx.createGain();
  gs.gain.setValueAtTime(0.0001, from);
  gs.gain.exponentialRampToValueAtTime(0.08, until - 0.01);
  gs.gain.linearRampToValueAtTime(0, until);
  saw.connect(lp).connect(gs).connect(out);
  play(ctx, saw, from, until);

  const swell = ctx.createBufferSource();
  swell.buffer = noise(ctx, 0.6, 73, true);
  const hp = ctx.createBiquadFilter();
  hp.type = 'highpass';
  hp.frequency.value = 5200;
  const gw = ctx.createGain();
  gw.gain.setValueAtTime(0.0001, until - 0.5);
  gw.gain.exponentialRampToValueAtTime(0.3, until - 0.01);
  gw.gain.linearRampToValueAtTime(0, until);
  swell.connect(hp).connect(gw).connect(out);
  play(ctx, swell, until - 0.5, until);
}

// The impact: a sub drop you feel more than hear, a burst of noise into a long
// plate, and a low boom under both.
function impact(ctx, out, hall, at, size = 1) {
  out = bus(ctx, out, { gain: size });
  const sub = ctx.createOscillator();
  sub.frequency.setValueAtTime(62, at);
  sub.frequency.exponentialRampToValueAtTime(29, at + 1.4);
  const gs = ctx.createGain();
  hit(gs.gain, at, 0.9, 0.003, 2.0);
  const warm = shaper(ctx, 1.4);
  sub.connect(gs).connect(warm).connect(out);
  play(ctx, sub, at, at + 2.6);

  const burst = ctx.createBufferSource();
  burst.buffer = noise(ctx, 1.2, 79, true);
  const lp = ctx.createBiquadFilter();
  lp.type = 'lowpass';
  lp.frequency.setValueAtTime(5200, at);
  lp.frequency.exponentialRampToValueAtTime(300, at + 0.8);
  const gb = ctx.createGain();
  hit(gb.gain, at, 0.55, 0.002, 0.7);
  burst.connect(lp).connect(gb).connect(out);
  const send = ctx.createGain();
  send.gain.value = 0.9;
  gb.connect(send).connect(hall);
  play(ctx, burst, at, at + 1.2);

  const boom = ctx.createOscillator();
  boom.type = 'triangle';
  boom.frequency.setValueAtTime(110, at);
  boom.frequency.exponentialRampToValueAtTime(52, at + 0.3);
  const gm = ctx.createGain();
  hit(gm.gain, at, 0.45, 0.002, 0.5);
  boom.connect(gm).connect(out);
  play(ctx, boom, at, at + 0.8);
}

// An outline appearing: a short digital blip where it is, higher the further away.
function data(ctx, out, at, pan, depth, random) {
  const f = [740, 1110, 1480][depth] * (1 + (random() - 0.5) * 0.04);
  const osc = ctx.createOscillator();
  osc.type = 'triangle';
  osc.frequency.setValueAtTime(f * 1.5, at);
  osc.frequency.exponentialRampToValueAtTime(f, at + 0.02);
  const g = ctx.createGain();
  hit(g.gain, at, [0.075, 0.06, 0.045][depth], 0.002, 0.07);
  const p = ctx.createStereoPanner();
  p.pan.value = pan * 0.75;
  osc.connect(g).connect(p).connect(out);
  play(ctx, osc, at, at + 0.15);
}

// The gathering: air drawn in, narrowing and rising, and a tone sweeping up under it,
// stopping dead on the drop.
function gather(ctx, out, hall, air, from, until) {
  const src = ctx.createBufferSource();
  src.buffer = air;
  const lp = ctx.createBiquadFilter();
  lp.type = 'lowpass';
  lp.Q.value = 2;
  lp.frequency.setValueAtTime(300, from);
  lp.frequency.exponentialRampToValueAtTime(7000, until);
  const g = ctx.createGain();
  g.gain.setValueAtTime(0.0001, from);
  g.gain.exponentialRampToValueAtTime(0.38, until - 0.01);
  g.gain.linearRampToValueAtTime(0, until);
  src.connect(lp).connect(g).connect(out);
  const send = ctx.createGain();
  send.gain.value = 0.3;
  g.connect(send).connect(hall);
  play(ctx, src, from, until);
  const tone = ctx.createOscillator();
  tone.frequency.setValueAtTime(180, from);
  tone.frequency.exponentialRampToValueAtTime(1300, until);
  const gt = ctx.createGain();
  gt.gain.setValueAtTime(0.0001, from);
  gt.gain.exponentialRampToValueAtTime(0.09, until - 0.01);
  gt.gain.linearRampToValueAtTime(0, until);
  tone.connect(gt).connect(out);
  play(ctx, tone, from, until);
}

// --------------------------------------------------------------------- output

async function render(build) {
  const ctx = new OfflineAudioContext(2, Math.ceil((LENGTH + TAIL) * RATE), RATE);
  const out = ctx.createGain();
  out.connect(ctx.destination);
  build(ctx, out);
  return ctx.startRendering();
}

// A 32-bit float WAV, so nothing is clipped before the master, as base64.
function wav(buffer) {
  const frames = buffer.length;
  const bytes = new DataView(new ArrayBuffer(44 + frames * 8));
  const text = (at, s) => [...s].forEach((c, i) => bytes.setUint8(at + i, c.charCodeAt(0)));
  text(0, 'RIFF'); bytes.setUint32(4, 36 + frames * 8, true); text(8, 'WAVE');
  text(12, 'fmt '); bytes.setUint32(16, 16, true); bytes.setUint16(20, 3, true); bytes.setUint16(22, 2, true);
  bytes.setUint32(24, RATE, true); bytes.setUint32(28, RATE * 8, true); bytes.setUint16(32, 8, true); bytes.setUint16(34, 32, true);
  text(36, 'data'); bytes.setUint32(40, frames * 8, true);
  const [left, right] = [buffer.getChannelData(0), buffer.getChannelData(1)];
  for (let i = 0; i < frames; i++) {
    bytes.setFloat32(44 + i * 8, left[i], true);
    bytes.setFloat32(48 + i * 8, right[i], true);
  }
  const all = new Uint8Array(bytes.buffer);
  let out = '';
  for (let i = 0; i < all.length; i += 0x8000) out += String.fromCharCode(...all.subarray(i, i + 0x8000));
  return btoa(out);
}

window.renderStems = async (cues) => ({
  music: wav(await render((ctx, out) => compose(ctx, out))),
  fx: wav(await render((ctx, out) => effects(ctx, out, cues))),
});
window.ready = true;
