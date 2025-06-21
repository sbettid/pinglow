################################# Build Container ###############################

# Use Rust official image for build stage
FROM rust:1.81-bullseye as builder

WORKDIR /app

# Copy files and build in release mode
COPY . .
RUN cargo build --release

################################# Prod Container #################################

# Use a minimal base image
FROM debian:bullseye-slim

WORKDIR /app
COPY --from=builder /app/target/release/pinglow .

# Run the binary
CMD ["./pinglow"]