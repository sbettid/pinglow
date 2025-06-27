################################# Build Container ###############################

# Use Rust official image for build stage
FROM rust:1.81-bullseye as rust-builder

WORKDIR /app

# Copy files and build in release mode
COPY . .
RUN cargo build --release

################################# Prod Container #################################

FROM debian:bullseye-slim

COPY --from=rust-builder /app/target/release/pinglow /usr/local/bin/pinglow

# Run the binary
CMD ["pinglow"]