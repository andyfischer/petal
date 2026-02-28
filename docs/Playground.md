# Playground

The Petal Playground is an interactive web app for exploring the compiler pipeline.
Write Petal code in the editor and see live tokens, AST, IR, and program output.

## Running

```bash
cd playground && npm run dev
```

This starts:
- An Express API server (port 4810 by default)
- A Vite dev server for the React frontend

Open the URL printed in the terminal to access the playground.

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
curl -X POST http://localhost:4810/analyze \
  -H "Content-Type: application/json" \
  -d '{"code": "print(1 + 2)"}'
```

Returns JSON with `tokens`, `ast`, `ir`, and `run` fields. Each field contains
either a `json` result or an `error` message.
