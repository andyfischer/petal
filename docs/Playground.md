# Playground

The Petal Playground is an interactive web app for exploring the compiler pipeline.
Write Petal code in the editor and see live tokens, AST, IR, and program output.

## Setup

The playground requires a `.env` file with a port for the API server. Create one
the first time you set up the project:

```bash
echo "PRISM_API_PORT=4027" > playground/.env
```

`PRISM_API_PORT` is required (the API refuses to start without it). `VITE_PORT`
is optional and defaults to 4007.

## Running

The API server and the Vite dev server are separate npm scripts — run them in
two terminals:

```bash
cd playground && npm run dev            # API server (Prism Framework)
cd playground/web && npm run dev         # React + Vite dev server
```

Open the Vite URL (default `http://localhost:4007`) to access the playground.
The frontend proxies `/api/*` to the API server via `vite.config.ts`.

## Stack

- API server: [Prism Framework](https://github.com/facetlayer/prism-framework) — shells out to `rust/target/debug/petal`
- Frontend: React 19 + Vite

## Features

- **Source editor** — Write and edit Petal code with live analysis
- **Example picker** — Load any of the bundled example programs into the editor
- **Token view** — See the lexer output (token types and values)
- **AST view** — See the parsed abstract syntax tree
- **IR view** — See the compiled term graph (dataflow IR)
- **Output view** — See the program's stdout output

All views update as you type.

## API Endpoints

The playground API can also be used directly:

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/analyze` | Returns JSON tokens, AST, IR, and run output |
| `POST` | `/analyze-text` | Returns human-readable text representations |
| `GET` | `/examples` | Lists all example files with their contents |

### Example: Analyze code

```bash
curl -X POST "http://localhost:${PRISM_API_PORT}/analyze" \
  -H "Content-Type: application/json" \
  -d '{"code": "print(1 + 2)"}'
```

Returns JSON with `tokens`, `ast`, `ir`, and `run` fields. Each field contains
either a `json` result or an `error` message.
