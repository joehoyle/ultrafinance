FROM rust:latest
RUN apt-get update && apt-get install -y cmake protobuf-compiler
