FROM rust:1.83 AS dependencies
WORKDIR /app
COPY Cargo.toml .
COPY Cargo.lock .
RUN mkdir -p src
RUN echo "fn main() {}" > src/main.rs
RUN cargo build --release

FROM rust:1.83 AS application
WORKDIR /app
COPY /Cargo.toml .
COPY /Cargo.lock .
COPY --from=dependencies /app/target/ /app/target
COPY --from=dependencies /usr/local/cargo /usr/local/cargo
COPY src/ src/
RUN cargo build --release

FROM debian:bookworm-slim AS runner
RUN apt-get update
RUN apt-get install -y ca-certificates 
RUN rm -rf /var/lib/apt/lists/*
EXPOSE 3000
COPY --from=application /app/target/release/grepolis_api_reflector /grepolis_api_reflector
CMD ["/grepolis_api_reflector"]
