################################# Build Container ###############################

# Use Rust official image for build stage
FROM rust:1.88-bullseye as rust-builder

WORKDIR /app

# Copy files and build in release mode
COPY . .
RUN cargo build --release

################################# Prod Container #################################

FROM debian:bullseye-slim

COPY --from=rust-builder /app/target/release/pinglow /usr/local/bin/pinglow

RUN addgroup --system pinglow && adduser --system --ingroup pinglow pinglow

RUN chown pinglow:pinglow /usr/local/bin/pinglow && chmod 755 /usr/local/bin/pinglow

USER pinglow

WORKDIR /home/pinglow

# Run the binary
CMD ["pinglow"]