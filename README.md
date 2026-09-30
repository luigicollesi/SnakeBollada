# SnakeBollada

Battlesnake written in Rust, based on the official [Battlesnake Rust Starter Project](https://github.com/BattlesnakeOfficial/starter-snake-rust).

The initial goal is to keep a clean, measurable baseline before evolving the decision engine toward collision safety, flood fill, pathfinding, territory analysis and deeper search.

## Stack

- Rust 1.98.1
- Rocket 0.5.1
- Battlesnake API v1
- Docker

## Run locally

```bash
cargo run
```

The server listens on:

```text
http://0.0.0.0:8000
```

Opening `http://localhost:8000` should return the Battlesnake metadata.

## API

The server exposes the standard Battlesnake endpoints:

- `GET /`
- `POST /start`
- `POST /move`
- `POST /end`

API documentation: https://docs.battlesnake.com/api

## Run with Docker

```bash
docker build -t snake-bollada .
docker run --rm -p 8000:8000 snake-bollada
```

## Local game

Install the Battlesnake CLI and run:

```bash
battlesnake play -W 11 -H 11 --name "SnakeBollada" --url http://localhost:8000 -g solo --browser
```

## Roadmap

1. Official starter behavior
2. Legal move filtering
3. Wall/body collision avoidance
4. Head-to-head danger analysis
5. Flood fill
6. Food/pathfinding
7. Territory/Voronoi evaluation
8. Turn simulation
9. Iterative deepening / adversarial search
10. Benchmarks and optimization

## Attribution

This project starts from the MIT-licensed Battlesnake Rust starter maintained by Battlesnake.
