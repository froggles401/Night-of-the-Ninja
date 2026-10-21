mod game;

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// One game per room. In Discord the room is the Activity instance (one per voice channel);
/// on the web it's a lobby code. `keys[p]` is player p's secret (None for bots); `users[p]` their Discord id.
#[derive(Default)]
struct Table {
    game: game::Game,
    /// The code web players join with; None for Discord rooms.
    code: Option<String>,
    keys: Vec<Option<u64>>,
    users: Vec<Option<String>>,
    /// Discord ids the host kicked; they can't rejoin this table.
    banned: Vec<String>,
}

struct App {
    // shortcut: finished rooms are never removed; fine until the server hosts thousands of games.
    tables: Mutex<HashMap<String, Table>>,
    http: reqwest::Client,
    client_id: Option<String>,
    client_secret: Option<String>,
}
type Shared = Arc<App>;
type Res<T> = Result<T, (StatusCode, String)>;

fn bad(e: impl ToString) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}
fn upstream(e: impl ToString) -> (StatusCode, String) {
    (StatusCode::BAD_GATEWAY, e.to_string())
}

#[derive(Deserialize)]
struct JoinReq {
    name: Option<String>,
    room: Option<String>,
    access_token: Option<String>,
}
#[derive(Deserialize)]
struct CreateReq {
    name: String,
}
#[derive(Deserialize)]
struct CodeReq {
    code: String,
}
#[derive(Deserialize)]
struct KeyQuery {
    key: String,
}
#[derive(Deserialize)]
struct ActReq {
    key: String,
    action: game::Action,
}
#[derive(Deserialize)]
struct DiscordUser {
    id: String,
    username: String,
    global_name: Option<String>,
    avatar: Option<String>,
}

/// Run `f` on the table and seat that `key` belongs to.
fn with_player<T>(s: &App, key: &str, f: impl FnOnce(&mut Table, usize) -> Res<T>) -> Res<T> {
    let k: u64 = key.parse().map_err(|_| (StatusCode::UNAUTHORIZED, "Unknown player".to_string()))?;
    let mut tables = s.tables.lock().unwrap();
    for t in tables.values_mut() {
        if let Some(p) = t.keys.iter().position(|&x| x == Some(k)) {
            return f(t, p);
        }
    }
    Err((StatusCode::UNAUTHORIZED, "Unknown player".into()))
}

async fn config(State(s): State<Shared>) -> Json<Value> {
    Json(json!({ "clientId": s.client_id }))
}

/// Discord Activity OAuth: trade the code from `authorize()` for an access token. Needs the client secret.
async fn token(State(s): State<Shared>, Json(r): Json<CodeReq>) -> Res<Json<Value>> {
    let (Some(id), Some(secret)) = (&s.client_id, &s.client_secret) else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "Set DISCORD_CLIENT_ID and DISCORD_CLIENT_SECRET.".into()));
    };
    let form = [
        ("client_id", id.as_str()),
        ("client_secret", secret.as_str()),
        ("grant_type", "authorization_code"),
        ("code", r.code.as_str()),
    ];
    let resp = s.http.post("https://discord.com/api/oauth2/token").form(&form).send().await.map_err(upstream)?;
    if !resp.status().is_success() {
        return Err(upstream(format!("Discord token exchange failed: {}", resp.status())));
    }
    let v: Value = resp.json().await.map_err(upstream)?;
    Ok(Json(json!({ "access_token": v["access_token"] })))
}

/// Lobby codes avoid look-alike characters (no I/L/O/0/1) so they're easy to read out loud.
const CODE_CHARS: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 5;

fn valid_code(c: &str) -> bool {
    c.len() == CODE_LEN && c.bytes().all(|b| CODE_CHARS.contains(&b))
}

fn valid_name(name: &str) -> Res<()> {
    if name.is_empty() || name.chars().count() > 20 {
        return Err(bad("Name must be 1-20 characters."));
    }
    Ok(())
}

/// Seat a new player at `t` and hand back their secret key.
fn seat(t: &mut Table, name: String, uid: Option<String>, avatar: Option<String>) -> Res<Json<Value>> {
    let p = t.game.join(name).map_err(bad)?;
    t.game.set_avatar(p, avatar);
    let key: u64 = rand::random();
    t.keys.push(Some(key));
    t.users.push(uid);
    // Sent as a string: JS numbers can't hold a full u64.
    Ok(Json(json!({ "key": key.to_string(), "code": t.code })))
}

/// Web: start a new lobby with a fresh code, with the creator as host.
async fn create(State(s): State<Shared>, Json(r): Json<CreateReq>) -> Res<Json<Value>> {
    let name = r.name.trim().to_string();
    valid_name(&name)?;
    let mut tables = s.tables.lock().unwrap();
    let code = loop {
        let c: String = (0..CODE_LEN).map(|_| CODE_CHARS[rand::random::<usize>() % CODE_CHARS.len()] as char).collect();
        if !tables.contains_key(&c) {
            break c;
        }
    };
    let t = tables.entry(code.clone()).or_default();
    t.code = Some(code);
    seat(t, name, None, None)
}

async fn join(State(s): State<Shared>, Json(r): Json<JoinReq>) -> Res<Json<Value>> {
    // Discord players are identified by asking Discord who the token belongs to, never by what the client claims.
    let (name, uid, avatar) = match &r.access_token {
        Some(tok) => {
            let u: DiscordUser = s
                .http
                .get("https://discord.com/api/users/@me")
                .bearer_auth(tok)
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(upstream)?
                .json()
                .await
                .map_err(upstream)?;
            // Served through /api/avatar: Discord's iframe blocks images from other sites.
            let avatar = u.avatar.map(|hash| format!("/api/avatar/{}/{hash}", u.id));
            (u.global_name.unwrap_or(u.username).chars().take(20).collect(), Some(u.id), avatar)
        }
        None => (r.name.unwrap_or_default().trim().to_string(), None, None),
    };
    valid_name(&name)?;
    let room = r.room.unwrap_or_default();
    let mut tables = s.tables.lock().unwrap();
    let t = if uid.is_some() {
        // Discord: the room is the Activity instance, created on first join.
        if room.is_empty() || room.len() > 100 {
            return Err(bad("Bad room id."));
        }
        tables.entry(room).or_default()
    } else {
        // Web: only existing lobbies, and only by their code (never a Discord room id).
        let code = room.trim().to_uppercase();
        if !valid_code(&code) {
            return Err(bad(format!("Lobby codes are {CODE_LEN} letters and numbers.")));
        }
        tables.get_mut(&code).ok_or((StatusCode::NOT_FOUND, "No lobby with that code.".to_string()))?
    };
    // Reopening the Activity puts you back in your seat.
    if let Some(p) = uid.as_ref().and_then(|id| t.users.iter().position(|u| u.as_ref() == Some(id))) {
        t.game.set_avatar(p, avatar);
        return Ok(Json(json!({ "key": t.keys[p].unwrap().to_string() })));
    }
    if uid.as_ref().is_some_and(|id| t.banned.contains(id)) {
        return Err((StatusCode::FORBIDDEN, "The host removed you from this lobby.".into()));
    }
    seat(t, name, uid, avatar)
}

/// Proxies a Discord avatar. Strictly validated, so this can only ever fetch avatar images from Discord's CDN.
async fn avatar(State(s): State<Shared>, Path((id, hash)): Path<(String, String)>) -> Res<impl IntoResponse> {
    let valid = (1..=20).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_digit())
        && (1..=40).contains(&hash.len())
        && hash.bytes().all(|b| b.is_ascii_hexdigit() || b == b'_');
    if !valid {
        return Err(bad("Bad avatar id."));
    }
    let url = format!("https://cdn.discordapp.com/avatars/{id}/{hash}.png?size=96");
    let resp = s.http.get(url).send().await.and_then(|r| r.error_for_status()).map_err(upstream)?;
    let bytes = resp.bytes().await.map_err(upstream)?;
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=86400")], bytes))
}

/// Leave the table: the seat goes (lobby) or a bot takes it (mid-game). Tables with no humans left are dropped.
async fn leave(State(s): State<Shared>, Json(r): Json<KeyQuery>) -> Res<StatusCode> {
    let unknown = || (StatusCode::UNAUTHORIZED, "Unknown player".to_string());
    let k: u64 = r.key.parse().map_err(|_| unknown())?;
    let mut tables = s.tables.lock().unwrap();
    let (room, p) = tables
        .iter()
        .find_map(|(room, t)| t.keys.iter().position(|&x| x == Some(k)).map(|p| (room.clone(), p)))
        .ok_or_else(unknown)?;
    let t = tables.get_mut(&room).unwrap();
    if t.game.leave(p) {
        t.keys.remove(p);
        t.users.remove(p);
    } else {
        // Mid-game the seat stays for the bot; forgetting the Discord id means reopening starts fresh.
        t.keys[p] = None;
        t.users[p] = None;
    }
    if t.game.humans() == 0 {
        tables.remove(&room);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn state(State(s): State<Shared>, Query(q): Query<KeyQuery>) -> Res<Json<Value>> {
    with_player(&s, &q.key, |t, p| {
        // shortcut: timers only advance while someone polls; fine since every open page polls each second.
        t.game.tick(std::time::Instant::now());
        let mut v = serde_json::to_value(t.game.view(p)).map_err(bad)?;
        v["code"] = json!(t.code);
        Ok(Json(v))
    })
}

async fn act(State(s): State<Shared>, Json(r): Json<ActReq>) -> Res<StatusCode> {
    use game::Action;
    with_player(&s, &r.key, |t, p| {
        // Seat changes must keep keys/users lined up with the game's player list.
        match r.action {
            Action::AddBot => {
                t.game.add_bot(p).map_err(bad)?;
                t.keys.push(None);
                t.users.push(None);
            }
            Action::Kick { who } => {
                t.game.kick(p, who).map_err(bad)?;
                t.keys.remove(who);
                t.banned.extend(t.users.remove(who));
            }
            Action::MakeHost { who } => t.game.make_host(p, who).map_err(bad)?,
            Action::SetNoHonor { on } => t.game.set_no_honor(p, on).map_err(bad)?,
            Action::SetTimer { secs } => t.game.set_timer(p, secs).map_err(bad)?,
            a => t.game.act(p, a).map_err(bad)?,
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

fn js(src: &'static str) -> ([(header::HeaderName, &'static str); 1], &'static str) {
    ([(header::CONTENT_TYPE, "text/javascript")], src)
}

/// Print each request and its status, so you can see whether Discord's proxy is reaching us.
async fn log_request(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    let res = next.run(req).await;
    // The 1-second state poll would flood the console.
    if path != "/api/state" || !res.status().is_success() {
        println!("{method} {path} -> {}", res.status().as_u16());
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lobby_codes() {
        assert!(valid_code("AB2CD"));
        assert!(!valid_code("ab2cd"), "callers upper-case first");
        assert!(!valid_code("AB0CD"), "no look-alike characters");
        assert!(!valid_code("ABCD"));
        assert!(!valid_code("1234567890123"), "Discord instance ids are never valid codes");
    }
}

#[tokio::main]
async fn main() {
    // Real environment variables win; `.env` (KEY=VALUE lines, # comments) fills in the rest.
    let dotenv: HashMap<String, String> = std::fs::read_to_string(".env")
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches('"').to_string()))
        .collect();
    let env = |k: &str| std::env::var(k).ok().or_else(|| dotenv.get(k).cloned()).filter(|v| !v.is_empty());
    let app = Arc::new(App {
        tables: Mutex::default(),
        http: reqwest::Client::new(),
        client_id: env("DISCORD_CLIENT_ID"),
        client_secret: env("DISCORD_CLIENT_SECRET"),
    });
    if app.client_secret.is_none() {
        println!("DISCORD_CLIENT_ID / DISCORD_CLIENT_SECRET not set: browser mode only.");
    }
    let router = Router::new()
        .route("/", get(|| async { Html(include_str!("../static/index.html")) }))
        .route("/app.js", get(|| async { js(include_str!("../static/app.js")) }))
        .route("/discord.js", get(|| async { js(include_str!("../static/discord.js")) }))
        .route("/api/config", get(config))
        .route("/api/avatar/{id}/{hash}", get(avatar))
        .route("/api/cards", get(|| async { Json(game::card_list()) }))
        .route("/api/token", post(token))
        .route("/api/create", post(create))
        .route("/api/join", post(join))
        .route("/api/state", get(state))
        .route("/api/act", post(act))
        .route("/api/leave", post(leave))
        .layer(axum::middleware::from_fn(log_request))
        .with_state(app);
    let port = env("PORT").unwrap_or_else(|| "3000".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await.unwrap();
    println!("Night of the Ninja on http://localhost:{port}");
    axum::serve(listener, router).await.unwrap();
}
