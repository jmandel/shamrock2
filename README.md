# Shamrock: a peer-to-peer clover clue game in Rust → wasm

A So Clover!-style word game, dealt from a 220-card deck (`core/src/shamrock.json`). Cards are dealt whole from a draw-down deck, so none repeats until the deck runs out. Browsers connect directly over WebRTC: there is no
game server, and signaling goes through public Nostr relays via
[Trystero](https://github.com/dmotz/trystero). Each player rotates the board
in their own view, while card placement is shared. Cards on the board turn
with your view; cards off the board stay where they are.

This is an experiment in the workflow of "delete abstraction layers, write a
validation harness, let the agent hill-climb it". See [DECISIONS.md](DECISIONS.md)
for the up-front decisions.

**Play:** https://joshuamandel.com/shamrock2/#pick-a-room-name (share the link; same room = same table)

## Layout

```
core/                Rust crate; all game logic. No dependencies, no wasm-bindgen.
  src/model.rs       replicated state: LWW registers + Lamport stamps, wire format
  src/app.rs         per-player view rotation, input, drag/drop, draw-command buffer
  src/app/page.rs    what the page says about the table: status strip (words, shorter
                     versions, buttons), menu, every sheet's content (seats, players,
                     clovers, share list, recap), notice sentences, whether a save is good
  src/pages.rs       what the page says from this device's own records (Past games, its
                     tables, what happened, a finished clover read back), table names
  src/json.rs        just enough JSON to read back what the page stores
  src/lib.rs         raw extern "C" exports
  tests/             native property tests (convergence, rotation invariants, …) and the
                     network simulator, which also checks every status strip it shows
web/                 static site: index.html, main.js, core.wasm, vendored Trystero.
                     main.js does what needs the browser (WebRTC, storage, DOM and CSS,
                     font measuring, encryption, links, dates in the local format) and
                     lays out what the core says; every decision and word is the core's.
legacy-web.html      deployed as /web/: sends old /web/ links to the site root
web/agent.js         the game as tools for AI assistants (WebMCP) and the agent play test
harness/serve.mjs    static server + local signaling relay
harness/run.mjs      Playwright scorecard: real tabs, real WebRTC, real pointer events
```

## Run it

```bash

bun install            # Trystero + Playwright (only needed for build/harness)
./build.sh             # cargo → web/core.wasm, bundle Trystero into web/vendor/
node harness/serve.mjs # prints a URL
```

- `http://localhost:8787/#myroom` uses public Nostr relays for signaling, so it works across machines.
- `?signal=ws://localhost:8788#myroom` uses the bundled local relay.
- `?net=local#myroom` uses BroadcastChannel, for same-browser tabs with no WebRTC.

`web/` is fully static, so any static host works. Pages serves it at the site root. Anyone with the same `#room` joins the table.

**Connections on restrictive networks.** Nostr introduces browsers to one another; WebRTC then needs a working route between them. Some Wi-Fi, VPN and mobile networks need a TURN relay for that second step. Configure one in `web/network.json`, either as `{"iceServers": [{"urls": ["turns:your-relay.example:443?transport=tcp"], "username": "browser-user", "credential": "browser-credential"}]}` or as `{"iceServersUrl": "https://your-credential-service.example/ice"}`. The endpoint may return an ICE-server array or an object with an `iceServers` array; it must allow requests from the site's origin. Include TURN over TLS/TCP on port 443 for networks that block UDP. These credentials are sent to browsers, so keep provider API keys on the credential service, never in this static file. Issue credentials with a lifetime long enough for a game; a reload fetches new ones. [Trystero's connection guide](https://github.com/dmotz/trystero#troubleshooting-connection-issues) explains the transport, and [Cloudflare documents a credential endpoint](https://developers.cloudflare.com/realtime/turn/generate-credentials/).

The checked-in configuration has no TURN service enabled. Failed credential requests fall back to direct connections. Discovery and peer retries are handled by Trystero; the page does not repeatedly leave the room during negotiation. If discovery succeeds but a peer cannot connect, the status says **Couldn't connect** and suggests another network, instead of waiting indefinitely for someone who is already there. TURN relays encrypted WebRTC traffic; game state still lives on the players' devices.

**How a game goes** (So Clover! rules, everyone at once):

1. **Join.** Your first visit asks for a name: tap Join, or *Just watch*. Names are identities, so picking a name that's already at the table takes over that seat (to switch devices, or to debug). Your device remembers your name for this room.
2. **Start with N players:** one press by anyone. The button names who will be dealt in (seated players at the table now); that roster is fixed for the round, and anyone who joins later is dealt in next round (they can help guess meanwhile).
3. **Write.** Everyone gets a private clover with 4 locked cards in random rotations. Tap a leaf, type a clue linking the two words facing it, then Enter; the board turns so the next empty leaf faces you. Once all four clues are in, pick how many decoy cards to mix in with the − / + on the board, then **Submit**. You can keep editing clues and decoys after submitting, right up until guessing starts. Clearing a clue un-submits, and **Unsubmit my clover** in the ⋯ menu withdraws. Drafts are shared state, so a takeover keeps them.
4. **The status line says who you're waiting for** in plain words ("Waiting for Ben"). Guessing starts by itself only when **all** of them have submitted, whether they're online, offline or gone. Anyone who has submitted can override with **Start anyway**. Every version of each player's clues is kept, so an edit that races Proceed resolves identically everywhere: whatever was in before the Proceed stamp counts. Clovers are guessed one at a time, in deal order, on a big shared board. Drag cards **anywhere**, like cards on a real table: they stay exactly where you leave them for everyone, even on top of each other, and a card only snaps when dropped close to a spot. Nothing else moves when you drop one. The card touched last sits on top. A card is in one hand at a time: one someone else is holding (outlined in their colour) can't be picked up or turned, and a tap says who has it; if two people grab it at the same moment, one hand keeps it on every screen and the other lets go. A card within reach of a spot counts for **Guess**; if several are, the nearest counts, and among equally near ones the one put there last (so taking the top card away leaves the one underneath counting). **Guess** boots any wrongly placed cards back to the tray, with a small shake and no commentary. Every screen remembers what a Guess rejected: a card back in a spot where it was rejected, turned the same way, shows a ✗ in its middle (as soon as you drag it there), and the spot's outline turns red. Right cards stay. There's no scoring and no limit: keep guessing until all four are right, and the clover glows.
While guessing, **tap the status line** ("Sam · 1/3 · 1 decoy ▾") to pick which clover everyone guesses: any order, jump ahead or back (solved ones show their answer). **Next clover** goes to the next unsolved one, and the round ends when all are solved.

5. **Next clover**, then the **recap**: when the last clover is solved, a sheet opens on everyone's screen with every clover of the round, one after another (scroll from one to the next): its clues, each with the two words it linked, the four answer cards showing their outward words, the decoys, and how many guesses it took, plus round totals. Tap the status line ("Round 1 is over") to see it again. It has *Share* and *Next round*. Then **Next round**. Seats and scores carry across rounds. The ⋯ menu has the invite link, switch player, watch, *Deal new cards*, *New game* (reset scores, keep whoever is here), and *Remove* for away players.

**Turning the board** is local to each player: tap ⟳ or the clover's round centre, or drag around it. A drag has momentum: let go of a quick flick and the board coasts on (about 0.18 s at the speed of its last 120 ms) before settling on the nearest quarter, so a short flick turns it a quarter while a slow nudge springs back. Tapping a leaf never turns the board: while writing, it opens that leaf's editor upright on screen, and each leaf shows its two keywords ("ocean & storm"). Cards on the board turn with it; cards in the tray stay put. Card turns, moves and board turns all animate. Each clover starts upright.

**No header bar.** The ⋯ menu button sits at the left of the bottom strip; players are listed by name (with what they're doing, and *Remove* for away players) in the menu. In the lobby, or whenever you're connected to nobody, the strip has an **Invite** button (the phone's share sheet, else copy link). The **tray** fits every card of the current clover without overlap, three to a row (a column in landscape) at your card size. A 5–6 card clover gets two rows at full size; only a clover that really has 7–8 cards takes a third row and shrinks its board to make room.

**The bottom bar is one thin strip**: a one-line status (whose clover, how many are in), the controls the moment needs and the main button; while writing, a thin coloured bar along its top fills as players submit. How-to tips show once per device (the next touch on the board dismisses them) and stay in the ⋯ menu. The menu also has **Full screen** where the browser supports it (Android, desktop); on iPhone, Add to Home Screen opens it full screen via the web manifest.

**Pinch zoom** is the board's own, per device: two fingers (or ctrl+wheel / a trackpad pinch) zoom and pan the board area up to 4×, while the bottom bar and menu stay normal size, so you can't get stranded zoomed in. While zoomed, one finger on a card still plays, and one finger anywhere else pans the view (the board only turns via ⟳ then; a tap is still a tap); **1×** (top-left, shown while zoomed) resets it. A second finger cancels whatever the first started (a dragged card goes back, nothing turns). The page itself doesn't pinch-zoom: zooming the whole page past the dock left the board filling the screen with no way back.

**Your tables and past games.** Opening the site without a room (no `#…`) shows the home page: *Start a table*, with a suggested table name (easy to type and unlikely to collide, like `merry-otter-cove`; ↻ for another, or type your own, used as typed but link-safe: "Family Night" → `family-night`) and your name, pre-filled with the last one you used anywhere; Start takes you there already seated. A table link you haven't joined opens a join sheet with the table and your usual name filled in: one tap on Join (editing the table name there switches tables). Then the tables you've played on this device (newest first, with the name you used; **Forget** drops one, and the name saved for it, from this device; its past games stay), and *Past games*. Every finished round is kept in this device's localStorage with its clovers, answers included: review each as a small clover (clues around the four answer cards, decoys listed, guesses taken), *Play these again* as solo play, or delete it. The ⋯ menu has *Past games…* and *All my tables* too. Nothing leaves the device.

**Light and dark.** The theme follows the device's light/dark setting; the ⋯ menu's *Theme* button cycles auto → light → dark (kept per device). Light is a pale sage table with white cards; dark is near-neutral charcoal with muted leaves and slightly dimmed cards so they don't glare. Rust draws the board from a palette per theme; every text pair in both passes WCAG AA (native test and harness).

**Start before you meet.** On the home page, choose *Let everyone start before we meet*, enter your name and your friends' names (one per line), then *Create group invite*. This reserves a clover for everyone, from 2 to 12 players, in a fresh room. Send the same invite link to the whole group. A first visit opens *Who are you?* with a large button for each invited name; it never picks a name based on another table. A name already online is marked and needs an extra confirmation for switching devices. Anyone can write and submit while everyone else is away. Clues stay on that browser until it connects with other players; when all reserved players' submissions have reached a device, guessing starts. *Start anyway* remains an explicit override. The invite carries only the initial deal, not later edits: a connected path between players is still needed to exchange their work. Subsequent rounds use the usual players-present-at-deal rule.

**Only some people are back?** A submitted player can press *Start anyway* to guess the available clovers. A reserved player arriving later sees their own unfinished clover, with *Add my clover* to submit and *Guess with group* to help immediately; *Write my clover* takes them back to their draft. A late submission becomes available in the same round without switching the current clover or clearing any guesses. Next clover visits the remaining unsolved clovers, including late ones. If the available clovers are already finished, a late submission reopens guessing and closes the recap. This lasts until the group explicitly starts the next round or a new game; someone not dealt into the current round can guess and will be dealt in next time.

**Saves do not expire.** Every table, including ordinary tables, restores its saved state regardless of age. *New game* explicitly starts over; *Forget* removes that table's save from this device. Browser data clearing can still remove local saves. Reopening an original group invite merges its starting state with the saved or live game and cannot roll a later round back. Group invite links are kept with Your tables and copied by both Invite and the menu. Protocol 4 uses writer identities for clover selection and still reads protocol 3 saves and invites.

**Finding each other.** Players meet through public Nostr relays (six large, widely run ones rather than Trystero's hashed pick of hobby relays). While a device is connected to nobody, the status line says *Looking for the others…*, and it re-announces from scratch after 12 s (backing off to 30 s) and as soon as the page returns to the foreground, which is what a manual reload used to fix.

**Card size** is per device: − / + in the ⋯ menu (narrow screens start larger). Positions on the board are shared; bigger cards just overlap more on your screen. At the largest size, cards in neighbouring spots touch edge to edge. In the tray, cards never grow past a third of the tray's width, so a saved big size can't make them overlap. At that limit + greys out and the menu says to pinch the board for more; − always shrinks what you see (it steps from the drawn size, not a bigger saved one).

**How tables stay in agreement** is written up in one place: [`docs/sync-model.html`](docs/sync-model.html), covering the cases, the shared state, which round wins, the notices, personal history, moving a group, self-healing, upgrades and the tests. In short: the room name is the table; only the seats and the current round sync; any change of round someone else makes is explained in one line (and listed under *What happened* in the ⋯ menu); every round you were in is kept in your Past games when it ends or is replaced; and when two groups who played apart meet in one room, they play together by default, while the group whose game was replaced can move to a fresh table in one tap, with the rest of that group offered one tap to follow.

**Presence** follows the WebRTC connections, not a heartbeat, so a phone that locks its screen or a backgrounded tab still counts as at the table.

**Layout:** the board grows to fill the screen. While guessing, a tray for loose cards sits below it (portrait) or beside it (landscape). Tray positions are stored relative to the tray, so they fit every screen.

## Sharing boards

At the end of a round, **Share** in the recap (or *Share boards* in the ⋯ menu) lists the round's clovers. Pick any (one for a single board, or a set) and copy the link. Everything travels in the link (`?b=`), so no server or file is involved. The boards are deflate-compressed before encryption: one clover is about 330 characters including the address, and four about 800. Links don't give answers away by accident: the Rust core scrambles each board (cards shuffled and randomly turned, the answer a separate card-and-turn line), and the JS shell only compresses that text and AES-GCM encrypts it under a key derived from the link's `#` fragment. (The device must decode it to play, so this guards against glances, not determined devtools digging.) Opening a share link is **solo play**: no room, no name, no connection to the old table or any new one. It goes straight into guessing the clovers in order under their writers' names, and Guess checks against the real answers. At the end, **Start your own table** opens a fresh room. Older links (JSON payloads) still open. The link includes the answers, so it's meant for people who weren't there.

## Importing an unfinished clover

Boards from elsewhere (e.g. read off a screenshot of an old game) become ordinary share links. Write the board as text: name, the four clues (top, right, bottom, left), then the answer cards for spots top-left, top-right, bottom-right and bottom-left (words clockwise from the top as they sit), then any decoys. Then call `window.__cl.shareFromText([text])` on any open page to get the link.

## Tools for AI assistants (WebMCP)

`web/agent.js` describes the game as tools an AI assistant can call: `get_table`, `join_table`, `start_round`, `write_clue`, `set_decoys`, `submit_clover`, `place_card`, `turn_card`, `check_guess`, `next_clover`, `choose_clover` and `get_recap`. In a browser with [WebMCP](https://github.com/webmachinelearning/webmcp) (`document.modelContext`, or the earlier `navigator.modelContext`), the page registers them for the browser's built-in agent; elsewhere it does nothing. Each tool goes through the same Rust entry points as touch input, and sees only what that player's screen shows: cards are numbered by their shuffled tray order, and `get_recap` (the answers) only works once the round has ended. The agent play-test driver uses the same tools (`window.__cl.tools`), and `harness/agentplay/tools-test.mjs` plays a whole three-player round through them alone. `harness/agentplay/mcp-game.mjs` goes further: it hands each player's tools to a separate Claude session over MCP, with nothing else to go on, and has them play a round.

## Harness

```bash
(cd core && cargo test --release)   # native properties, merge rules (tests/merge.rs), the network simulator (300 chaotic games)
node harness/run.mjs                # browser gates + metrics → harness/out/scorecard.json
node harness/faults.mjs             # fault scenarios in real browsers (ONLY=text runs one)
node harness/invites.mjs            # shared group link, name picker, partial groups, late submissions, old saves (CHROMIUM=/path/to/chromium if needed)
node harness/connections.mjs        # delayed/blocked ICE, recovery and credential failure; TURN_BIN=/path/to/turnserver also tests real TURN/TCP
node harness/longnames.mjs          # 24-character names on a 360 px phone: the status strip still fits
node harness/layout.mjs             # every screen and sheet, stress data, 6 sizes: generic layout faults (ONLY=state)
node harness/idle.mjs               # what an idle table costs: CPU, frames and paints per second (IDLE_S=20)
```

**Fault scenarios** (`harness/faults.mjs`) cover what the simulator can't: the page's own connection handling, reload and restore, the status line, real WebRTC and signaling. Each scenario is its own table with separate browser contexts (separate storage, like separate phones), driven through the agent tools, with faults from a test-only `?chaos` hook (`cut` silences a page both ways while its connections stay up, `delayMs` lags what it receives), the signaling relay stopping and restarting, and pages reloading or closing mid-gesture. Scenarios: four players joining at once before anyone has connected; a phone in a tunnel while the others carry on (no connection changes, so only the table-hash resync can heal it: about 4 s); a player 600 ms behind; reloading mid-writing (back in the seat with half-written clues intact); everyone closing the table and coming back (the first back restores it from their own save); the relay going down mid-game (connected players carry on, a newcomer is told they're waiting and finds the table when it returns); a phone dying mid-drag (the hold clears, the card is free); two hands reaching for the same card (the one holding it keeps it; grabbed at once over a laggy link, every screen settles on one hand and the card lands where that hand dropped it); being left alone (the status says so, and clears); two groups who played apart in one room meeting, one moving to a fresh table and the rest of its group following (one of them later, from a save); and a page hearing a newer protocol version (told to reload, stops sending changes).

**The network simulator** (`core/tests/network.rs`) plays whole games on a simulated mesh that behaves like the real transport (one connection per pair, in-order delivery per connection, messages lost when a connection closes, a full sync each time one opens) with latency and jitter, links dropping and reconnecting, partitions that heal, devices going offline, reloading (restoring their save and rejoining under their saved name 1.5 s later, as the page does) or closing for good, while 3–5 players act at random: joining, writing, submitting, Proceed, dragging (and abandoning drags), guessing, jumping, dealing again, New game. After the chaos everything heals and goes quiet, and each run must show one state on every device, nobody connected without their seat, no device's table gone back to less progress, no card left drawn as held, and a round that can still be played to its end. Besides 150 calm and 150 stormy games of everyday play, eight **story families** steer the faults towards one kind of situation each (a long split with both sides dealing, two groups that sat down apart meeting, everyone reloading at once, a New game during a split, edits crossing round changes, many names coming and going, a long session, a page hearing a newer protocol), 40 games each, and a move story has one group move to a new room while the rest follow (some later, some after a reload). Each family has a coverage rule that fails the test if its situation stopped occurring often enough (for example, another group at the table in at least four in five two-table games). `SEEDS=10` runs ten times as many of everything, and a failure prints its seed and last events (`SEED=n … -- --nocapture` replays it with a full log).

It found five real bugs, each now also a fixed scenario test. Two came from the design, and the fix was to simplify it:

- *Tables wiped each other.* State lived inside a "game" that replaced any other game wholesale when they met, so a page that sat down before syncing, or a reloaded player who dealt alone, could erase a table in play. There is no game container any more: seats belong to the table and are only ever merged, everything else belongs to a round, and one rule ranks rounds (epoch, then rounds dealt, then players dealt in, then age), where a New game is just the next epoch's lobby. Two tables that meet now merge, and a lone deal can't outrank a table further along.
- *Ghost holds.* "Someone is dragging this card" was stored in the shared table, so a phone that died mid-drag left it held forever. Holds now ride on the live pointer messages, which end when a device lets go, goes quiet or leaves; nothing to clean up.

The other three got their own mechanism: a table vanished if every device holding it reloaded (each device keeps the table in localStorage, restored on load without an age limit); a change that missed one peer never reached it if its sender then left (presence pings carry a hash of the table, and devices that stay different resync); and a round whose dealt-in players had all left with nothing submitted could only be escaped through the menu (the strip offers *Deal again*).

The browser harness drives the real UI through a whole game: 5 players (one phone-sized), a watcher, a seat takeover from a tablet, two rounds, and a New game. Latest scorecard (headless Chromium, WebRTC over loopback):

| metric | value |
|---|---|
| gates | 46/46 |
| every deck word fits its card edge at full size | widest: camouflage 141 of 146 units |
| worst text contrast (canvas + page) | 6.5 : 1 (WCAG AA ≥ 4.5) |
| presses to join / write a clover + Submit | 1 / 6 |
| last Submit → guessing starts | ~60 ms |
| card drop visible on 6 other screens (p50 / p95) | ~18 / ~33 ms |
| late visitor gets full state | ~300 ms |
| frame CPU, wasm + Canvas2D (p95) | ~1.3 ms |
| card word size while guessing, desktop / phone (default · largest) | 23.6 / 15.6 · 18.4 px |
| board width on a 390 px phone | 96% of screen |
| wasm size / JS imports | 220 KB / 0 |

Bugs the harness caught this iteration: dropping onto an occupied spot stacked cards without snapping, so turns went to the wrong card. A snapped card drew *under* a loose one, so taps hit the loose card. And a CSS `display` rule overrode `hidden`, leaving the decoy stepper showing during guessing (found by looking at screenshots, not by a gate). Adding the card-size control then surfaced two flaky failures. Repeating runs turned them into real bugs. Presence was a heartbeat sent from the render loop, so a throttled background tab looked like a player who had left. And any device, even a watcher, could auto-start guessing based on its own partial view of who was connected. Presence is now connection-based, and auto-start waits for every seated player.

## Look and feel

The mark is four rounded cards set as a clover, one of them gold (the decoy). The palette is green felt, paper cards and that one warm gold accent. The board is a four-leaf clover: a heart-shaped leaf on each side carries its clue in Fraunces, and the card spots sit in the round centre. Card words are in Figtree. Both fonts ship with the site (`web/fonts`), so there are no third-party requests. Cards are 210 board units against a 26-unit word size, with each word set 86 units in from the centre, so even the deck's longest word can't reach the corners where the neighbouring words sit (gated against the whole deck in the real font). Motion is understated: cards deal in from the centre with a short stagger, and turns, moves and board spins ease. The leaves glow softly when a clover is finished, sheets slide up, and buttons give a little on press. All of it is switched off under `prefers-reduced-motion`. Text can't be selected outside inputs, so a long press on mobile doesn't open dictionary lookups. Every canvas text colour pair is checked for WCAG AA contrast in both test suites.
