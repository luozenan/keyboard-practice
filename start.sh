#!/bin/bash
APP_DIR="$(cd "$(dirname "$0")" && pwd)"
APP_NAME="typing_game"
PORT=30001

PID=$(ss -tlnp "sport = :$PORT" 2>/dev/null | grep -oP 'pid=\K[0-9]+')
if [ -n "$PID" ]; then
    echo "Killing existing process on port $PORT (PID: $PID)"
    kill -9 $PID
    sleep 1
fi

cd "$APP_DIR"
echo "Building $APP_NAME..."
cargo build --release 2>&1

echo "Starting $APP_NAME on port $PORT..."
nohup ./target/release/$APP_NAME > /dev/null 2>&1 &

echo "Started PID: $!"
