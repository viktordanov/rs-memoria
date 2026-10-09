# Orders API

This folder owns the orders API.
Run `cargo run` in this folder. The API listens on port 8080.

<!-- memoria:export id="summary" -->
### Orders API

The orders API accepts orders over HTTP and keeps them in memory. It depends on no other service.
<!-- /memoria:export -->

## Endpoints

- `POST /orders` stores one order and returns `201 Created`.
- `GET /orders/{id}` returns one order, or `404 Not Found`.
