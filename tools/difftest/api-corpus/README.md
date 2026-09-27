# Route corpus

The queries that `difftest api compare` sends to two nodes, one JSON file per route group. Each file is an array of `{"name", "method", "path", "query", "body"?}` cases; the answers are judged as the harness's `api` module describes.
