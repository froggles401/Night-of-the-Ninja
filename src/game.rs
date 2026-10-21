//! Night of the Ninja rules engine. A pure state machine: `prompt(p)` says what player `p`
//! must decide right now, and `act(p, action)` applies their answer.

use rand::{seq::SliceRandom, thread_rng, Rng};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum House {
    Lotus(u8),
    Crane(u8),
    Ronin,
}

impl House {
    fn side(self) -> Option<usize> {
        match self {
            House::Lotus(_) => Some(0),
            House::Crane(_) => Some(1),
            House::Ronin => None,
        }
    }
    fn rank(self) -> u8 {
        match self {
            House::Lotus(r) | House::Crane(r) => r,
            House::Ronin => u8::MAX,
        }
    }
    fn label(self) -> String {
        match self {
            House::Lotus(r) => format!("Lotus {r}"),
            House::Crane(r) => format!("Crane {r}"),
            House::Ronin => "Ronin".into(),
        }
    }
}

const SIDES: [&str; 2] = ["Lotus", "Crane"];
/// Night phases in resolution order. Mastermind is declared last, at dawn.
const PHASES: [&str; 6] = ["Spy", "Mystic", "Trickster", "Blind Assassin", "Shinobi", "Mastermind"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Spy,
    Mystic,
    Shapeshifter,
    GraveDigger,
    Troublemaker,
    SoulMerchant,
    Thief,
    Judge,
    BlindAssassin,
    Shinobi,
    MirrorMonk,
    Martyr,
    Mastermind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Card {
    kind: Kind,
    num: u8,
}

impl Card {
    /// None for react cards, which are only played in response to a kill.
    fn phase(self) -> Option<usize> {
        use Kind::*;
        match self.kind {
            Spy => Some(0),
            Mystic => Some(1),
            Shapeshifter | GraveDigger | Troublemaker | SoulMerchant | Thief | Judge => Some(2),
            BlindAssassin => Some(3),
            Shinobi => Some(4),
            Mastermind => Some(5),
            MirrorMonk | Martyr => None,
        }
    }
    /// Tricksters show as "Trickster N" (their name goes in the card text), like the printed cards.
    fn label(self) -> String {
        if self.phase() == Some(2) {
            format!("Trickster {}", self.num)
        } else if self.num > 0 {
            format!("{} {}", self.name(), self.num)
        } else {
            self.name().into()
        }
    }
    /// Cards that only make sense with Honor tokens; No Honor mode leaves them out.
    fn uses_honor(self) -> bool {
        matches!(self.kind, Kind::Martyr | Kind::SoulMerchant | Kind::Thief)
    }
    /// The card's rules text, prefixed with the Trickster's name.
    fn detail(self) -> String {
        if self.phase() == Some(2) { format!("{}: {}", self.name(), self.help()) } else { self.help().into() }
    }
    fn name(self) -> &'static str {
        use Kind::*;
        match self.kind {
            Spy => "Spy",
            Mystic => "Mystic",
            Shapeshifter => "Shapeshifter",
            GraveDigger => "Grave Digger",
            Troublemaker => "Troublemaker",
            SoulMerchant => "Soul Merchant",
            Thief => "Thief",
            Judge => "Judge",
            BlindAssassin => "Blind Assassin",
            Shinobi => "Shinobi",
            MirrorMonk => "Mirror Monk",
            Martyr => "Martyr",
            Mastermind => "Mastermind",
        }
    }
    fn help(self) -> &'static str {
        use Kind::*;
        match self.kind {
            Spy => "Look at another player's House card.",
            Mystic => "Look at the House card and one Ninja card of another player.",
            Shapeshifter => "Look at the House cards of two players. You may secretly swap them.",
            GraveDigger => "Look at two discarded Ninja cards and take one.",
            Troublemaker => "Look at another player's House card. You may reveal it.",
            SoulMerchant => "Look at another player's Honor token or House card. You may swap one Honor token with them.",
            Thief => "Your House is revealed. Take an Honor token from a player who has more tokens than you.",
            Judge => "Your House is revealed. Choose a player and KILL them. Reacts don't work on the Judge.",
            BlindAssassin => "Choose a player and KILL them.",
            Shinobi => "Choose a player and look at their House card. You may KILL them.",
            MirrorMonk => "React: reveal this if a Shinobi or Blind Assassin chooses to KILL you. KILL them instead.",
            Martyr => "React: reveal this if a Shinobi or Blind Assassin chooses to KILL you. Gain an Honor token.",
            Mastermind => "If you're still alive at the Reveal, your House wins the round. (If you're the Ronin, no House wins.)",
        }
    }
}

/// One entry per card type, for the help menu's card list.
#[derive(Serialize)]
pub struct CardInfo {
    name: String,
    phase: &'static str,
    /// Card numbers in the deck (0 = unnumbered); its length is how many copies there are.
    numbers: Vec<u8>,
    text: String,
    /// Removed in No Honor mode.
    honor: bool,
}

pub fn card_list() -> Vec<CardInfo> {
    let mut deck = new_deck(false);
    // Night order, with the reacts slotted in before the Mastermind (which is revealed at dawn).
    deck.sort_by_key(|c| (c.phase().map_or(9, |ph| ph * 2), c.num));
    let mut out: Vec<(Kind, CardInfo)> = vec![];
    for c in deck {
        match out.iter_mut().find(|(k, _)| *k == c.kind) {
            Some((_, info)) => info.numbers.push(c.num),
            None => out.push((
                c.kind,
                CardInfo {
                    name: if c.phase() == Some(2) { c.label() } else { c.name().into() },
                    phase: c.phase().map_or("React", |ph| PHASES[ph]),
                    numbers: vec![c.num],
                    text: c.detail(),
                    honor: c.uses_honor(),
                },
            )),
        }
    }
    out.into_iter().map(|(_, info)| info).collect()
}

fn new_deck(no_honor: bool) -> Vec<Card> {
    let mut d = full_deck();
    d.retain(|c| !(no_honor && c.uses_honor()));
    d
}

fn full_deck() -> Vec<Card> {
    use Kind::*;
    // The box's 33 Ninja cards: 6 each of Spy, Mystic, Blind Assassin and Shinobi, 6 Tricksters, and 3 one-off cards.
    let mut d = vec![];
    for num in 1..=6 {
        for kind in [Spy, Mystic, BlindAssassin, Shinobi] {
            d.push(Card { kind, num });
        }
    }
    for (i, kind) in [Shapeshifter, GraveDigger, Troublemaker, SoulMerchant, Thief, Judge].into_iter().enumerate() {
        d.push(Card { kind, num: i as u8 + 1 });
    }
    for kind in [MirrorMonk, Martyr, Mastermind] {
        d.push(Card { kind, num: 0 });
    }
    d
}

fn token_pile() -> Vec<u8> {
    // shortcut: 35 tokens worth 2/3/4, but the exact split isn't in the rulebook; this is a guess.
    let mut t = [vec![2; 15], vec![3; 12], vec![4; 8]].concat();
    t.shuffle(&mut thread_rng());
    t
}

/// Which side wins among survivors: lowest rank wins, ties fall to the next survivor.
fn winner(survivors: &[House]) -> Option<usize> {
    let ranks = |side| {
        let mut v: Vec<u8> = survivors.iter().filter(|h| h.side() == Some(side)).map(|h| h.rank()).collect();
        v.sort();
        v
    };
    let (lotus, crane) = (ranks(0), ranks(1));
    for i in 0.. {
        match (lotus.get(i), crane.get(i)) {
            (None, None) => return None,
            (Some(_), None) => return Some(0),
            (None, Some(_)) => return Some(1),
            (Some(a), Some(b)) if a < b => return Some(0),
            (Some(a), Some(b)) if a > b => return Some(1),
            _ => {}
        }
    }
    unreachable!()
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum Action {
    Start,
    Next,
    Pick { i: usize },
    Play { is: Vec<usize> },
    Target { is: Vec<usize> },
    Decide { yes: bool },
    // Host controls. The server handles these itself because they change who sits at the table.
    AddBot,
    Kick { who: usize },
    MakeHost { who: usize },
    SetNoHonor { on: bool },
    /// Seconds per decision; 0 turns the timer off.
    SetTimer { secs: u64 },
}

/// Pause before each bot step while cards resolve. Zero in tests so games run instantly.
const BOT_PACE: Duration = if cfg!(test) { Duration::ZERO } else { Duration::from_millis(1600) };

const BOT_NAMES: [&str; 10] = ["Hanzo", "Kasumi", "Goemon", "Sasuke", "Ayame", "Jubei", "Kotaro", "Momochi", "Fuma", "Shiro"];

/// What the client should ask. `kind` picks the widget; `n` is how many targets to choose.
#[derive(Serialize, Clone, Debug)]
pub struct Prompt {
    kind: &'static str,
    text: String,
    options: Vec<String>,
    /// Card text shown under each option; empty when the options aren't cards.
    details: Vec<String>,
    n: usize,
}

#[derive(Serialize)]
pub struct CardView {
    name: String,
    text: String,
}

impl From<Card> for CardView {
    fn from(c: Card) -> Self {
        CardView { name: c.label(), text: c.detail() }
    }
}

#[derive(Serialize)]
pub struct PlayView {
    seat: usize,
    card: CardView,
    /// "done", "now", "next", or "skipped" (its owner died first).
    state: &'static str,
}

/// The card being resolved right now: who plays it and whom it's aimed at (all public).
#[derive(Serialize)]
pub struct Acting {
    seat: usize,
    card: CardView,
    targets: Vec<usize>,
}

#[derive(Clone)]
enum Follow {
    Shinobi(usize),
    Swap(usize, usize),
    GravePick(Vec<Card>),
    GraveNow(Card),
    Reveal(usize),
    MerchantChoose(usize),
    MerchantSwap(usize, usize),
}

#[derive(Clone)]
enum Stage {
    Lobby,
    Draft,
    Declare(usize),
    /// The card at the front of `queue` is waiting for its owner to choose targets.
    Act,
    /// ...and then for a follow-up decision.
    Follow(Follow),
    React { victim: usize, attacker: usize },
    RoundOver,
    GameOver,
}

struct Player {
    name: String,
    house: House,
    /// What this player thinks their House is; a Shapeshifter swap doesn't tell them.
    belief: House,
    hand: Vec<Card>,
    tokens: Vec<u8>,
    alive: bool,
    revealed: bool,
    bot: bool,
    /// Log length when this player last made a move; their private log lines after it are "news".
    acted_at: usize,
    /// Houses this player has privately seen this round, by seat. Bots use it to pick targets.
    seen: Vec<Option<House>>,
    /// Ninja cards this player has turned face up this round (played or revealed as a react).
    played: Vec<Card>,
    /// The Ninja card this player last saw in each seat's hand with a Mystic.
    peeked: Vec<Option<Card>>,
    /// What killed this player this round, e.g. "Goemon's Blind Assassin 2".
    killed_by: Option<String>,
    /// Profile picture URL (Discord players only).
    avatar: Option<String>,
}

pub struct Game {
    players: Vec<Player>,
    host: usize,
    stage: Stage,
    phase: usize,
    draft: Vec<Vec<Card>>,
    picks: Vec<Option<usize>>,
    second_pick: bool,
    declared: Vec<Option<Vec<usize>>>,
    queue: VecDeque<(usize, Card)>,
    discard: Vec<Card>,
    pool: Vec<u8>,
    mastermind: Option<usize>,
    /// (None = everyone, Some(p) = only p can see it, text)
    log: Vec<(Option<usize>, String)>,
    winners: Vec<usize>,
    /// No Honor mode: no tokens, no Martyr/Soul Merchant/Thief, and each round is a whole game.
    no_honor: bool,
    /// Bumped on every stage change; lets `tick` tell when a timeout moved the game on.
    step: u64,
    deadline: Option<Instant>,
    /// When the next bot move during card resolution is due, so people can follow each step.
    bot_due: Option<Instant>,
    /// Every card played this phase, in resolution order, and those skipped because their owner had died.
    plays: Vec<(usize, Card)>,
    /// Which phase `plays` belongs to; None until the first cards of the round are revealed.
    plays_phase: Option<usize>,
    skipped: Vec<(usize, Card)>,
    pace: Duration,
    turn_secs: u64,
}

#[derive(Serialize)]
pub struct PlayerView {
    name: String,
    bot: bool,
    alive: bool,
    tokens: usize,
    /// Ninja cards in hand. Public, like a hand of cards on the table.
    cards: usize,
    /// Public once revealed; otherwise what the viewer saw with their own peeks (see `private`).
    house: Option<String>,
    /// True when `house` is known only to the viewer (and may be stale after a Shapeshifter swap).
    private: bool,
    /// Face-up cards everyone can see.
    played: Vec<CardView>,
    /// A card in their hand that only the viewer knows about (from a Mystic).
    peeked: Option<CardView>,
    /// What killed them, once dead.
    death: Option<String>,
    avatar: Option<String>,
}

#[derive(Serialize)]
pub struct View {
    me: usize,
    host: usize,
    lobby: bool,
    no_honor: bool,
    /// Most players allowed: [normal, No Honor mode].
    caps: [usize; 2],
    /// Seconds left to decide; None when there's no clock running.
    timer: Option<u64>,
    turn_secs: u64,
    status: String,
    house: String,
    /// Your House card as you know it, for your own player card (None in the lobby).
    belief: Option<String>,
    hand: Vec<CardView>,
    tokens: Vec<u8>,
    players: Vec<PlayerView>,
    prompt: Option<Prompt>,
    log: Vec<String>,
    winners: Vec<String>,
    /// What only you learned since your last move (peeks, stolen or drawn tokens...).
    news: Vec<String>,
    /// Step for the phase bar: 0 = draft, 1..=PHASES.len() = Night phases, then dawn; None in the lobby.
    track: Option<usize>,
    /// The latest phase's cards in resolution order, with who played them.
    plays: Vec<PlayView>,
    /// Which phase (index into PHASES) `plays` belongs to; None before the first reveal of the round.
    plays_phase: Option<usize>,
    acting: Option<Acting>,
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

impl Game {
    pub fn new() -> Self {
        Game {
            players: vec![],
            host: 0,
            stage: Stage::Lobby,
            phase: 0,
            draft: vec![],
            picks: vec![],
            second_pick: false,
            declared: vec![],
            queue: VecDeque::new(),
            discard: vec![],
            pool: vec![],
            mastermind: None,
            log: vec![],
            winners: vec![],
            no_honor: true, // the default game; Honor mode is the opt-in variant
            step: 0,
            deadline: None,
            bot_due: None,
            plays: vec![],
            plays_phase: None,
            skipped: vec![],
            pace: BOT_PACE,
            turn_secs: 60,
        }
    }

    pub fn join(&mut self, name: String) -> Result<usize, String> {
        if !matches!(self.stage, Stage::Lobby) {
            return Err("The game has already started.".into());
        }
        if self.players.len() >= self.max_players() {
            return Err(format!("The game is full ({} players).", self.max_players()));
        }
        self.say(format!("{name} joined."));
        self.players.push(Player {
            name,
            house: House::Ronin,
            belief: House::Ronin,
            hand: vec![],
            tokens: vec![],
            alive: true,
            revealed: false,
            bot: false,
            acted_at: 0,
            seen: vec![],
            played: vec![],
            peeked: vec![],
            killed_by: None,
            avatar: None,
        });
        Ok(self.players.len() - 1)
    }

    fn check_host(&self, by: usize) -> Result<(), String> {
        if by == self.host { Ok(()) } else { Err("Only the host can do that.".into()) }
    }

    /// Everyone is dealt 3 Ninja cards, so the deck caps the table: 11 normally, 9 in No Honor mode.
    fn max_players(&self) -> usize {
        new_deck(self.no_honor).len() / 3
    }

    pub fn set_no_honor(&mut self, by: usize, on: bool) -> Result<(), String> {
        self.check_host(by)?;
        if !matches!(self.stage, Stage::Lobby) {
            return Err("The mode can only be changed in the lobby.".into());
        }
        if on && self.players.len() > new_deck(true).len() / 3 {
            return Err(format!("Without Honor mode the game seats at most {} players. Remove some first.", new_deck(true).len() / 3));
        }
        self.no_honor = on;
        self.say(if on { "Honor mode is off: each round is a whole game.".into() } else { "Honor mode is ON: play to 10 points with Honor tokens.".into() });
        Ok(())
    }

    pub fn add_bot(&mut self, by: usize) -> Result<(), String> {
        self.check_host(by)?;
        let name = BOT_NAMES
            .iter()
            .map(|n| format!("🤖 {n}"))
            .find(|n| self.players.iter().all(|p| &p.name != n))
            .ok_or("No more bots available.")?;
        self.join(name)?;
        self.players.last_mut().unwrap().bot = true;
        Ok(())
    }

    /// Lobby only: removing a seat mid-round would break the deal.
    pub fn kick(&mut self, by: usize, who: usize) -> Result<(), String> {
        self.check_host(by)?;
        if !matches!(self.stage, Stage::Lobby) {
            return Err("Players can only be removed in the lobby.".into());
        }
        if who == by || who >= self.players.len() {
            return Err("You can't remove that player.".into());
        }
        let gone = self.players.remove(who);
        if who < self.host {
            self.host -= 1;
        }
        self.say(format!("{} was removed by the host.", gone.name));
        Ok(())
    }

    /// In the lobby the seat is removed; mid-game a bot takes it over so the round can finish.
    /// Returns true if the seat was removed (the server must drop it too).
    pub fn leave(&mut self, p: usize) -> bool {
        let lobby = matches!(self.stage, Stage::Lobby);
        if lobby {
            let gone = self.players.remove(p);
            self.say(format!("{} left.", gone.name));
            if p < self.host {
                self.host -= 1;
            }
        } else {
            self.players[p].bot = true;
            self.players[p].name.push_str(" (left)");
            self.say(format!("{} left; a bot plays their seat.", self.name(p)));
        }
        // The host must be a human still at the table.
        if (lobby && p == self.host) || self.players.get(self.host).is_some_and(|h| h.bot) {
            if let Some(h) = (0..self.players.len()).find(|&q| !self.players[q].bot) {
                self.host = h;
                self.say(format!("{} is now the host.", self.name(h)));
            }
        }
        self.host = self.host.min(self.players.len().saturating_sub(1));
        if !lobby {
            self.run_bots();
        }
        lobby
    }

    pub fn humans(&self) -> usize {
        self.players.iter().filter(|pl| !pl.bot).count()
    }

    pub fn make_host(&mut self, by: usize, who: usize) -> Result<(), String> {
        self.check_host(by)?;
        if who == by || who >= self.players.len() || self.players[who].bot {
            return Err("Pick another human player.".into());
        }
        self.host = who;
        self.say(format!("{} is now the host.", self.name(who)));
        Ok(())
    }

    /// Bots act on what they know: public reveals plus their own peeks. Anything else is a coin flip.
    fn bot_action(&self, p: usize) -> Option<Action> {
        let pr = self.prompt(p)?;
        let mut rng = thread_rng();
        let k = pr.options.len();
        Some(match pr.kind {
            // Option 0 is always a react card; "No reaction" is last.
            "pick" if matches!(self.stage, Stage::React { .. }) => Action::Pick { i: 0 },
            "pick" => Action::Pick { i: rng.gen_range(0..k) },
            "play" => Action::Play { is: (0..k).collect() },
            "target" => {
                let (ts, n) = self.targets();
                // Lower rank = better target; ties are broken at random by the shuffle.
                let rank = |t: usize| match self.queue[0].1.kind {
                    Kind::BlindAssassin | Kind::Judge | Kind::Shinobi => self.threat(p, t),
                    Kind::Spy | Kind::Mystic | Kind::Troublemaker | Kind::SoulMerchant => {
                        self.known_side(p, t).is_some() as u8
                    }
                    _ => 0,
                };
                let mut is: Vec<usize> = (0..ts.len()).collect();
                is.shuffle(&mut rng);
                is.sort_by_key(|&i| rank(ts[i]));
                is.truncate(n);
                Action::Target { is }
            }
            "decide" => Action::Decide {
                yes: match &self.stage {
                    // Kill or expose only known enemies.
                    Stage::Follow(Follow::Shinobi(t)) | Stage::Follow(Follow::Reveal(t)) => self.threat(p, *t) == 0,
                    Stage::Follow(Follow::GraveNow(_)) => true,
                    _ => rng.gen(),
                },
            },
            _ => return None,
        })
    }

    fn next_bot_move(&self) -> Option<(usize, Action)> {
        (0..self.players.len()).filter(|&p| self.players[p].bot).find_map(|p| self.bot_action(p).map(|a| (p, a)))
    }

    fn bot_move(&mut self, p: usize, a: Action) -> bool {
        let ok = self.apply(p, a);
        if let Err(e) = &ok {
            eprintln!("bot {} made an illegal move: {e}", self.name(p));
        }
        ok.is_ok()
    }

    /// Bots draft and declare instantly, but resolve their cards one paced step at a time (see `tick`).
    fn run_bots(&mut self) {
        while let Some((p, a)) = self.next_bot_move() {
            let resolving = matches!(self.stage, Stage::Act | Stage::Follow(_) | Stage::React { .. });
            if resolving && !self.pace.is_zero() {
                self.bot_due.get_or_insert(Instant::now() + self.pace);
                return;
            }
            if !self.bot_move(p, a) {
                break;
            }
        }
    }

    fn name(&self, p: usize) -> &str {
        &self.players[p].name
    }
    fn say(&mut self, s: String) {
        self.log.push((None, s));
    }
    fn tell(&mut self, p: usize, s: String) {
        self.log.push((Some(p), s));
    }
    fn draw(&mut self) -> u8 {
        if self.pool.is_empty() {
            // shortcut: the box has a fixed 35 tokens; we reshuffle a fresh pile instead of running dry.
            self.pool = token_pile();
        }
        self.pool.pop().unwrap()
    }
    fn playable(&self, p: usize, ph: usize) -> Vec<usize> {
        (0..self.players[p].hand.len()).filter(|&i| self.players[p].hand[i].phase() == Some(ph)).collect()
    }
    fn reacts(&self, p: usize) -> Vec<usize> {
        (0..self.players[p].hand.len()).filter(|&i| self.players[p].hand[i].phase().is_none()).collect()
    }

    /// Legal targets for the card at the front of the queue, and how many to choose.
    fn targets(&self) -> (Vec<usize>, usize) {
        let (p, card) = self.queue[0];
        let all = 0..self.players.len();
        let (ts, n): (Vec<usize>, usize) = match card.kind {
            Kind::Shapeshifter => (all.collect(), 2),
            Kind::GraveDigger => (vec![], 0),
            Kind::Thief => {
                let mine = self.players[p].tokens.len();
                (all.filter(|&q| self.players[q].tokens.len() > mine).collect(), 1)
            }
            Kind::BlindAssassin | Kind::Shinobi | Kind::Judge => {
                (all.filter(|&q| q != p && self.players[q].alive).collect(), 1)
            }
            _ => (all.filter(|&q| q != p).collect(), 1),
        };
        let n = n.min(ts.len());
        (ts, n)
    }

    pub fn prompt(&self, p: usize) -> Option<Prompt> {
        let me = &self.players[p];
        let actor = self.queue.front().map(|&(q, _)| q);
        let mk = |kind, text: String, options: Vec<String>, n| Some(Prompt { kind, text, options, details: vec![], n });
        let cards = |kind, text: String, cs: Vec<Card>, n| {
            let details = cs.iter().map(|c| c.detail()).collect();
            Some(Prompt { kind, text, options: cs.iter().map(|c| c.label()).collect(), details, n })
        };
        let held = |is: Vec<usize>| is.iter().map(|&i| me.hand[i]).collect::<Vec<_>>();
        let yes_no = || vec!["Yes".to_string(), "No".to_string()];
        match &self.stage {
            Stage::Lobby if p == self.host => mk(
                "start",
                format!("{} players joined. Start once 4-{} are in.", self.players.len(), self.max_players()),
                vec![],
                0,
            ),
            Stage::Draft if self.picks[p].is_none() => {
                cards("pick", "Draft: choose a Ninja card to keep.".into(), self.draft[p].clone(), 1)
            }
            Stage::Declare(ph) if me.alive && self.declared[p].is_none() => cards(
                "play",
                format!(
                    "{} phase: play any of your {} cards? Everyone answers, so passing gives nothing away.",
                    PHASES[*ph], PHASES[*ph]
                ),
                held(self.playable(p, *ph)),
                0,
            ),
            Stage::Act if actor == Some(p) => {
                let card = self.queue[0].1;
                let (ts, n) = self.targets();
                let text = format!("{} — {}", card.label(), card.detail());
                mk("target", text, ts.iter().map(|&t| self.name(t).to_string()).collect(), n)
            }
            Stage::Follow(f) if actor == Some(p) => match f {
                Follow::Shinobi(t) => mk("decide", format!("KILL {}?", self.name(*t)), yes_no(), 0),
                Follow::Swap(a, b) => mk(
                    "decide",
                    format!("Swap the House cards of {} and {}?", self.name(*a), self.name(*b)),
                    yes_no(),
                    0,
                ),
                Follow::GravePick(cs) => cards("pick", "Take which card?".into(), cs.clone(), 1),
                Follow::GraveNow(c) => mk(
                    "decide",
                    format!("Play {} right now? ({}) No keeps it for later.", c.label(), c.detail()),
                    yes_no(),
                    0,
                ),
                Follow::Reveal(t) => {
                    mk("decide", format!("Reveal {}'s House card to everyone?", self.name(*t)), yes_no(), 0)
                }
                Follow::MerchantChoose(t) => mk(
                    "pick",
                    format!("Look at {}'s...", self.name(*t)),
                    vec!["House card".into(), "An Honor token".into()],
                    1,
                ),
                Follow::MerchantSwap(..) => {
                    mk("decide", "Swap that token for your lowest token?".into(), yes_no(), 0)
                }
            },
            Stage::React { victim, attacker } if *victim == p => {
                let text = format!("{} is killing you! Reveal a react card?", self.name(*attacker));
                cards("pick", text, held(self.reacts(p)), 1).map(|mut pr| {
                    pr.options.push("No reaction".into());
                    pr.details.push(String::new());
                    pr
                })
            }
            Stage::RoundOver if p == self.host => mk("next", "Round over.".into(), vec![], 0),
            Stage::GameOver if p == self.host => {
                mk("next", "Game over. Return to the lobby to change settings or players, then start again.".into(), vec![], 0)
            }
            _ => None,
        }
    }

    /// A human's move, followed by every bot move it unblocks.
    pub fn act(&mut self, p: usize, a: Action) -> Result<(), String> {
        self.apply(p, a)?;
        self.run_bots();
        Ok(())
    }

    fn apply(&mut self, p: usize, a: Action) -> Result<(), String> {
        let pr = self.prompt(p).ok_or("Nothing for you to do right now.")?;
        let k = pr.options.len();
        // Marked before the move resolves, so what this move reveals counts as news.
        let mark = self.log.len();
        match (self.stage.clone(), a) {
            (Stage::Lobby, Action::Start) => {
                if !(4..=self.max_players()).contains(&self.players.len()) {
                    return Err(format!("Need 4-{} players.", self.max_players()));
                }
                self.new_game();
            }
            (Stage::RoundOver, Action::Next) => self.start_round(),
            (Stage::GameOver, Action::Next) => self.return_to_lobby(),
            (Stage::Declare(ph), Action::Play { is }) => {
                if is.iter().any(|&i| i >= k) {
                    return Err("No such card.".into());
                }
                let opts = self.playable(p, ph);
                let mut chosen: Vec<usize> = is.iter().map(|&i| opts[i]).collect();
                chosen.sort();
                chosen.dedup();
                self.declared[p] = Some(chosen);
                self.close_declare();
            }
            (Stage::Act, Action::Target { is }) => {
                let (ts, n) = self.targets();
                let mut sorted = is.clone();
                sorted.sort();
                sorted.dedup();
                if is.len() != n || sorted.len() != n || is.iter().any(|&i| i >= ts.len()) {
                    return Err(format!("Choose exactly {n} different player(s)."));
                }
                self.resolve(is.iter().map(|&i| ts[i]).collect());
            }
            (_, Action::Pick { i }) if pr.kind == "pick" && i < k => self.pick(p, i),
            (Stage::Follow(f), Action::Decide { yes }) if pr.kind == "decide" => self.decide(f, yes),
            _ => return Err("That move doesn't fit the current prompt.".into()),
        }
        self.players[p].acted_at = mark;
        Ok(())
    }

    /// Back to the lobby with the same seats, cleared of everything from the last game.
    fn return_to_lobby(&mut self) {
        for pl in &mut self.players {
            pl.tokens.clear();
            pl.hand.clear();
            pl.played.clear();
            (pl.alive, pl.revealed, pl.killed_by) = (true, false, None);
        }
        self.winners.clear();
        self.queue.clear();
        self.plays.clear();
        self.plays_phase = None;
        self.say("Back in the lobby.".into());
        self.set_stage(Stage::Lobby);
    }

    fn new_game(&mut self) {
        for pl in &mut self.players {
            pl.tokens.clear();
        }
        self.pool = token_pile();
        self.winners.clear();
        self.start_round();
    }

    fn start_round(&mut self) {
        let n = self.players.len();
        let mut rng = thread_rng();
        let mut houses: Vec<House> = (1..=(n / 2) as u8).flat_map(|r| [House::Lotus(r), House::Crane(r)]).collect();
        if n % 2 == 1 {
            houses.push(House::Ronin);
        }
        houses.shuffle(&mut rng);
        let mut deck = new_deck(self.no_honor);
        deck.shuffle(&mut rng);
        self.draft = (0..n).map(|_| deck.split_off(deck.len() - 3)).collect();
        self.picks = vec![None; n];
        self.second_pick = false;
        self.discard.clear();
        self.queue.clear();
        self.plays.clear();
        self.plays_phase = None;
        self.skipped.clear();
        self.mastermind = None;
        self.set_stage(Stage::Draft);
        self.say("━━━ A new round begins ━━━".into());
        for (p, h) in houses.into_iter().enumerate() {
            let pl = &mut self.players[p];
            (pl.house, pl.belief, pl.alive, pl.revealed) = (h, h, true, false);
            pl.hand.clear();
            pl.seen = vec![None; n];
            pl.played.clear();
            pl.peeked = vec![None; n];
            pl.killed_by = None;
            self.tell(p, format!("You are {}.", h.label()));
        }
    }

    fn close_draft(&mut self) {
        let n = self.players.len();
        let mut passed = vec![Vec::new(); n];
        for q in 0..n {
            let mut h = std::mem::take(&mut self.draft[q]);
            let c = h.remove(self.picks[q].take().unwrap());
            self.players[q].hand.push(c);
            if self.second_pick {
                self.discard.extend(h);
            } else {
                passed[(q + 1) % n] = h;
            }
        }
        if self.second_pick {
            self.say("Night falls.".into());
            self.begin_declare(0);
        } else {
            self.draft = passed;
            self.second_pick = true;
            self.set_stage(Stage::Draft); // restarts the clock for the second pick
        }
    }

    fn begin_declare(&mut self, ph: usize) {
        if ph == PHASES.len() {
            return self.end_round();
        }
        self.phase = ph;
        self.declared = self.players.iter().map(|pl| (!pl.alive).then(Vec::new)).collect();
        self.set_stage(Stage::Declare(ph));
        self.close_declare();
    }

    fn close_declare(&mut self) {
        if self.declared.iter().any(Option::is_none) {
            return;
        }
        let mut played = vec![];
        for p in 0..self.players.len() {
            for i in self.declared[p].take().unwrap().into_iter().rev() {
                let c = self.players[p].hand.remove(i);
                self.players[p].played.push(c);
                played.push((p, c));
            }
        }
        played.sort_by_key(|&(_, c)| c.num);
        if played.is_empty() && self.phase < 5 {
            self.say(format!("Nobody plays a {}.", PHASES[self.phase]));
        }
        for &(p, c) in &played {
            self.say(format!("{} plays {}.", self.name(p), c.label()));
        }
        // Replaces the previous phase's list only now, so people can see how that phase ended.
        self.plays = played.clone();
        self.plays_phase = Some(self.phase);
        self.skipped.clear();
        self.queue = played.into();
        self.advance();
    }

    /// Move to the next card that needs input, skipping dead owners; next phase when the queue is empty.
    fn advance(&mut self) {
        while let Some(&(p, card)) = self.queue.front() {
            if !self.players[p].alive {
                self.say(format!("{} is dead, so their {} does nothing.", self.name(p), card.label()));
                self.queue.pop_front();
                self.discard.push(card);
                self.skipped.push((p, card));
            } else if card.kind == Kind::Mastermind {
                self.mastermind = Some(p);
                self.queue.pop_front();
            } else {
                self.set_stage(Stage::Act);
                return;
            }
        }
        self.begin_declare(self.phase + 1);
    }

    /// Every stage change restarts the clock for whoever must decide next.
    fn set_stage(&mut self, s: Stage) {
        self.stage = s;
        self.step += 1;
        let secs = match self.stage {
            Stage::Draft | Stage::Declare(_) | Stage::Act | Stage::Follow(_) | Stage::React { .. } => self.turn_secs,
            Stage::Lobby | Stage::RoundOver | Stage::GameOver => 0,
        };
        self.deadline = (secs > 0).then(|| Instant::now() + Duration::from_secs(secs));
    }

    pub fn set_timer(&mut self, by: usize, secs: u64) -> Result<(), String> {
        self.check_host(by)?;
        if !matches!(self.stage, Stage::Lobby) {
            return Err("The timer can only be changed in the lobby.".into());
        }
        if secs > 600 {
            return Err("The timer can be at most 10 minutes.".into());
        }
        self.turn_secs = secs;
        Ok(())
    }

    /// The move made for a player whose time ran out: pass, say no, or pick at random when a choice is required.
    fn timeout_action(&self, p: usize) -> Option<Action> {
        match self.prompt(p)?.kind {
            "play" => Some(Action::Play { is: vec![] }),
            "decide" => Some(Action::Decide { yes: false }),
            _ => self.bot_action(p),
        }
    }

    /// Called often (on every poll); makes default moves for everyone still deciding once the deadline passes.
    pub fn tick(&mut self, now: Instant) {
        if self.bot_due.is_some_and(|d| now >= d) {
            self.bot_due = None;
            if let Some((p, a)) = self.next_bot_move() {
                if self.bot_move(p, a) {
                    self.run_bots();
                }
            }
        }
        if self.deadline.is_none_or(|d| now < d) {
            return;
        }
        let step = self.step;
        self.deadline = None;
        self.say("⏱ Time's up!".into());
        // Stop once the game moves on, so the next player gets their full time.
        for p in 0..self.players.len() {
            if self.step != step {
                break;
            }
            if let Some(a) = self.timeout_action(p) {
                if self.apply(p, a).is_ok() {
                    self.tell(p, "⏱ You ran out of time, so a default move was made for you.".into());
                }
            }
        }
        self.run_bots();
    }

    fn finish(&mut self) {
        if let Some((_, c)) = self.queue.pop_front() {
            self.discard.push(c);
        }
        self.advance();
    }

    fn learn(&mut self, p: usize, t: usize) {
        self.players[p].seen[t] = Some(self.players[t].house);
    }

    pub fn set_avatar(&mut self, p: usize, url: Option<String>) {
        self.players[p].avatar = url;
    }

    /// Which side `p` believes `t` is on: from a public reveal, their own peeks, or (for themself) their House card.
    /// None = unknown; Some(None) = the Ronin.
    fn known_side(&self, p: usize, t: usize) -> Option<Option<usize>> {
        let target = &self.players[t];
        if t == p {
            Some(target.belief.side())
        } else if target.revealed {
            Some(target.house.side())
        } else {
            self.players[p].seen[t].map(|h| h.side())
        }
    }

    /// 0 = known enemy, 1 = unknown, 2 = known ally. The Ronin has no allies.
    fn threat(&self, p: usize, t: usize) -> u8 {
        let mine = self.players[p].belief.side();
        match self.known_side(p, t) {
            None => 1,
            Some(s) if mine.is_some() && s == mine => 2,
            Some(_) => 0,
        }
    }

    fn reveal(&mut self, p: usize) {
        let pl = &mut self.players[p];
        pl.revealed = true;
        pl.belief = pl.house;
        self.say(format!("{}'s House is revealed: {}.", self.name(p), self.players[p].house.label()));
    }

    /// `by` used `card` to kill `v`; both go on v's player card so deaths never look random.
    fn kill(&mut self, v: usize, by: usize, card: Card) {
        // Tricksters read better by name ("Judge") than by number.
        let what = if card.phase() == Some(2) { card.name().to_string() } else { card.label() };
        let cause = format!("{}'s {what}", self.name(by));
        self.say(format!("☠ {} is killed by {cause}.", self.name(v)));
        self.players[v].alive = false;
        self.players[v].killed_by = Some(cause);
    }

    fn strike(&mut self, a: usize, v: usize) {
        self.say(format!("{} attacks {}!", self.name(a), self.name(v)));
        if self.reacts(v).is_empty() {
            self.kill(v, a, self.queue[0].1);
            self.finish();
        } else {
            self.set_stage(Stage::React { victim: v, attacker: a });
        }
    }

    fn resolve(&mut self, ts: Vec<usize>) {
        let (p, card) = self.queue[0];
        let mut rng = thread_rng();
        if ts.is_empty() && !matches!(card.kind, Kind::GraveDigger | Kind::Thief | Kind::Judge) {
            return self.finish();
        }
        let t = ts.first().copied();
        let house = |g: &Game, q: usize| format!("{}'s House card is {}.", g.name(q), g.players[q].house.label());
        match card.kind {
            Kind::Spy => {
                let t = t.unwrap();
                self.say(format!("{} spies on {}.", self.name(p), self.name(t)));
                self.tell(p, house(self, t));
                self.learn(p, t);
                self.finish();
            }
            Kind::Mystic => {
                let t = t.unwrap();
                self.say(format!("{} reads {}'s fate.", self.name(p), self.name(t)));
                let card = self.players[t].hand.choose(&mut rng).copied();
                self.players[p].peeked[t] = card;
                let peek = card.map_or("no Ninja cards".into(), |c| c.label());
                self.tell(p, format!("{} They hold: {peek}.", house(self, t)));
                self.learn(p, t);
                self.finish();
            }
            Kind::Shapeshifter => {
                let (a, b) = (ts[0], ts[1]);
                self.say(format!("{} looks at {}'s and {}'s House cards.", self.name(p), self.name(a), self.name(b)));
                self.tell(p, format!("{} {}", house(self, a), house(self, b)));
                self.learn(p, a);
                self.learn(p, b);
                self.set_stage(Stage::Follow(Follow::Swap(a, b)));
            }
            Kind::GraveDigger => {
                let mut cs = self.discard.clone();
                cs.shuffle(&mut rng);
                cs.truncate(2);
                if cs.is_empty() {
                    self.say(format!("{} digs, but the discard pile is empty.", self.name(p)));
                    self.finish();
                } else {
                    self.say(format!("{} digs through the discards.", self.name(p)));
                    self.set_stage(Stage::Follow(Follow::GravePick(cs)));
                }
            }
            Kind::Troublemaker => {
                let t = t.unwrap();
                self.say(format!("{} snoops on {}.", self.name(p), self.name(t)));
                self.tell(p, house(self, t));
                self.learn(p, t);
                self.set_stage(Stage::Follow(Follow::Reveal(t)));
            }
            Kind::SoulMerchant => {
                let t = t.unwrap();
                self.say(format!("{} bargains with {}.", self.name(p), self.name(t)));
                self.set_stage(Stage::Follow(Follow::MerchantChoose(t)));
            }
            Kind::Thief => {
                self.reveal(p);
                if let Some(t) = t {
                    let i = rng.gen_range(0..self.players[t].tokens.len());
                    let v = self.players[t].tokens.remove(i);
                    self.players[p].tokens.push(v);
                    self.say(format!("{} steals an Honor token from {}.", self.name(p), self.name(t)));
                    self.tell(p, format!("You stole a {v}-point token."));
                    self.tell(t, format!("You lost a {v}-point token."));
                }
                self.finish();
            }
            Kind::Judge => {
                self.reveal(p);
                if let Some(t) = t {
                    self.say(format!("The Judge sentences {}.", self.name(t)));
                    self.kill(t, p, card);
                }
                self.finish();
            }
            Kind::BlindAssassin => self.strike(p, t.unwrap()),
            Kind::Shinobi => {
                let t = t.unwrap();
                self.say(format!("{} stalks {}.", self.name(p), self.name(t)));
                self.tell(p, house(self, t));
                self.learn(p, t);
                self.set_stage(Stage::Follow(Follow::Shinobi(t)));
            }
            Kind::MirrorMonk | Kind::Martyr | Kind::Mastermind => self.finish(),
        }
    }

    fn pick(&mut self, p: usize, i: usize) {
        match self.stage.clone() {
            Stage::Draft => {
                self.picks[p] = Some(i);
                if self.picks.iter().all(Option::is_some) {
                    self.close_draft();
                }
            }
            Stage::React { victim, attacker } => {
                match self.reacts(victim).get(i) {
                    None => self.kill(victim, attacker, self.queue[0].1),
                    Some(&h) => {
                        let c = self.players[victim].hand.remove(h);
                        self.players[victim].played.push(c);
                        self.discard.push(c);
                        if c.kind == Kind::MirrorMonk {
                            self.say(format!(
                                "{} reveals a Mirror Monk! The blow turns back on {}.",
                                self.name(victim),
                                self.name(attacker)
                            ));
                            self.kill(attacker, victim, c);
                        } else {
                            self.say(format!("{} reveals a Martyr and dies with honor.", self.name(victim)));
                            let v = self.draw();
                            self.players[victim].tokens.push(v);
                            self.tell(victim, format!("You drew a {v}-point Honor token."));
                            self.kill(victim, attacker, self.queue[0].1);
                        }
                    }
                }
                self.finish();
            }
            Stage::Follow(Follow::GravePick(cs)) => {
                let c = cs[i];
                let pos = self.discard.iter().position(|&d| d == c).unwrap();
                self.discard.remove(pos);
                self.tell(p, format!("You take {}.", c.label()));
                if c.phase().is_some_and(|ph| ph < 5) {
                    self.set_stage(Stage::Follow(Follow::GraveNow(c)));
                } else {
                    self.players[p].hand.push(c);
                    self.finish();
                }
            }
            Stage::Follow(Follow::MerchantChoose(t)) => {
                if i == 0 {
                    let s = format!("{}'s House card is {}.", self.name(t), self.players[t].house.label());
                    self.tell(p, s);
                    self.learn(p, t);
                    self.finish();
                } else if self.players[t].tokens.is_empty() {
                    self.tell(p, format!("{} has no Honor tokens.", self.name(t)));
                    self.finish();
                } else {
                    let j = thread_rng().gen_range(0..self.players[t].tokens.len());
                    self.tell(p, format!("{} has a {}-point token.", self.name(t), self.players[t].tokens[j]));
                    if self.players[p].tokens.is_empty() {
                        self.finish();
                    } else {
                        self.set_stage(Stage::Follow(Follow::MerchantSwap(t, j)));
                    }
                }
            }
            _ => {}
        }
    }

    fn decide(&mut self, f: Follow, yes: bool) {
        let p = self.queue[0].0;
        match f {
            Follow::Shinobi(t) if yes => self.strike(p, t),
            Follow::Swap(a, b) => {
                if yes {
                    let ha = self.players[a].house;
                    self.players[a].house = self.players[b].house;
                    self.players[b].house = ha;
                    self.players[p].seen.swap(a, b);
                }
                self.finish();
            }
            Follow::GraveNow(c) => {
                if yes {
                    let (_, gd) = self.queue.pop_front().unwrap();
                    self.discard.push(gd);
                    self.say(format!("{} plays {} from the grave.", self.name(p), c.label()));
                    self.queue.push_front((p, c));
                    self.plays.insert(self.plays.iter().position(|&x| x == (p, gd)).map_or(0, |i| i + 1), (p, c));
                    self.players[p].played.push(c);
                    self.advance();
                } else {
                    self.players[p].hand.push(c);
                    self.finish();
                }
            }
            Follow::Reveal(t) => {
                if yes {
                    self.reveal(t);
                }
                self.finish();
            }
            Follow::MerchantSwap(t, j) => {
                if yes {
                    let mine = &self.players[p].tokens;
                    let k = (0..mine.len()).min_by_key(|&k| mine[k]).unwrap();
                    let (a, b) = (mine[k], self.players[t].tokens[j]);
                    self.players[p].tokens[k] = b;
                    self.players[t].tokens[j] = a;
                    self.tell(p, format!("You traded your {a} for their {b}."));
                    self.tell(t, format!("{} swapped your {b}-point token for a {a}.", self.name(p)));
                }
                self.finish();
            }
            _ => self.finish(),
        }
    }

    fn end_round(&mut self) {
        let n = self.players.len();
        self.queue.clear();
        self.say("Dawn. Everyone reveals their House:".into());
        for p in 0..n {
            let pl = &self.players[p];
            let s = format!("{}: {}{}", pl.name, pl.house.label(), if pl.alive { "" } else { " (dead)" });
            self.say(s);
            self.players[p].revealed = true;
        }
        let mm = self.mastermind.map(|m| self.players[m].house);
        if let Some(m) = self.mastermind {
            self.say(format!("{} survives as the Mastermind!", self.name(m)));
        }
        let survivors: Vec<House> = self.players.iter().filter(|pl| pl.alive).map(|pl| pl.house).collect();
        let side = match mm {
            Some(h) => h.side(),
            None => winner(&survivors),
        };
        let alive: Vec<usize> = (0..n).filter(|&q| self.players[q].alive).collect();
        let mut award: Vec<usize> = match side {
            Some(s) => {
                self.say(format!("{} House wins the round!", SIDES[s]));
                (0..n).filter(|&q| self.players[q].house.side() == Some(s)).collect()
            }
            None if mm.is_none() => {
                let reward = if self.no_honor { "wins" } else { "earns honor" };
                self.say(format!("A perfect tie: every survivor {reward}."));
                alive.clone()
            }
            None => {
                self.say("The Mastermind is the Ronin: no House wins.".into());
                vec![]
            }
        };
        for &q in &alive {
            if self.players[q].house == House::Ronin && !award.contains(&q) {
                self.say(format!("{} survives as the Ronin.", self.name(q)));
                award.push(q);
            }
        }
        if self.no_honor {
            // No tokens: whoever would have earned one wins, and the round is the whole game.
            let names: Vec<&str> = award.iter().map(|&q| self.name(q)).collect();
            let s = if names.is_empty() { "Nobody wins.".into() } else { format!("🏆 {} win!", names.join(" & ")) };
            self.say(s);
            self.winners = award;
            self.set_stage(Stage::GameOver);
            return;
        }
        for q in award {
            let v = self.draw();
            self.players[q].tokens.push(v);
            self.tell(q, format!("You drew a {v}-point Honor token."));
        }
        let score = |q: usize| self.players[q].tokens.iter().map(|&t| t as u32).sum::<u32>();
        let best = (0..n).map(score).max().unwrap_or(0);
        if best >= 10 {
            self.winners = (0..n).filter(|&q| score(q) == best).collect();
            let names: Vec<&str> = self.winners.iter().map(|&q| self.name(q)).collect();
            self.say(format!("🏆 {} win(s) the game with {best} points!", names.join(" & ")));
            self.set_stage(Stage::GameOver);
        } else {
            self.set_stage(Stage::RoundOver);
        }
    }

    pub fn view(&self, p: usize) -> View {
        let me = &self.players[p];
        let actor = self.queue.front().map(|&(q, c)| format!("{} is resolving {}", self.name(q), c.label()));
        let status = match &self.stage {
            Stage::Lobby => "Lobby".into(),
            Stage::Draft => format!("Ninja draft — pick {} of 2", if self.second_pick { 2 } else { 1 }),
            Stage::Declare(ph) => format!("Night — {} phase: everyone declares", PHASES[*ph]),
            Stage::Act | Stage::Follow(_) | Stage::React { .. } => format!("Night — {}", actor.unwrap_or_default()),
            Stage::RoundOver => "Round over".into(),
            Stage::GameOver => "Game over".into(),
        };
        let status = if self.no_honor { status } else { format!("{status} · Honor mode") };
        let lobby = matches!(self.stage, Stage::Lobby);
        View {
            me: p,
            host: self.host,
            lobby,
            no_honor: self.no_honor,
            caps: [new_deck(false).len() / 3, new_deck(true).len() / 3],
            timer: self.deadline.map(|d| d.saturating_duration_since(Instant::now()).as_secs()),
            turn_secs: self.turn_secs,
            status,
            house: if lobby {
                "—".into()
            } else if me.revealed {
                me.house.label()
            } else {
                format!("{} (as far as you know)", me.belief.label())
            },
            belief: (!lobby).then(|| if me.revealed { me.house } else { me.belief }.label()),
            hand: me.hand.iter().map(|&c| c.into()).collect(),
            tokens: me.tokens.clone(),
            players: self
                .players
                .iter()
                .enumerate()
                .map(|(t, pl)| {
                    let seen = me.seen.get(t).copied().flatten().filter(|_| !lobby && !pl.revealed && t != p);
                    PlayerView {
                        name: pl.name.clone(),
                        bot: pl.bot,
                        alive: pl.alive,
                        tokens: pl.tokens.len(),
                        cards: pl.hand.len(),
                        house: if pl.revealed && !lobby { Some(pl.house.label()) } else { seen.map(|h| h.label()) },
                        private: seen.is_some(),
                        played: pl.played.iter().map(|&c| c.into()).collect(),
                        // Only while it's still in their hand; once played it shows face up instead.
                        peeked: me
                            .peeked
                            .get(t)
                            .copied()
                            .flatten()
                            .filter(|c| pl.hand.contains(c))
                            .map(CardView::from),
                        death: pl.killed_by.clone(),
                        avatar: pl.avatar.clone(),
                    }
                })
                .collect(),
            prompt: self.prompt(p),
            log: self
                .log
                .iter()
                .filter(|(to, _)| to.is_none_or(|q| q == p))
                .map(|(to, s)| if to.is_some() { format!("🔒 {s}") } else { s.clone() })
                .rev()
                .take(80)
                .collect(),
            winners: self.winners.iter().map(|&q| self.players[q].name.clone()).collect(),
            track: match self.stage {
                Stage::Lobby => None,
                Stage::Draft => Some(0),
                Stage::Declare(_) | Stage::Act | Stage::Follow(_) | Stage::React { .. } => Some(self.phase + 1),
                Stage::RoundOver | Stage::GameOver => Some(PHASES.len() + 1),
            },
            plays: self
                .plays
                .iter()
                .map(|&play| {
                    let resolving = matches!(self.stage, Stage::Act | Stage::Follow(_) | Stage::React { .. });
                    let state = if resolving && self.queue.front() == Some(&play) {
                        "now"
                    } else if self.queue.contains(&play) {
                        "next"
                    } else if self.skipped.contains(&play) {
                        "skipped"
                    } else {
                        "done"
                    };
                    PlayView { seat: play.0, card: play.1.into(), state }
                })
                .collect(),
            plays_phase: self.plays_phase,
            acting: match &self.stage {
                Stage::Act | Stage::Follow(_) | Stage::React { .. } => self.queue.front().map(|&(seat, c)| Acting {
                    seat,
                    card: c.into(),
                    targets: match &self.stage {
                        Stage::Follow(Follow::Shinobi(t) | Follow::Reveal(t) | Follow::MerchantChoose(t)) => vec![*t],
                        Stage::Follow(Follow::MerchantSwap(t, _)) => vec![*t],
                        Stage::Follow(Follow::Swap(a, b)) => vec![*a, *b],
                        Stage::React { victim, .. } => vec![*victim],
                        _ => vec![],
                    },
                }),
                _ => None,
            },
            news: self.log[me.acted_at.min(self.log.len())..]
                .iter()
                .filter(|(to, _)| *to == Some(p))
                .map(|(_, s)| s.clone())
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use House::*;

    /// Honor mode is opt-in; these tests need multi-round games and the full 11 seats.
    fn honor_game() -> Game {
        let mut g = Game::new();
        g.no_honor = false;
        g
    }

    #[test]
    fn rulebook_reveal_examples() {
        // Lotus 1 beats any number of Crane survivors.
        assert_eq!(winner(&[Lotus(1), Crane(3), Crane(4)]), Some(0));
        // Highest cards tie (2s), so the next pair decides: 3 beats 4.
        assert_eq!(winner(&[Lotus(2), Lotus(4), Crane(2), Crane(3)]), Some(1));
        // Total tie: no House wins.
        assert_eq!(winner(&[Lotus(2), Crane(2), Ronin]), None);
        assert_eq!(winner(&[]), None);
    }

    #[test]
    fn random_games_always_finish() {
        let mut rng = thread_rng();
        for n in 4..=new_deck(false).len() / 3 {
            for _ in 0..20 {
                let mut g = honor_game();
                for i in 0..n {
                    g.join(format!("P{i}")).unwrap();
                }
                let mut steps = 0;
                while !matches!(g.stage, Stage::GameOver) {
                    steps += 1;
                    assert!(steps < 100_000, "game never ended");
                    let p = (0..n).find(|&p| g.prompt(p).is_some()).expect("nobody can act");
                    let pr = g.prompt(p).unwrap();
                    let k = pr.options.len();
                    let a = match pr.kind {
                        "start" => Action::Start,
                        "next" => Action::Next,
                        "pick" => Action::Pick { i: rng.gen_range(0..k) },
                        "decide" => Action::Decide { yes: rng.gen() },
                        "play" => Action::Play { is: (0..k).filter(|_| rng.gen()).collect() },
                        "target" => Action::Target { is: rand::seq::index::sample(&mut rng, k, pr.n).into_vec() },
                        other => panic!("unknown prompt {other}"),
                    };
                    g.act(p, a).unwrap();
                }
                assert!(!g.winners.is_empty());
                assert!(g.players.iter().all(|pl| pl.revealed));
            }
        }
    }

    #[test]
    fn host_with_bots_plays_to_the_end() {
        for n in 4..=new_deck(false).len() / 3 {
            for _ in 0..10 {
                let mut g = honor_game();
                g.join("Host".into()).unwrap();
                for _ in 1..n {
                    g.add_bot(0).unwrap();
                }
                let mut steps = 0;
                while !matches!(g.stage, Stage::GameOver) {
                    steps += 1;
                    assert!(steps < 10_000, "game never ended");
                    // Bots move by themselves, so the human host must always be the one we're waiting on.
                    let pr = g.prompt(0).expect("waiting on a bot");
                    let a = g.bot_action(0).unwrap_or(if pr.kind == "start" { Action::Start } else { Action::Next });
                    g.act(0, a).unwrap();
                }
            }
        }
    }

    /// Host P0 (Crane 2) plus bots P1 (Lotus 1), P2 (Lotus 2), P3 (Crane 1); P1 is about to resolve `card`.
    fn bot_turn(card: Kind) -> Game {
        let mut g = Game::new();
        g.join("Host".into()).unwrap();
        for _ in 0..3 {
            g.add_bot(0).unwrap();
        }
        g.act(0, Action::Start).unwrap();
        for (p, h) in [Crane(2), Lotus(1), Lotus(2), Crane(1)].into_iter().enumerate() {
            (g.players[p].house, g.players[p].belief) = (h, h);
        }
        g.queue = VecDeque::from([(1, Card { kind: card, num: 1 })]);
        g.stage = Stage::Act;
        g
    }

    fn bot_target(g: &Game) -> usize {
        match g.bot_action(1) {
            Some(Action::Target { is }) => g.targets().0[is[0]],
            other => panic!("expected a target, got {other:?}"),
        }
    }

    #[test]
    fn bots_never_kill_a_revealed_ally() {
        for _ in 0..50 {
            let mut g = bot_turn(Kind::BlindAssassin);
            g.players[2].revealed = true; // P2 revealed Lotus, like a Judge does
            assert_ne!(bot_target(&g), 2);
            g.players[3].revealed = true; // a revealed enemy is the first choice
            assert_eq!(bot_target(&g), 3);
        }
    }

    #[test]
    fn peeked_house_shows_only_on_the_peekers_cards() {
        let mut g = bot_turn(Kind::Spy);
        g.players[1].seen[3] = Some(Crane(1));
        let spy = &g.view(1).players[3];
        assert_eq!((spy.house.as_deref(), spy.private), (Some("Crane 1"), true));
        assert_eq!(g.view(0).players[3].house, None, "nobody else learns it");
        g.players[3].revealed = true;
        let public = &g.view(0).players[3];
        assert_eq!((public.house.as_deref(), public.private), (Some("Crane 1"), false));
    }

    #[test]
    fn played_cards_are_public_and_mystic_peeks_private() {
        let mut g = bot_turn(Kind::Mystic);
        let shinobi = Card { kind: Kind::Shinobi, num: 2 };
        g.players[3].hand = vec![shinobi];
        g.resolve(vec![3]); // P1's Mystic looks at P3
        assert_eq!(g.view(1).players[3].peeked.as_ref().map(|c| c.name.as_str()), Some("Shinobi 2"));
        assert!(g.view(0).players[3].peeked.is_none(), "only the Mystic saw it");
        // Once P3 plays it, it's face up for everyone and no longer a private peek.
        g.players[3].hand.clear();
        g.players[3].played.push(shinobi);
        assert!(g.view(1).players[3].peeked.is_none());
        assert_eq!(g.view(0).players[3].played[0].name, "Shinobi 2");
    }

    #[test]
    fn leaving_the_lobby_and_mid_game() {
        let mut g = Game::new();
        for i in 0..3 {
            g.join(format!("P{i}")).unwrap();
        }
        assert!(g.leave(0), "lobby: seat removed");
        assert_eq!((g.players.len(), g.host, g.players[0].name.as_str()), (2, 0, "P1"));
        g.add_bot(0).unwrap();
        g.add_bot(0).unwrap();
        g.act(0, Action::Start).unwrap();
        assert!(!g.leave(0), "mid-game: the seat stays");
        assert!(g.players[0].bot && g.host == 1, "a bot plays it and P2 hosts");
        // The bot-played seat never holds the game up.
        while !matches!(g.stage, Stage::GameOver) {
            let a = g.bot_action(1).expect("waiting on the remaining human");
            g.act(1, a).unwrap();
        }
    }

    #[test]
    fn phase_plays_show_who_played_what() {
        let mut g = bot_turn(Kind::Spy);
        let (spy1, spy4) = (Card { kind: Kind::Spy, num: 1 }, Card { kind: Kind::Spy, num: 4 });
        g.plays = vec![(1, spy1), (2, spy4)];
        g.queue = VecDeque::from([(1, spy1), (2, spy4)]);
        let states = |g: &Game| g.view(0).plays.iter().map(|pl| (pl.seat, pl.state)).collect::<Vec<_>>();
        assert_eq!(states(&g), [(1, "now"), (2, "next")]);
        g.players[2].alive = false;
        g.resolve(vec![0]);
        assert_eq!(states(&g), [(1, "done"), (2, "skipped")]);
    }

    #[test]
    fn deaths_say_who_did_it() {
        let mut g = bot_turn(Kind::BlindAssassin);
        g.players[2].hand.clear(); // no react card to save them
        let acting = g.view(0).acting.unwrap();
        assert_eq!((acting.seat, acting.card.name.as_str()), (1, "Blind Assassin 1"));
        g.resolve(vec![2]);
        assert_eq!(g.view(0).players[2].death.as_deref(), Some("🤖 Hanzo's Blind Assassin 1"));
        assert!(g.log.last().unwrap().1.contains("killed by 🤖 Hanzo's Blind Assassin 1"));
    }

    #[test]
    fn bots_resolve_cards_at_a_readable_pace() {
        let mut g = bot_turn(Kind::Spy);
        g.pace = Duration::from_secs(2);
        g.run_bots();
        assert!(matches!(g.stage, Stage::Act) && g.queue.len() == 1, "the bot waits its turn");
        let now = Instant::now();
        g.tick(now);
        assert_eq!(g.queue.len(), 1, "still waiting before the pause is over");
        g.tick(now + Duration::from_secs(3));
        assert!(g.queue.is_empty(), "then the Spy resolves");
    }

    #[test]
    fn bots_spy_on_someone_new() {
        for _ in 0..50 {
            let mut g = bot_turn(Kind::Spy);
            g.players[1].seen[3] = Some(Crane(1));
            g.players[2].revealed = true;
            assert_eq!(bot_target(&g), 0, "the only House P1 doesn't know yet");
        }
    }

    #[test]
    fn timeouts_alone_finish_a_game() {
        let mut g = Game::new();
        g.join("AFK host".into()).unwrap();
        for _ in 0..4 {
            g.add_bot(0).unwrap();
        }
        g.act(0, Action::Start).unwrap();
        let mut now = Instant::now();
        let mut steps = 0;
        while !matches!(g.stage, Stage::GameOver) {
            steps += 1;
            assert!(steps < 10_000, "game stalled");
            if g.prompt(0).is_some_and(|p| p.kind == "next") {
                g.act(0, Action::Next).unwrap();
            } else {
                now += Duration::from_secs(601);
                g.tick(now);
            }
        }
    }

    #[test]
    fn timeout_only_moves_the_stalled_step() {
        let mut g = Game::new();
        for i in 0..4 {
            g.join(format!("P{i}")).unwrap();
        }
        g.act(0, Action::Start).unwrap();
        g.tick(Instant::now()); // not yet due
        assert!(matches!(g.stage, Stage::Draft) && !g.second_pick);
        g.act(0, Action::Pick { i: 0 }).unwrap();
        g.tick(Instant::now() + Duration::from_secs(61));
        // The three slow players were picked for; the second pick starts with a fresh clock.
        assert!(g.second_pick && g.picks.iter().all(Option::is_none));
        assert!(g.view(1).news.iter().any(|s| s.contains("ran out of time")));
        assert!(!g.view(0).news.iter().any(|s| s.contains("ran out of time")));
        assert!(g.view(0).timer.unwrap() >= 59);
    }

    #[test]
    fn no_honor_mode() {
        assert!(Game::new().no_honor, "No Honor is the default");
        assert_eq!((new_deck(false).len(), new_deck(true).len()), (33, 30));
        assert!(new_deck(true).iter().all(|c| !matches!(c.kind, Kind::Martyr | Kind::SoulMerchant | Kind::Thief)));
        // 11 players is too many for the smaller deck.
        let mut g = honor_game();
        g.join("Host".into()).unwrap();
        for _ in 0..10 {
            g.add_bot(0).unwrap();
        }
        assert!(g.set_no_honor(0, true).is_err());
        g.kick(0, 10).unwrap();
        g.set_no_honor(0, true).unwrap();
        assert!(g.add_bot(0).is_err(), "10 is the cap");
        // Every game is a single round with no tokens handed out.
        for n in 4..=10 {
            for _ in 0..10 {
                let mut g = Game::new();
                g.join("Host".into()).unwrap();
                for _ in 1..n {
                    g.add_bot(0).unwrap();
                }
                g.set_no_honor(0, true).unwrap();
                g.act(0, Action::Start).unwrap();
                while !matches!(g.stage, Stage::GameOver) {
                    let a = g.bot_action(0).expect("host should be the one we're waiting on");
                    g.act(0, a).unwrap();
                }
                assert!(g.players.iter().all(|pl| pl.tokens.is_empty()));
            }
        }
    }

    #[test]
    fn card_list_covers_every_card() {
        let list = card_list();
        assert_eq!(list.iter().map(|c| c.numbers.len()).sum::<usize>(), 33);
        assert_eq!(list.iter().filter(|c| c.honor).map(|c| c.numbers.len()).sum::<usize>(), 3);
        assert!(list.iter().all(|c| !c.text.is_empty()));
        assert_eq!(list[0].name, "Spy");
        assert_eq!(list.last().unwrap().name, "Mastermind");
    }

    #[test]
    fn news_shows_private_info_until_next_move() {
        let mut g = Game::new();
        for i in 0..4 {
            g.join(format!("P{i}")).unwrap();
        }
        g.act(0, Action::Start).unwrap();
        for p in 0..4 {
            let news = g.view(p).news;
            assert_eq!(news.len(), 1);
            assert!(news[0].starts_with("You are "), "{news:?}");
        }
        g.act(1, Action::Pick { i: 0 }).unwrap();
        assert!(g.view(1).news.is_empty());
        assert_eq!(g.view(2).news.len(), 1, "other players keep theirs");
    }

    #[test]
    fn game_over_returns_to_the_lobby() {
        let mut g = Game::new();
        g.join("Host".into()).unwrap();
        for _ in 0..3 {
            g.add_bot(0).unwrap();
        }
        g.act(0, Action::Start).unwrap();
        while !matches!(g.stage, Stage::GameOver) {
            let a = g.bot_action(0).expect("waiting on the host");
            g.act(0, a).unwrap();
        }
        g.act(0, Action::Next).unwrap();
        assert!(matches!(g.stage, Stage::Lobby));
        assert!(g.players.iter().all(|pl| pl.alive && pl.hand.is_empty() && pl.played.is_empty()));
        g.add_bot(0).unwrap(); // the lobby is fully usable again
        g.act(0, Action::Start).unwrap();
        assert!(matches!(g.stage, Stage::Draft));
    }

    #[test]
    fn host_controls() {
        let mut g = Game::new();
        g.join("A".into()).unwrap();
        g.join("B".into()).unwrap();
        assert!(g.add_bot(1).is_err(), "only the host adds bots");
        g.add_bot(0).unwrap();
        assert!(g.make_host(0, 2).is_err(), "bots can't host");
        g.make_host(0, 1).unwrap();
        assert!(g.kick(0, 1).is_err(), "A is no longer host");
        g.kick(1, 0).unwrap();
        assert_eq!((g.players.len(), g.host, g.players[0].name.as_str()), (2, 0, "B"));
        assert!(g.kick(0, 0).is_err(), "can't kick yourself");
        g.add_bot(0).unwrap();
        g.add_bot(0).unwrap();
        g.act(0, Action::Start).unwrap();
        assert!(g.kick(0, 1).is_err(), "no kicking mid-game");
    }

    #[test]
    fn wrong_player_and_bad_input_rejected() {
        let mut g = Game::new();
        for i in 0..4 {
            g.join(format!("P{i}")).unwrap();
        }
        assert!(g.act(1, Action::Start).is_err());
        g.act(0, Action::Start).unwrap();
        assert!(g.act(0, Action::Pick { i: 9 }).is_err());
        assert!(g.join("late".into()).is_err());
    }
}
