FROM docker.io/rust:alpine

RUN apk update && apk add libudev-zero-dev hidapi-dev linux-headers

WORKDIR /app
COPY . /app/

RUN cargo build --features hidraw
