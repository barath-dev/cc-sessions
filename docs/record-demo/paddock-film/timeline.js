// The Paddock film's timing, shared by the picture (stage.js) and the sound
// (score.js): every cut, move and click is a time here, so the music and the
// effects land on the frame they belong to. Times are seconds from the start.
//
// The music is felt in half time and each scene is two bars, so every cut falls on
// a downbeat. Everything in a cut is counted in its beats, so its tempo sets its pace.
//
// There are two cuts. `short` (15 s, 160 beats a minute) opens on the app. `long`
// (25 s, 128 beats a minute: the same film, given more time to read) opens on an
// abstract intro, three bars long: the sessions the author had open, scattered in the
// dark, gathered into one window as the beat drops; the app scenes follow, and the
// end card is held longer.

// Seeded random numbers, so the outlines fall the same way every run.
export function seeded(seed) {
  return () => {
    seed = (seed + 0x6d2b79f5) >>> 0;
    let t = seed;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// The intro's terminal outlines: the sessions the author had open the day the film
// was made, 23 of them, as Paddock counted them (15 done but not closed, 3 waiting
// for an answer, 2 working, 3 idle). Each has when it appears (on a sixteenth from
// `from`; the first few are already there in the first frame, which is what a feed
// shows before anyone presses play), where (as fractions of the frame), how deep
// (0 near, 2 far) and its state. Placed on a loose grid with jitter, so they fill
// the frame without piling up.
export const OPEN = { total: 23, done: 15, waiting: 3, working: 2, idle: 3 };

function outlines(step, from) {
  const random = seeded(97);
  const cells = [];
  for (let row = 0; row < 6; row++) for (let col = 0; col < 5; col++) cells.push([col, row]);
  const shuffle = (list) => {
    for (let i = list.length - 1; i > 0; i--) {
      const j = Math.floor(random() * (i + 1));
      [list[i], list[j]] = [list[j], list[i]];
    }
    return list;
  };
  shuffle(cells);
  const states = shuffle([...Array(OPEN.done).fill('done'), ...Array(OPEN.waiting).fill('waiting'),
    ...Array(OPEN.working).fill('working'), ...Array(OPEN.idle).fill('idle')]);
  const early = 6;
  return cells.slice(0, OPEN.total).map(([col, row], n) => ({
    at: n < early ? -1 : from + (n - early) * step,
    x: (col + 0.2 + random() * 0.6) / 5,
    y: (row + 0.2 + random() * 0.6) / 6,
    depth: random() < 0.35 ? 0 : random() < 0.6 ? 1 : 2,
    state: states[n],
    size: 0.8 + random() * 0.45,
    lines: 2 + Math.floor(random() * 2),
    cursor: random() < 0.35,
    seed: Math.floor(random() * 1e9),
  }));
}

export function timeline(cut = 'short') {
  const long = cut === 'long';
  const BPM = long ? 128 : 160;
  const BEAT = 60 / BPM;
  const BAR = 4 * BEAT;
  const STEP = BEAT / 4;
  const INTRO = long ? 3 * BAR : 0;
  const SCENES = { both: INTRO, all: INTRO + 2 * BAR, search: INTRO + 4 * BAR, usage: INTRO + 6 * BAR, end: INTRO + 8 * BAR };
  const LENGTH = long ? 25 : 15;
  // The music drops out and rises into the end card; the impact is the name.
  const RISE = SCENES.end - BAR * 2 / 3;
  const IMPACT = SCENES.end;
  // The drums come in with the second scene, or with the app itself after the intro.
  const GROOVE = { from: long ? SCENES.both : SCENES.all, until: RISE };
  // In the intro: scattered until GATHER, gathered into the window's grid by the drop.
  const GATHER = INTRO - BAR;
  const OUTLINES = long ? outlines(STEP, 0.05) : [];
  // The intro's three lines: open (already up in the first frame), done, waiting.
  // The outlines answer each: the done ones go green on the second, the waiting ones
  // start to pulse on the third.
  const SAID = long ? [-0.3, 2 * BEAT, 4 * BEAT] : [];

  // The search, typed a letter to each eighth note from the second beat of its scene.
  const WORD = 'cart';
  const typedAt = (n) => SCENES.search + BEAT + n * (BEAT / 2);
  // The pointer picks the Codex conversation on the third beat, then points at Resume.
  const PICK = SCENES.search + 4 * BEAT;
  const RESUME = SCENES.search + 6 * BEAT;
  // The labels on the first scene's two sessions, and the ring on the Codex account.
  const LABELS = [SCENES.both + 2 * BEAT, SCENES.both + 2.5 * BEAT];
  const RING = SCENES.all + 2 * BEAT;
  // The bars of Usage grow in one after another, an eighth note apart.
  const barAt = (n) => SCENES.usage + BEAT + n * (BEAT / 2);

  // What the sound follows: whooshes on camera moves (`pan` is where the move goes,
  // -1 left to 1 right), ticks on keys and clicks, pops on labels, and in the intro
  // a blip for each outline (one per sixteenth), the gathering and the drop.
  function cues(bars = 5) {
    const blips = [];
    for (const o of OUTLINES) if (o.at >= 0 && !blips.some((b) => Math.abs(b.at - o.at) < 1e-6)) blips.push({ kind: 'blip', at: o.at, pan: o.x * 2 - 1, depth: o.depth });
    return [
      ...blips,
      ...(long ? [{ kind: 'chime', at: SAID[1] + 0.05 }, ...[0, 1, 2].map((n) => ({ kind: 'pop', at: SAID[2] + 0.05 + n * STEP })),
        { kind: 'gather', at: GATHER, until: INTRO }, { kind: 'drop', at: INTRO }] : []),
      { kind: 'whoosh', at: SCENES.all, pan: 0, size: 1 },
      { kind: 'whoosh', at: SCENES.search, pan: -0.5, size: 0.8 },
      { kind: 'whoosh', at: PICK + 0.12, pan: 0.6, size: 0.5 },
      { kind: 'whoosh', at: SCENES.usage, pan: -0.4, size: 0.9 },
      ...LABELS.map((at) => ({ kind: 'pop', at })),
      { kind: 'pop', at: RING },
      ...[...WORD].map((_, n) => ({ kind: 'key', at: typedAt(n) })),
      { kind: 'click', at: PICK },
      { kind: 'click', at: RESUME },
      { kind: 'chime', at: RESUME + 0.05 },
      ...Array.from({ length: bars }, (_, n) => ({ kind: 'tick', at: barAt(n) })),
      { kind: 'riser', at: RISE, until: IMPACT },
      { kind: 'impact', at: IMPACT },
      { kind: 'shimmer', at: IMPACT + 0.05 },
    ];
  }

  return {
    cut, BPM, BEAT, BAR, STEP, LENGTH, INTRO, SCENES, RISE, IMPACT, GROOVE, GATHER, OUTLINES, SAID,
    WORD, typedAt, PICK, RESUME, LABELS, RING, barAt, cues,
  };
}
