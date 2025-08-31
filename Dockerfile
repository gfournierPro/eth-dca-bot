# Use the official Rust image as a parent image for building
FROM rust:1.75 as builder

# Set the working directory in the container
WORKDIR /usr/src/app

# Copy the Cargo files
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to cache dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Build dependencies (this layer will be cached)
RUN cargo build --release

# Remove the dummy main.rs
RUN rm src/main.rs

# Copy the source code
COPY src ./src

# Build the actual application
RUN cargo build --release

# Use a minimal runtime image
FROM debian:bookworm-slim

# Install necessary runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user
RUN useradd -r -s /bin/false dcabot

# Set the working directory
WORKDIR /app

# Copy the binary from the builder stage
COPY --from=builder /usr/src/app/target/release/eth-dca-bot /app/eth-dca-bot

# Change ownership to the dcabot user
RUN chown dcabot:dcabot /app/eth-dca-bot

# Switch to the non-root user
USER dcabot

# Run the binary
CMD ["./eth-dca-bot"]
