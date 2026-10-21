# static/discord.js is committed, so the image only needs Rust; re-run `npm run build` before deploying after an SDK bump.
FROM rust:1-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
# reqwest uses OpenSSL for the HTTPS call to Discord's token endpoint.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/night-of-the-ninja /usr/local/bin/
ENV PORT=3000
EXPOSE 3000
CMD ["night-of-the-ninja"]
