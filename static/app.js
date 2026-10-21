const $ = id => document.getElementById(id);
// Discord loads Activities with ?frame_id=...; anything else is plain-browser mode.
const inDiscord = new URLSearchParams(location.search).has('frame_id');
// sessionStorage, not localStorage: each tab is its own player, so one PC can test with several tabs.
let key = inDiscord ? null : sessionStorage.getItem('ninjaKey');
let lastPrompt = '';
let lastPlayers = '';

function el(tag, text, cls) { const e = document.createElement(tag); if (text != null) e.textContent = text; if (cls) e.className = cls; return e; }
// A card name in bold with its rules text underneath.
function cardFace(e, name, text) { e.append(el('b', name)); if (text) e.append(el('span', text, 'sub')); return e; }

async function api(path, body) {
  const r = await fetch(path, body ? { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) } : undefined);
  if (!r.ok) { const e = new Error(await r.text()); e.status = r.status; throw e; }
  return r.status === 204 ? null : r.json();
}

// Sign in with the Discord account and sit at this Activity instance's table.
async function discordJoin() {
  $('homeForm').hidden = true;
  $('joinErr').textContent = 'Connecting to Discord…';
  try {
    const { DiscordSDK } = await import('/discord.js');
    const { clientId } = await api('/api/config');
    const sdk = new DiscordSDK(clientId);
    await sdk.ready();
    const { code } = await sdk.commands.authorize({ client_id: clientId, response_type: 'code', state: '', prompt: 'none', scope: ['identify'] });
    const { access_token } = await api('/api/token', { code });
    key = (await api('/api/join', { room: sdk.instanceId, access_token })).key;
    poll();
  } catch (e) { $('joinErr').textContent = e.message || String(e); }
}

// Home page: create a lobby or join one by code. The name is remembered for next time (per browser only).
try { $('name').value = localStorage.getItem('ninjaName') || ''; } catch (e) { /* storage blocked */ }
$('code').value = (new URLSearchParams(location.search).get('code') || '').toUpperCase().slice(0, 5);
$('code').oninput = () => { $('code').value = $('code').value.toUpperCase(); };
async function enter(create) {
  const name = $('name').value.trim();
  if (!name) { $('joinErr').textContent = 'Enter your name first.'; $('name').focus(); return; }
  if (!create && !$('code').value.trim()) { $('joinErr').textContent = 'Enter a lobby code to join.'; $('code').focus(); return; }
  try {
    const r = create ? await api('/api/create', { name }) : await api('/api/join', { name, room: $('code').value });
    try { localStorage.setItem('ninjaName', name); } catch (e) { /* storage blocked */ }
    key = r.key;
    sessionStorage.setItem('ninjaKey', key);
    $('joinErr').textContent = '';
    poll();
  } catch (e) { $('joinErr').textContent = e.message; }
}
$('createBtn').onclick = () => enter(true);
$('homeForm').onsubmit = ev => { ev.preventDefault(); enter(false); };

// Errors go on the page: Discord's iframe may block alert().
const PHASE_NAMES = ['Spy', 'Mystic', 'Trickster', 'Blind Assassin', 'Shinobi', 'Mastermind'];
const STEPS = ['Draft', ...PHASE_NAMES, 'Dawn'];
const PLAY_MARK = { done: '✓', now: '▶', next: '…', skipped: '☠' };

// The Night tracker: where we are, what's resolving right now, and what just happened.
function renderNight(v) {
  // Phase bar across the top, from the draft to dawn.
  $('phases').hidden = v.track == null;
  if (v.track != null) {
    $('phases').replaceChildren(...STEPS.map((n, i) => {
      const li = el('li', n, i < v.track ? 'done' : i === v.track ? 'now' : '');
      li.title = n;
      if (i === v.track) li.setAttribute('aria-current', 'step');
      return li;
    }));
  }
  $('night').hidden = !(v.track >= 1);
  if (!(v.track >= 1)) return;
  // Every card of the latest phase, who played it, and how far it's got.
  $('playsTitle').textContent = v.plays_phase == null ? 'Cards played' : `${PHASE_NAMES[v.plays_phase]} phase cards`;
  $('plays').replaceChildren(...(v.plays.length
    ? v.plays.map(pl => {
        const li = el('li', null, pl.state);
        li.append(el('span', PLAY_MARK[pl.state], 'mark'), el('span', `${pl.card.name} — ${v.players[pl.seat].name}`));
        if (pl.state === 'skipped') li.title = 'Its owner died before it resolved';
        return li;
      })
    : [el('li', v.plays_phase == null ? 'Waiting for the first cards to be revealed…' : 'Nobody played a card.', 'next')]));
  const spot = $('spot');
  spot.replaceChildren();
  if (v.acting) {
    const name = s => v.players[s].name;
    spot.append(el('span', '▶ ', 'arrow'), el('b', `${name(v.acting.seat)}'s ${v.acting.card.name}`));
    if (v.acting.targets.length) spot.append(' → ', el('b', v.acting.targets.map(name).join(' & ')));
    spot.append(el('span', v.acting.card.text, 'sub'));
  } else {
    spot.append(el('span', v.track > PHASE_NAMES.length ? 'Dawn: every House card is revealed.' : `${PHASE_NAMES[v.track - 1]} phase: everyone is deciding whether to play cards…`, 'sub'));
  }
}

async function leave() {
  const b = $('leaveBtn');
  if (!b.classList.contains('confirm')) {
    b.classList.add('confirm');
    b.textContent = 'Really leave?';
    setTimeout(() => { b.classList.remove('confirm'); b.textContent = 'Leave'; }, 4000);
    return;
  }
  try { await api('/api/leave', { key }); } catch (e) { /* already gone */ }
  key = null;
  sessionStorage.removeItem('ninjaKey');
  lastPrompt = lastPlayers = '';
  show(false);
  $('joinErr').textContent = inDiscord ? 'You left the game. Close and reopen the Activity to join again.' : 'You left the game.';
}
$('leaveBtn').onclick = leave;

// On phones the side panel is a drawer; these buttons only show there.
function setDrawer(open) {
  $('side').classList.toggle('open', open);
  $('logBtn').setAttribute('aria-expanded', String(open));
}
$('logBtn').onclick = () => setDrawer(!$('side').classList.contains('open'));
$('sideClose').onclick = () => { setDrawer(false); $('logBtn').focus(); };

// Help overlay. The card list comes from the server's real deck, so it can't drift from the game.
let cardsLoaded = false;
async function openHelp() {
  $('help').hidden = false;
  $('helpClose').focus();
  if (cardsLoaded) return;
  try {
    const cards = await api('/api/cards');
    const list = $('cardList');
    list.replaceChildren();
    let phase = '';
    for (const c of cards) {
      if (c.phase !== phase) { phase = c.phase; list.append(el('h3', { React: 'Reacts', Mastermind: 'At dawn' }[phase] ?? `${phase} phase`)); }
      const row = el('div', null, 'cardRow');
      const copies = c.numbers.length;
      const nums = c.numbers[0] === 0 ? '' : ` · #${c.numbers.length > 1 ? c.numbers[0] + '–' + c.numbers[copies - 1] : c.numbers[0]}`;
      const title = el('b', c.name);
      row.append(title);
      if (c.honor) row.append(el('span', 'Honor mode only', 'tag'));
      row.append(el('span', `${copies} ${copies === 1 ? 'copy' : 'copies'}${nums}`, 'sub'), el('span', c.text, 'sub'));
      list.append(row);
    }
    cardsLoaded = true;
  } catch (e) { $('cardList').textContent = 'Could not load cards: ' + e.message; }
}
function closeHelp() { $('help').hidden = true; $('helpBtn').focus(); }
$('helpBtn').onclick = openHelp;
$('helpClose').onclick = closeHelp;
$('help').onclick = e => { if (e.target === $('help')) closeHelp(); };
document.addEventListener('keydown', e => { if (e.key === 'Escape' && !$('help').hidden) closeHelp(); });

async function act(action) {
  $('actErr').textContent = '';
  try { await api('/api/act', { key, action }); lastPrompt = ''; lastPlayers = ''; poll(); }
  catch (e) { $('actErr').textContent = e.message; }
}

async function poll() {
  if (!key) return show(false);
  try { render(await api('/api/state?key=' + key)); show(true); }
  catch (e) { if (e.status === 401) { key = null; sessionStorage.removeItem('ninjaKey'); show(false); } }
}

function show(inGame) {
  $('joinBox').hidden = inGame;
  for (const id of ['prompt', 'you', 'table', 'logBox', 'side']) $(id).hidden = !inGame;
  if (!inGame) { $('night').hidden = true; $('phases').hidden = true; $('status').textContent = ''; }
  $('leaveBtn').hidden = !inGame;
  $('logBtn').hidden = !inGame;
  if (!inGame) setDrawer(false);
  document.querySelector('main').classList.toggle('solo', !inGame); // no empty side column on the join screen
  if (!inGame) $('settings').hidden = true; // otherwise render() decides (lobby only)
}

function render(v) {
  $('status').textContent = v.status + (v.winners.length ? ' — winner: ' + v.winners.join(' & ') : '');
  $('house').textContent = v.house;
  $('hand').replaceChildren(...(v.hand.length ? v.hand.map(c => cardFace(el('span', null, 'chip'), c.name, c.text)) : [el('span', 'none')]));
  const score = v.tokens.reduce((a, b) => a + b, 0);
  $('tokens').textContent = v.tokens.length ? `${v.tokens.join(' + ')} = ${score} / 10` : '0 / 10';
  $('honorRow').hidden = v.no_honor;
  // Rebuilt only on change, so the host's Kick / Make host buttons don't vanish mid-click.
  const amHost = v.me === v.host;
  renderNight(v);
  const tableSig = JSON.stringify([v.players, v.host, v.lobby, v.me, v.no_honor, v.turn_secs, v.acting, v.code]);
  if (tableSig !== lastPlayers) {
    lastPlayers = tableSig;
    $('players').replaceChildren(...v.players.map((p, i) => {
      const acting = v.acting && v.acting.seat === i, targeted = v.acting && v.acting.targets.includes(i);
      const li = el('div', null, `seat${p.alive ? '' : ' dead'}${i === v.me ? ' me' : ''}${acting ? ' acting' : ''}${targeted ? ' targeted' : ''}`);
      if (acting) li.append(el('div', `▶ ${v.acting.card.name}`, 'flag'));
      if (targeted) li.append(el('div', '🎯 Target', 'flag'));
      // Avatar: Discord picture, else 🤖 for bots or the name's first letter.
      let av;
      if (p.avatar) { av = document.createElement('img'); av.src = p.avatar; av.alt = ''; }
      else av = el('div', p.bot ? '🤖' : [...p.name][0].toUpperCase());
      av.classList.add('avatar');
      const name = p.bot ? p.name.replace(/^🤖 /, '') : p.name; // the avatar already says 🤖
      const info = el('div', null, 'info');
      const nameEl = el('div', `${p.alive ? '' : '💀 '}${name}${i === v.host ? ' 👑' : ''}${i === v.me ? ' (you)' : ''}`, 'seatName');
      nameEl.title = p.name;
      info.append(nameEl);
      const top = el('div', null, 'top');
      top.append(av, info);
      li.append(top);
      // House: public once revealed, or 🔒 when only you know it (from your peeks).
      // Your own card shows what you believe. Dashed = could be out of date after a Shapeshifter swap.
      const mine = i === v.me && !p.house ? v.belief : null;
      const house = p.house ?? mine;
      const kind = house ? house.split(' ')[0].toLowerCase() : 'unknown';
      const badge = el('span', house ? (p.private ? '🔒 ' : '') + house : (v.lobby ? 'No House yet' : 'House ?'), `house ${kind}${mine || p.private ? ' guess' : ''}`);
      if (p.private) badge.title = 'Only you know this, from your own peek';
      const stats = el('div', null, 'stats');
      stats.append(badge);
      if (!v.no_honor) stats.append(el('span', `🏅 ${p.tokens}`));
      const hand = el('span');
      hand.setAttribute('aria-label', `${p.cards} Ninja card${p.cards === 1 ? '' : 's'}`);
      hand.title = hand.getAttribute('aria-label');
      for (let c = 0; c < p.cards; c++) hand.append(el('span', null, 'cb'));
      hand.append(` ${p.cards}`);
      stats.append(hand);
      info.append(stats);
      if (p.death) li.append(el('div', `☠ ${p.death}`, 'death'));
      // Face-up cards for everyone; 🔒 = a card only you saw in their hand (Mystic).
      if (p.played.length || p.peeked) {
        const shown = el('div', null, 'shown');
        for (const c of p.played) { const m = el('span', c.name, 'mini'); m.title = c.text; shown.append(m); }
        if (p.peeked) {
          const m = el('span', '🔒 ' + p.peeked.name, 'mini peek');
          m.title = `In their hand (only you saw it): ${p.peeked.text}`;
          shown.append(m);
        }
        li.append(shown);
      }
      const small = (text, action) => { const b = el('button', text); b.onclick = () => act(action); li.append(b); };
      if (amHost && i !== v.me) {
        if (!p.bot) small('Make host', { type: 'MakeHost', who: i });
        if (v.lobby) small(p.bot ? 'Remove' : 'Kick', { type: 'Kick', who: i });
      }
      return li;
    }));
    // Web lobbies show their code and an invite link while people can still join.
    const lc = $('lobbyCode');
    lc.hidden = !(v.code && v.lobby);
    if (!lc.hidden) {
      const copy = el('button', 'Copy invite link');
      copy.onclick = async () => {
        try { await navigator.clipboard.writeText(`${location.origin}/?code=${v.code}`); copy.textContent = 'Copied!'; }
        catch (e) { copy.textContent = `${location.origin}/?code=${v.code}`; }
      };
      lc.replaceChildren(el('span', 'Lobby code', 'sub'), el('b', v.code), copy);
    }
    const bar = $('hostBar');
    bar.replaceChildren();
    // Game settings live in their own box, shown only in the lobby (where they can change).
    const sets = $('settingsBody');
    sets.replaceChildren();
    $('settings').hidden = !v.lobby;
    if (amHost && v.lobby) {
      if (v.players.length < v.caps[v.no_honor ? 1 : 0]) {
        const b = el('button', '🤖 Add bot'); b.onclick = () => act({ type: 'AddBot' }); bar.append(b);
      }
      const l = el('label'); const c = document.createElement('input'); c.type = 'checkbox'; c.checked = !v.no_honor;
      c.onchange = () => act({ type: 'SetNoHonor', on: !c.checked });
      l.append(c, ' Honor mode', el('span', `Play several rounds to 10 points with Honor tokens. Adds the Martyr, Soul Merchant and Thief. Up to ${v.caps[0]} players (${v.caps[1]} without).`, 'sub'));
      sets.append(l);
      const l2 = el('label', 'Turn timer ');
      const turn = document.createElement('select');
      for (const secs of [0, 30, 60, 90, 120]) {
        const o = el('option', secs ? `${secs} seconds` : 'Off'); o.value = secs; o.selected = secs === v.turn_secs; turn.append(o);
      }
      turn.onchange = () => act({ type: 'SetTimer', secs: +turn.value });
      l2.append(turn); sets.append(l2);
    } else if (v.lobby) {
      const fmt = s => s ? `${s} seconds` : 'off';
      sets.append(
        el('div', `Honor mode: ${v.no_honor ? 'off' : 'on'}`),
        el('div', `Turn timer: ${fmt(v.turn_secs)}`),
        el('span', 'Only the host can change these.', 'sub'),
      );
    }
  }
  $('log').replaceChildren(...v.log.map(s => el('li', s)));
  // Only rebuild the prompt when it changes, so half-made selections survive polling.
  const sig = JSON.stringify([v.prompt, v.news, v.winners]);
  if (sig !== lastPrompt) {
    lastPrompt = sig; renderPrompt(v.prompt, v.news);
    if (v.winners.length) {
      const won = v.players[v.me] && v.winners.includes(v.players[v.me].name);
      const b = el('div', null, 'victory');
      b.append(el('div', won ? '🏆 You win! 🏆' : '🏆 Victory 🏆', 'big'), el('div', v.winners.join(' & '), 'names'));
      $('prompt').insertBefore(b, $('prompt').children[1]); // right under the heading
    }
  }
  // Updated every poll without rebuilding the prompt.
  const t = $('timer');
  t.textContent = v.timer == null ? '' : `⏱ ${v.timer}s`;
  t.className = v.timer != null && v.timer <= 10 ? 'timer low' : 'timer';
}

function renderPrompt(p, news) {
  const box = $('prompt');
  const h = el('h2', 'Your move ');
  h.append(el('span', '', 'timer'));
  h.lastChild.id = 'timer';
  box.replaceChildren(h);
  // What you just learned, so you don't have to dig through the log.
  if (news.length) { const n = el('div', null, 'news'); n.append(...news.map(s => el('div', '🔒 ' + s))); box.append(n); }
  if (!p) { h.firstChild.textContent = 'Waiting… '; box.append(el('div', 'Nothing for you to do right now.')); return; }
  box.append(el('p', p.text));
  const btn = (text, fn, cls) => { const b = el('button', text, cls); b.onclick = fn; box.append(b); return b; };
  switch (p.kind) {
    case 'start': btn('Start game', () => act({ type: 'Start' }), 'primary'); break;
    case 'next': btn(p.text.startsWith('Game over') ? 'Return to lobby' : 'Next round', () => act({ type: 'Next' }), 'primary'); break;
    case 'pick': {
      const grid = el('div', null, 'choices');
      p.options.forEach((o, i) => {
        if (!p.details[i]) return btn(o, () => act({ type: 'Pick', i }));
        const b = cardFace(el('button', null, 'card'), o, p.details[i]);
        b.onclick = () => act({ type: 'Pick', i });
        grid.append(b);
      });
      if (grid.children.length) box.append(grid);
      break;
    }
    case 'decide': btn('Yes', () => act({ type: 'Decide', yes: true }), 'primary'); btn('No', () => act({ type: 'Decide', yes: false })); break;
    case 'play': {
      // Click card tiles to choose what to play; the button says what will happen.
      const chosen = new Set();
      const go = el('button', 'Pass', 'primary');
      const label = () => { go.textContent = chosen.size ? `Play ${chosen.size} card${chosen.size > 1 ? 's' : ''}` : 'Pass'; };
      const grid = el('div', null, 'choices');
      p.options.forEach((o, i) => {
        const b = cardFace(el('button', null, 'card pick'), o, p.details[i]);
        b.setAttribute('aria-pressed', 'false');
        b.onclick = () => {
          if (chosen.has(i)) chosen.delete(i); else chosen.add(i);
          b.classList.toggle('on', chosen.has(i));
          b.setAttribute('aria-pressed', String(chosen.has(i)));
          label();
        };
        grid.append(b);
      });
      if (p.options.length) box.append(grid);
      if (!p.options.length) box.append(el('p', 'You have no cards for this phase.', 'sub'));
      go.onclick = () => act({ type: 'Play', is: [...chosen] });
      box.append(go);
      break;
    }
    case 'target': {
      if (p.n === 0) { btn('Continue', () => act({ type: 'Target', is: [] }), 'primary'); break; }
      const chosen = [];
      const go = el('button', `Choose ${p.n}`, 'primary'); go.disabled = true;
      p.options.forEach((o, i) => {
        const b = btn(o, () => {
          const at = chosen.indexOf(i);
          if (at >= 0) chosen.splice(at, 1); else if (chosen.length < p.n) chosen.push(i);
          b.classList.toggle('on', chosen.includes(i));
          go.disabled = chosen.length !== p.n;
        });
      });
      box.append(el('br'), go);
      go.onclick = () => act({ type: 'Target', is: chosen });
      break;
    }
  }
}

if (inDiscord) discordJoin(); else poll();
setInterval(poll, 1000);
