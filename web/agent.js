// Tools for AI assistants (WebMCP): the same moves a player makes, offered to
// a browser's built-in agent when the browser supports it, and to the agent
// play-test driver (harness/agentplay) through window.__cl.tools. Each tool
// acts through the same Rust entry points as touch input, and only sees what
// this player's own screen shows (answers stay hidden until a round ends).

const LEAVES = ['top', 'right', 'bottom', 'left'];
const RULES = 'Shamrock is a cooperative word game (So Clover! rules). Each player gets a clover: four cards in four spots '
  + '(0 top-left, 1 top-right, 2 bottom-right, 3 bottom-left), each card with a word on every edge. The top clue links the top '
  + 'words of spots 0 and 1, right links the right words of spots 1 and 2, bottom links the bottom words of spots 2 and 3, '
  + 'left links the left words of spots 3 and 0. Everyone writes four clues for their own clover (traditionally one word, '
  + 'lowercase unless a name or title, linking both words), then the table guesses each clover in turn: its cards are shuffled '
  + 'and turned in a tray with decoy cards mixed in, and the guessers place and turn them, then check. Right spots lock; wrong '
  + 'cards bounce back to the tray unturned (back to how they were dealt). Card numbers are tray labels only and say nothing '
  + 'about the answer. Each card\'s words are given by edge (top, right, bottom, left) as it faces now; turning it a quarter '
  + 'clockwise moves every word one edge on (left to top, top to right, and so on). A card in spot s shows two words outward: spot 0 its top and left, '
  + 'spot 1 top and right, spot 2 right and bottom, spot 3 bottom and left.';

/** The tool list. `g` is the page's game handle (see main.js). */
export function agentTools(g) {
  const view = () => g.agentView();
  const done = (result, ok = true) => ({ ok, result });
  const labelToIndex = (label) => view().cards.find((c) => c.card === label)?._i;
  const settle = () => g.X.settle();
  const point = (fn, ...a) => g.probe(fn, ...a).slice(0, 2);
  // A gesture into the Rust input path, in canvas coordinates.
  function drag(from, to) {
    g.X.pointer_down(...from);
    for (let s = 1; s <= 12; s++) g.X.pointer_move(from[0] + (to[0] - from[0]) * s / 12, from[1] + (to[1] - from[1]) * s / 12);
    g.X.pointer_up(...to);
    g.sync();
  }
  const card = { type: 'integer', minimum: 0, description: 'Card number as listed by get_table (a tray label)' };
  const cloverArg = { type: 'integer', minimum: 0, description: 'The clover you are looking at: clover.index in get_table' };
  const topArg = { type: 'string', description: 'The word that should end up on the card\'s top edge' };
  const cardOf = (i) => view().cards.find((c) => c._i === i);
  // A move meant for one clover never lands on another's cards.
  const moved = (clover) => {
    const u = g.ui();
    if (u.phase !== 'guessing') return `guessing isn't under way (the table is in ${u.phase})`;
    if (clover !== u.k) return `the table has moved on: it's on clover ${u.k} (${u.writerName}'s) now, not clover ${clover}. Nothing was moved; look again (get_table) before moving cards`;
    return null;
  };
  const busy = (i, label) => { const by = cardOf(i)?.heldBy; return by ? `${by} is moving card ${label} right now; wait for them to let go` : null; };
  // Why a move didn't land: usually someone else moved the same card at the same moment.
  const missed = (c, label, what) => {
    const me = g.ui().name, by = c.movedBy;
    return `card ${label} ${what}: ${by && by !== me ? `${by} moved it at the same moment` : 'it was moved again before this landed'}; it is at ${c.at}, reading ${c.words.join(' / ')}`;
  };
  const tap = (i) => { settle(); const p = point(g.X.probe_card, i); g.X.pointer_down(...p); g.X.pointer_up(...p); g.sync(); };
  // Turn until `top` is on the top edge (at most three quarter turns). Null when it is.
  const turnTo = (i, label, top) => {
    for (let k = 0; k < 4 && cardOf(i).words[0] !== top; k++) tap(i);
    const c = cardOf(i);
    return c.words[0] === top ? null : missed(c, label, `doesn't have "${top}" on top`);
  };

  return [
    {
      name: 'get_table',
      description: `Read the table: phase (lobby, writing, guessing, end), players, and the clover on screen with its clues and cards (rejectedHere on a card: a check already rejected it in that spot, turned that way). ${RULES}`,
      annotations: { readOnlyHint: true },
      inputSchema: { type: 'object', properties: {} },
      run() {
        const u = g.ui(), v = view();
        return {
          phase: u.phase, round: u.round, you: u.name ?? null, seated: u.seated,
          players: u.players.map((p) => ({ name: p.name, status: p.status, away: p.away, you: p.you })),
          status: g.status(),
          decoys: u.decoys, readyCount: u.readyCount,
          clover: {
            index: u.k, count: u.n, writer: u.writerName ?? null, isMine: !!u.isWriter, solved: !!u.done,
            clues: Object.fromEntries(LEAVES.map((l, d) => [l, v.clues[d] || null])),
            // While writing your own clover: the two words each leaf must link.
            leafWords: Object.fromEntries(LEAVES.map((l, d) => [l, v.leafWords[d]])),
            // Spots locked by a right guess (guessing only).
            solvedSpots: v.solvedSlots,
            // words: by edge, as the card is turned now. at:
            // "tray", "slot N", or "board" (set down outside any spot).
            // rejectedHere: an earlier check already rejected this card in
            // this spot, turned as it is now.
            // heldBy: another player is moving this card right now (a card
            // is in one hand at a time).
            // movedBy: who last moved or turned it.
            cards: v.cards.map(({ _i, ...c }) => ({ ...c, words: Object.fromEntries(LEAVES.map((l, e) => [l, c.words[e]])) })),
          },
        };
      },
    },
    {
      name: 'join_table',
      description: 'Take a seat at this table under a name (or rejoin as yourself).',
      inputSchema: { type: 'object', properties: { name: { type: 'string', maxLength: 24 } }, required: ['name'] },
      run({ name }) { g.joinAs(String(name ?? '')); return done(g.ui().seated ? `seated as ${g.ui().name}` : 'could not join', g.ui().seated); },
    },
    {
      name: 'start_round',
      description: 'Deal a new round to everyone seated (from the lobby, or after a round ends).',
      inputSchema: { type: 'object', properties: {} },
      run() {
        const u = g.ui();
        if (!['lobby', 'end'].includes(u.phase)) return done(`a round is already under way (${u.phase})`, false);
        if (!u.dealable.length) return done('nobody is seated yet', false);
        g.act('deal');
        return done(`dealt to ${u.dealable.join(', ')}`);
      },
    },
    {
      name: 'write_clue',
      description: 'Write the clue on one leaf of your own clover (writing phase). It should link both words that leaf faces (see leafWords in get_table).',
      inputSchema: { type: 'object', properties: { leaf: { enum: LEAVES }, text: { type: 'string', maxLength: 40 } }, required: ['leaf', 'text'] },
      run({ leaf, text }) {
        const d = LEAVES.indexOf(leaf);
        if (d < 0) return done('leaf must be top, right, bottom or left', false);
        if (g.ui().phase !== 'writing') return done('clues are written in the writing phase', false);
        g.X.set_clue(d, g.writeText(String(text ?? '')), 0);
        g.sync();
        const now = view().clues[d];
        return done(`${leaf} clue is now "${now}"`, now === String(text ?? '').trim());
      },
    },
    {
      name: 'set_decoys',
      description: 'Choose how many decoy cards (0-4) get mixed in when others guess your clover.',
      inputSchema: { type: 'object', properties: { count: { type: 'integer', minimum: 0, maximum: 4 } }, required: ['count'] },
      run({ count }) { g.X.set_decoys(Math.max(0, Math.min(4, count | 0))); g.sync(); return done(`decoys: ${g.ui().decoys}`); },
    },
    {
      name: 'submit_clover',
      description: 'Submit your four clues. Everyone submitting starts guessing; a late submission adds your clover to the round already being guessed.',
      inputSchema: { type: 'object', properties: {} },
      run() {
        const u = g.ui();
        if (u.ready) return done('already submitted');
        if (!u.canReady) return done('write all four clues first', false);
        g.act('ready');
        return done('submitted');
      },
    },
    {
      name: 'place_card',
      description: 'Move a card into a spot (0 top-left, 1 top-right, 2 bottom-right, 3 bottom-left) or back to the tray, while guessing. '
        + 'Give `clover`, the clover you are looking at (clover.index in get_table): if the table has moved on, the move is refused instead of landing on another clover\'s cards. '
        + 'Give `top`, the word that should end up on the card\'s top edge, to place and turn it in one step; this is safe to repeat (if two players carry out the same plan, the card still ends up turned once). '
        + 'Without `top` the card keeps its turn. Cards may share a spot, like cards on a real table: nothing else moves, and the card put there last counts for check_guess (move the other one away, e.g. to the tray, to tidy up).',
      inputSchema: { type: 'object', properties: { card, spot: { oneOf: [{ type: 'integer', minimum: 0, maximum: 3 }, { const: 'tray' }] }, clover: cloverArg, top: topArg }, required: ['card', 'spot', 'clover'] },
      run({ card: label, spot, clover, top }) {
        const stale = moved(clover);
        if (stale) return done(stale, false);
        const i = labelToIndex(label);
        if (i === undefined) return done(`no card ${label}`, false);
        const held = busy(i, label);
        if (held) return done(held, false);
        if (top !== undefined && !cardOf(i).words.includes(top)) return done(`card ${label} has no word "${top}" (it reads ${cardOf(i).words.join(' / ')})`, false);
        const want = spot === 'tray' ? 'tray' : `slot ${spot}`;
        // Already there: nothing to drag (so repeating a move changes nothing).
        if (cardOf(i).at !== want) {
          settle();
          const to = spot === 'tray' ? g.trayPoint() : point(g.X.probe_slot, spot);
          drag(point(g.X.probe_card, i), to);
        }
        const c = cardOf(i);
        if (c.at !== want) return done(missed(c, label, `isn't in ${spot === 'tray' ? 'the tray' : `spot ${spot}`}`), false);
        if (top !== undefined) {
          const t = turnTo(i, label, top);
          if (t) return done(t, false);
        }
        return done(`card ${label} is ${spot === 'tray' ? 'in the tray' : `in spot ${spot}`}, reading top/right/bottom/left: ${cardOf(i).words.join(' / ')}`);
      },
    },
    {
      name: 'turn_card',
      description: 'Turn a card, while guessing. Give `top`, the word that should end up on its top edge: this is safe to repeat (two players carrying out the same plan still leave it turned once). '
        + 'Or give `times`, quarter turns clockwise: each word moves one edge on (left to top, top to right, right to bottom, bottom to left). '
        + 'Give `clover` (clover.index in get_table) so a turn meant for one clover never lands on another.',
      inputSchema: { type: 'object', properties: { card, clover: cloverArg, top: topArg, times: { type: 'integer', minimum: 1, maximum: 3 } }, required: ['card', 'clover'] },
      run({ card: label, clover, top, times }) {
        const stale = moved(clover);
        if (stale) return done(stale, false);
        const i = labelToIndex(label);
        if (i === undefined) return done(`no card ${label}`, false);
        const held = busy(i, label);
        if (held) return done(held, false);
        if (top !== undefined) {
          if (!cardOf(i).words.includes(top)) return done(`card ${label} has no word "${top}" (it reads ${cardOf(i).words.join(' / ')})`, false);
          const t = turnTo(i, label, top);
          if (t) return done(t, false);
        } else {
          for (let t = 0; t < (times ?? 1); t++) tap(i);
        }
        return done(`card ${label} reads top/right/bottom/left: ${cardOf(i).words.join(' / ')}`);
      },
    },
    {
      name: 'check_guess',
      description: 'Check the cards in the spots. Right spots lock (see solvedSpots); wrong cards go back to the tray, unturned, and need placing and turning again. '
        + 'Any guesser may check at any time; try to agree first. Give `clover` (clover.index in get_table) so a check meant for one clover never checks another.',
      inputSchema: { type: 'object', properties: { clover: cloverArg }, required: ['clover'] },
      run({ clover }) {
        const stale = moved(clover);
        if (stale) return done(stale, false);
        const u = g.ui();
        // Someone else's check may have just solved it.
        if (u.done) return done(`clover ${u.k} is already solved; next_clover with from: ${u.k} moves on`);
        if (u.isWriter) return done('this is your own clover: the others guess it', false);
        if (!u.canCheck) return done('fill all four spots first', false);
        g.act('check');
        const v = view();
        return done(v.solvedSlots.length === 4 ? `solved clover ${u.k}! (next_clover with from: ${u.k} moves on)` : `right so far: spots ${v.solvedSlots.join(', ') || 'none'}`);
      },
    },
    {
      name: 'next_clover',
      description: 'Once the clover on screen is solved, move the whole table on to the next one (or finish the round after the last). '
        + 'Pass `from`, the index of the clover you solved: if someone already moved the table on from it, that counts as done.',
      inputSchema: { type: 'object', properties: { from: { type: 'integer', minimum: 0, description: 'Index of the solved clover you are moving on from (clover.index in get_table)' } } },
      run({ from } = {}) {
        const u = g.ui();
        const now = () => { const v = g.ui(); return v.phase === 'end' ? 'the round is over' : `now clover ${v.k} of ${v.n} (${v.writerName}'s)`; };
        // Everyone shares one current clover: another player may have moved on first.
        if (from !== undefined && (u.phase !== 'guessing' || u.k !== from) && u.boards[from]?.solved) return done(`someone already moved the table on from clover ${from}; ${now()}`);
        if (u.phase === 'end') return done('the round is already over');
        if (!u.done) return done(`clover ${u.k} (${u.writerName}'s) is on screen and isn't solved yet${from === undefined ? '; if you just solved another, someone already moved the table on' : ''}`, false);
        g.act('next');
        return done(now());
      },
    },
    {
      name: 'choose_clover',
      description: 'While guessing, switch the table to another clover (numbered from 0 in the order they were dealt).',
      inputSchema: { type: 'object', properties: { clover: { type: 'integer', minimum: 0 } }, required: ['clover'] },
      run({ clover }) { g.X.jump(clover | 0); g.sync(); return done(`clover ${g.ui().k} of ${g.ui().n}`, g.ui().k === (clover | 0)); },
    },
    {
      name: 'get_recap',
      description: 'After a round ends: every clover with its clues, the two words each clue linked, the answer cards in spot order (each card\'s words as [top, right, bottom, left] as it sits), decoys, and how many guesses it took.',
      annotations: { readOnlyHint: true },
      inputSchema: { type: 'object', properties: {} },
      run() {
        if (g.ui().phase !== 'end') return done('the recap is available once the round has ended', false);
        return g.review().map((b) => {
          const [writer, ...rest] = b.text.split('\n');
          const cards = rest.slice(4).map((l) => l.split('|'));
          // Leaf d links spot d and the next spot clockwise, both on side d.
          const clues = Object.fromEntries(LEAVES.map((l, d) => [l, { clue: rest[d], linked: [cards[d]?.[d], cards[(d + 1) % 4]?.[d]] }]));
          return { writer, clues, answer: cards.slice(0, 4), decoys: cards.slice(4), guesses: b.guesses, solved: b.solved };
        });
      },
    },
  ];
}

/** Offer the tools to the browser's agent, if it has one (WebMCP). The API
 *  is still settling: `document.modelContext` is current, with the earlier
 *  `navigator.modelContext` as a fallback. Does nothing elsewhere. */
export function offerTools(tools) {
  const mc = document.modelContext ?? navigator.modelContext;
  if (typeof mc?.registerTool !== 'function') return false;
  for (const t of tools) {
    const { run, ...spec } = t;
    try {
      Promise.resolve(mc.registerTool({
        ...spec,
        async execute(args) { return { content: [{ type: 'text', text: JSON.stringify(await run(args ?? {})) }] }; },
      })).catch(() => {});
    } catch {}
  }
  return true;
}
