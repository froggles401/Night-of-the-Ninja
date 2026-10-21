# Night of the Ninja — Discord Activity

A Rust (axum) server runs the game. Players launch it as an Activity inside a Discord voice channel.
Each voice channel gets its own table, and each player is seated by their Discord account, so
closing and reopening the Activity puts them back in their seat.

## Build

```bash
npm install
npm run build      # bundles Discord's SDK into static/discord.js
cargo run
```

It also works as a plain website at http://localhost:3000:
- **Create lobby** makes a table with a 5-character code.
- Friends join with that code, or with the invite link `http://<host>/?code=ABCDE`.

## One-time Discord setup

1. Go to https://discord.com/developers/applications and click **New Application**.
2. On the **OAuth2** page:
   - Copy the **Client ID**.
   - Reset and copy the **Client Secret**.
   - Under Redirects, add `https://127.0.0.1`. Discord requires a redirect even though Activities never use it.
3. On **Activities → Settings**, turn on **Enable Activities**.
4. Discord can only load the Activity over public HTTPS. For development, run a tunnel:
   ```bash
   winget install Cloudflare.cloudflared
   cloudflared tunnel --url http://localhost:3000
   ```
   The tunnel prints a URL like `https://something.trycloudflare.com`. The URL changes every time you
   restart the tunnel. When that happens, update the next step.
5. On **Activities → URL Mappings**, set the **Root Mapping** `/` to that host, without the `https://`.

## Run it

Paste your Client ID and Client Secret into `.env` in the project folder, then start the server.
The secret stays on the server and is never sent to players. `.env` is gitignored, so keep it out of commits.
Real environment variables with the same names override `.env`.

```bash
cargo run
```

In Discord:
1. Join a voice channel.
2. Click the rocket (**Activities**) button and pick your app.

Until the app is published, only you and members of the app's team (**Team** page in the portal) can launch it.
Add your friends there to test together.

## Deploy (Fly.io)

The `Dockerfile` and `fly.toml` run the server on one always-on Fly.io machine. Games are kept in memory,
so the app must run on exactly one machine. With two machines, players at the same table could land on different ones.

```bash
winget install flyctl
fly auth login
fly launch --no-deploy --copy-config   # pick a unique app name if night-of-the-ninja is taken
fly secrets set DISCORD_CLIENT_ID=... DISCORD_CLIENT_SECRET=...
fly deploy --ha=false
```

Then set the Discord **Root Mapping** `/` to `<app-name>.fly.dev`. That URL stays the same across deploys, so you no longer need a tunnel.
Every deploy restarts the server, which ends any game in progress.

## Files

- `src/game.rs`: the rules engine and its tests (`cargo test`).
- `src/main.rs`: HTTP API, Discord OAuth token exchange, and one table per Activity instance.
- `static/index.html`, `static/app.js`: the game page.
- `web/discord-sdk.js`: the entry file for the bundled SDK.
