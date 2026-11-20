FROM rust@sha256:223cc32ddca8f3ee90c436dbc94b6b5f052ea7f9653c5201e9612f08ad7d21b8 AS build
WORKDIR /work
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked -j 2

FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates ffmpeg && apt-get clean && find /var/lib/apt/lists -type f -delete
COPY --from=build /work/target/release/xsx-native /usr/local/bin/xsx-native
ENV XBOX_AUTH_FILE=/auth/session.json STATE_DIR=/state
ENTRYPOINT ["/usr/local/bin/xsx-native"]
