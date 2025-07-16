################################# Build Container ###############################

# Use Rust official image for build stage
FROM --platform=$BUILDPLATFORM rust:1.88-bullseye as rust-builder

WORKDIR /app

# Install cross-compilation dependencies
RUN apt-get update && apt-get install -y \
    musl-tools \
    pkg-config \
    curl \
    build-essential \
    cmake \
    perl \
    git

# Copy files and build in release mode
COPY . .


# Set target based on platform
ARG TARGETPLATFORM

RUN case "${TARGETPLATFORM}" in \
    "linux/amd64")  export TARGET="x86_64-unknown-linux-musl" ;; \
    "linux/arm64")  export TARGET="aarch64-unknown-linux-musl" ;; \
    *)              echo "unsupported platform: $TARGETPLATFORM" && exit 1 ;; \
    esac && \
    rustup target add $TARGET && \
    cargo build --release --target=$TARGET --target-dir /app/target && cp /app/target/$TARGET/release/pinglow /app/pinglow


################################# Prod Container #################################

FROM debian:bullseye-slim

COPY --from=rust-builder /app/pinglow /usr/local/bin/pinglow

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN addgroup --system pinglow && adduser --system --ingroup pinglow pinglow

RUN chown pinglow:pinglow /usr/local/bin/pinglow && chmod 755 /usr/local/bin/pinglow

USER pinglow

WORKDIR /home/pinglow

# Run the binary
CMD ["pinglow"]