# syntax=docker/dockerfile:1.6
FROM rust:1.94-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p marmotte-cli && \
    cp target/release/marmotte /usr/local/bin/marmotte

FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /usr/local/bin/marmotte /usr/local/bin/marmotte
USER nonroot
EXPOSE 8080
VOLUME ["/var/lib/marmotte"]
ENTRYPOINT ["/usr/local/bin/marmotte"]
CMD ["serve", "--config", "/etc/marmotte/config.toml"]
